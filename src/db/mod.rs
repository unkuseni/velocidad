//! Database layer backed by **libSQL** — runs either against a local SQLite
//! file (`file:./velocidad.db`) or a remote **Turso** instance
//! (`libsql://...turso.io`). The same code path works for both.

pub mod models;
pub mod repo;

use anyhow::{anyhow, Context};
use libsql::Connection;

use crate::config::Config;

/// All schema migrations, applied in order at startup.
/// Each entry is wrapped in a transaction and tracked in `_migrations`.
const MIGRATIONS: &[&str] = &[
    // Users
    r#"
    CREATE TABLE IF NOT EXISTS users (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        telegram_id INTEGER UNIQUE NOT NULL,
        username    TEXT,
        first_name  TEXT,
        created_at  TEXT NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // Wallets (one per user/address; private keys stored encrypted)
    r#"
    CREATE TABLE IF NOT EXISTS wallets (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        address       TEXT NOT NULL,
        network       TEXT NOT NULL DEFAULT 'ethereum',
        label         TEXT NOT NULL DEFAULT 'Main',
        encrypted_key TEXT,
        is_default    INTEGER NOT NULL DEFAULT 0,
        created_at    TEXT NOT NULL DEFAULT (datetime('now')),
        UNIQUE (user_id, address)
    );
    "#,
    // Token registry + cached security scan results
    r#"
    CREATE TABLE IF NOT EXISTS tokens (
        address     TEXT PRIMARY KEY,
        network     TEXT NOT NULL DEFAULT 'ethereum',
        name        TEXT,
        symbol      TEXT,
        decimals    INTEGER DEFAULT 18,
        risk_score  INTEGER DEFAULT 0,
        is_honeypot INTEGER DEFAULT 0,
        liquidity   REAL,
        last_price  REAL,
        updated_at  TEXT
    );
    "#,
    // Orders / trades (buy, sell, snipe, limit)
    r#"
    CREATE TABLE IF NOT EXISTS orders (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id      INTEGER NOT NULL REFERENCES users(id),
        wallet_id    INTEGER REFERENCES wallets(id),
        network      TEXT NOT NULL DEFAULT 'ethereum',
        token_address TEXT NOT NULL,
        side         TEXT NOT NULL CHECK (side IN ('buy','sell','snipe','limit')),
        amount_in    REAL,
        amount_out   REAL,
        price        REAL,
        slippage     REAL DEFAULT 0.05,
        status       TEXT NOT NULL DEFAULT 'pending'
                     CHECK (status IN ('pending','executed','failed','cancelled')),
        tx_hash      TEXT,
        error        TEXT,
        created_at   TEXT NOT NULL DEFAULT (datetime('now')),
        executed_at  TEXT
    );
    "#,
    // Positions (open holdings per user/token)
    r#"
    CREATE TABLE IF NOT EXISTS positions (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id),
        wallet_id     INTEGER REFERENCES wallets(id),
        token_address TEXT NOT NULL,
        network       TEXT NOT NULL DEFAULT 'ethereum',
        quantity      REAL NOT NULL DEFAULT 0,
        avg_price     REAL NOT NULL DEFAULT 0,
        realized_pnl  REAL NOT NULL DEFAULT 0,
        updated_at    TEXT,
        UNIQUE (user_id, token_address)
    );
    "#,
    // Price alerts
    r#"
    CREATE TABLE IF NOT EXISTS alerts (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id),
        token_address TEXT NOT NULL,
        condition     TEXT NOT NULL CHECK (condition IN ('above','below')),
        target_price  REAL NOT NULL,
        is_triggered  INTEGER NOT NULL DEFAULT 0,
        created_at    TEXT NOT NULL DEFAULT (datetime('now')),
        triggered_at  TEXT
    );
    "#,
    // Per-user key/value settings (slippage, defaults, ...)
    r#"
    CREATE TABLE IF NOT EXISTS settings (
        user_id INTEGER NOT NULL,
        key     TEXT NOT NULL,
        value   TEXT NOT NULL,
        PRIMARY KEY (user_id, key)
    );
    "#,
    // Alerts are chain-scoped (multi-chain support)
    r#"
    ALTER TABLE alerts ADD COLUMN network TEXT NOT NULL DEFAULT 'ethereum';
    "#,
    // Indexes for the background workers
    r#"
    CREATE INDEX IF NOT EXISTS idx_orders_pending_limit
        ON orders (status, side) WHERE status = 'pending' AND side = 'limit';
    "#,
    r#"
    CREATE INDEX IF NOT EXISTS idx_alerts_untriggered
        ON alerts (is_triggered);
    "#,
    // NOTE: libSQL execute() runs a single statement per migration — multi-
    // statement strings silently execute only the first one. Each table rebuild
    // below is therefore split into one entry per statement.
    // Positions: scope per (user, network, token) — the same token address
    // exists on multiple chains (e.g. WETH on Optimism + Base). Zero-quantity
    // rows are kept so realized PnL aggregates survive full closes.
    r#"
    CREATE TABLE IF NOT EXISTS positions_v2 (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id       INTEGER NOT NULL REFERENCES users(id),
        wallet_id     INTEGER REFERENCES wallets(id),
        token_address TEXT NOT NULL,
        network       TEXT NOT NULL DEFAULT 'ethereum',
        quantity      REAL NOT NULL DEFAULT 0,
        avg_price     REAL NOT NULL DEFAULT 0,
        realized_pnl  REAL NOT NULL DEFAULT 0,
        updated_at    TEXT,
        UNIQUE (user_id, network, token_address)
    );
    "#,
    r#"
    INSERT INTO positions_v2 (id, user_id, wallet_id, token_address, network, quantity, avg_price, realized_pnl, updated_at)
        SELECT id, user_id, wallet_id, token_address, network, quantity, avg_price, realized_pnl, updated_at FROM positions
        WHERE NOT EXISTS (SELECT 1 FROM positions_v2);
    "#,
    r#"DROP TABLE IF EXISTS positions;"#,
    r#"ALTER TABLE positions_v2 RENAME TO positions;"#,
    // Tokens: (network, address) composite key.
    r#"
    CREATE TABLE IF NOT EXISTS tokens_v2 (
        network     TEXT NOT NULL DEFAULT 'ethereum',
        address     TEXT NOT NULL,
        name        TEXT,
        symbol      TEXT,
        decimals    INTEGER DEFAULT 18,
        risk_score  INTEGER DEFAULT 0,
        is_honeypot INTEGER DEFAULT 0,
        liquidity   REAL,
        last_price  REAL,
        updated_at  TEXT,
        PRIMARY KEY (network, address)
    );
    "#,
    r#"
    INSERT INTO tokens_v2 (network, address, name, symbol, decimals, risk_score, is_honeypot, liquidity, last_price, updated_at)
        SELECT network, address, name, symbol, decimals, risk_score, is_honeypot, liquidity, last_price, updated_at FROM tokens
        WHERE NOT EXISTS (SELECT 1 FROM tokens_v2);
    "#,
    r#"DROP TABLE IF EXISTS tokens;"#,
    r#"ALTER TABLE tokens_v2 RENAME TO tokens;"#,
    // Take-profit / stop-loss per open position (native units, 0 = unset).
    r#"
    ALTER TABLE positions ADD COLUMN tp_price REAL DEFAULT 0;
    "#,
    r#"
    ALTER TABLE positions ADD COLUMN sl_price REAL DEFAULT 0;
    "#,
];

