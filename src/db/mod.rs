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

        let conn = db.connect().context("failed to acquire libSQL connection")?;

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