/// Shared handle to the libSQL database. Cheap to clone (`Connection` is
/// internally an `Arc`), so `Arc<Db>` or `Db` can be passed around freely.
#[derive(Clone)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open a database from config and apply pending migrations.
    ///
    /// - `file:./velocidad.db` or a bare path  → local libSQL file
    /// - `libsql://<db>.turso.io`              → remote Turso (needs `TURSO_AUTH_TOKEN`)
    pub async fn open(config: &Config) -> anyhow::Result<Self> {
        let db = if config.is_remote() {
            let token = config
                .turso_auth_token
                .clone()
                .or_else(|| auth_token_from_url(&config.database_url))
                .ok_or_else(|| {
                    anyhow!("TURSO_AUTH_TOKEN is required when using a remote Turso database")
                })?;
            tracing::info!(url = %config.database_url, "connecting to remote Turso database");
            libsql::Builder::new_remote(config.database_url.clone(), token)
                .build()
                .await
                .context("failed to build remote libSQL database")?
        } else {
            let path = config
                .database_url
                .strip_prefix("file:")
                .unwrap_or(&config.database_url);
            tracing::info!(path, "opening local libSQL database");
            libsql::Builder::new_local(path)
                .build()
                .await
                .context("failed to build local libSQL database")?
        };

        let conn = db
            .connect()
            .context("failed to acquire libSQL connection")?;
        // Wait up to 5s for a busy database instead of failing immediately
        // (worker + bot + API share one connection; deferred transactions can
        // collide). Remote Turso may reject the pragma — ignore that.
        let _ = conn.execute("PRAGMA busy_timeout = 5000", ()).await;

        let this = Self { conn };
        this.migrate().await?;
        Ok(this)
    }

    /// Access the underlying libSQL connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Apply any migrations that have not run yet.
    async fn migrate(&self) -> anyhow::Result<()> {
        self.conn
            .execute(
                "CREATE TABLE IF NOT EXISTS _migrations (
                    name       TEXT PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
                )",
                (),
            )
            .await?;

        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let name = format!("migration_{:03}", i);
            let already: i64 = self
                .conn
                .query(
                    "SELECT COUNT(*) FROM _migrations WHERE name = ?1",
                    libsql::params![name.clone()],
                )
                .await?
                .next()
                .await?
                .map(|row| row.get::<i64>(0))
                .transpose()?
                .unwrap_or(0);

            if already > 0 {
                continue;
            }

            // SQLite has no ADD COLUMN IF NOT EXISTS: if a previous run added
            // the column but crashed before recording the migration, skip it.
            if let Some((table, column)) = add_column_target(sql) {
                let exists: i64 = self
                    .conn
                    .query(
                        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                        libsql::params![table, column],
                    )
                    .await?
                    .next()
                    .await?
                    .map(|row| row.get::<i64>(0))
                    .transpose()?
                    .unwrap_or(0);
                if exists > 0 {
                    tracing::info!(name, "column already exists — skipping migration");
                    let tx = self.conn.transaction().await?;
                    tx.execute(
                        "INSERT INTO _migrations (name) VALUES (?1)",
                        libsql::params![name],
                    )
                    .await?;
                    tx.commit().await?;
                    continue;
                }
            }

            tracing::info!(name, "applying migration");
            let tx = self.conn.transaction().await?;
            tx.execute(sql, ()).await?;
            tx.execute(
                "INSERT INTO _migrations (name) VALUES (?1)",
                libsql::params![name],
            )
            .await?;
            tx.commit().await?;
        }
        Ok(())
    }
}

/// Parse "ALTER TABLE <t> ADD COLUMN <c> ..." into (table, column), if the
/// statement is one. Used to make column additions re-runnable.
fn add_column_target(sql: &str) -> Option<(&str, &str)> {
    let s = sql.trim_start();
    let rest = s.strip_prefix("ALTER TABLE")?.trim_start();
    let (table, rest) = rest.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("ADD COLUMN")?.trim_start();
    let (column, _) = rest.split_once(char::is_whitespace)?;
    Some((table, column))
}

/// Turso URLs sometimes embed the token as basic-auth, e.g.
/// `libsql://token@host.turso.io`. Extract it if present.
fn auth_token_from_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("libsql://")?;
    let (userinfo, _) = rest.split_once('@')?;
    if userinfo.contains(':') {
        userinfo.split_once(':').map(|(_, t)| t.to_string())
    } else {
        Some(userinfo.to_string())
    }
}
