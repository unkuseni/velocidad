//! Repository functions — parameterized queries against libSQL.
//!
//! These are deliberately thin: they take a `&Connection` so they can be used
//! either directly or inside a transaction. libSQL 0.9's connection API is
//! fully async, so every function is `async fn`.

use anyhow::{bail, Context, Result};
use libsql::{params, Connection};

use super::models::{Alert, Order, Position, Token, User, Wallet};

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

pub async fn get_user(conn: &Connection, telegram_id: i64) -> Result<Option<User>> {
    let mut rows = conn
        .query(
            "SELECT id, telegram_id, username, first_name, created_at
             FROM users WHERE telegram_id = ?1",
            params![telegram_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(User {
        id: row.get(0)?,
        telegram_id: row.get(1)?,
        username: row.get(2)?,
        first_name: row.get(3)?,
        created_at: row.get(4)?,
    }))
}

pub async fn get_or_create_user(
    conn: &Connection,
    telegram_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
) -> Result<User> {
    if let Some(user) = get_user(conn, telegram_id).await? {
        // refresh profile fields opportunistically
        conn.execute(
            "UPDATE users SET username = COALESCE(?2, username),
                               first_name = COALESCE(?3, first_name)
             WHERE telegram_id = ?1",
            params![telegram_id, username, first_name],
        )
        .await?;
        return Ok(user);
    }
    conn.execute(
        "INSERT INTO users (telegram_id, username, first_name) VALUES (?1, ?2, ?3)",
        params![telegram_id, username, first_name],
    )
    .await?;
    get_user(conn, telegram_id)
        .await?
        .context("user insert failed")
}

// ---------------------------------------------------------------------------
// Wallets
// ---------------------------------------------------------------------------

pub async fn list_wallets(conn: &Connection, user_id: i64) -> Result<Vec<Wallet>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, address, network, label, encrypted_key, is_default, created_at
             FROM wallets WHERE user_id = ?1 ORDER BY is_default DESC, id ASC",
            params![user_id],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Wallet {
            id: row.get(0)?,
            user_id: row.get(1)?,
            address: row.get(2)?,
            network: row.get(3)?,
            label: row.get(4)?,
            encrypted_key: row.get(5)?,
            is_default: row.get::<i64>(6)? != 0,
            created_at: row.get(7)?,
        });
    }
    Ok(out)
}

pub async fn get_default_wallet(conn: &Connection, user_id: i64) -> Result<Option<Wallet>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, address, network, label, encrypted_key, is_default, created_at
             FROM wallets WHERE user_id = ?1 ORDER BY is_default DESC, id ASC LIMIT 1",
            params![user_id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Wallet {
        id: row.get(0)?,
        user_id: row.get(1)?,
        address: row.get(2)?,
        network: row.get(3)?,
        label: row.get(4)?,
        encrypted_key: row.get(5)?,
        is_default: row.get::<i64>(6)? != 0,
        created_at: row.get(7)?,
    }))
}

pub async fn insert_wallet(
    conn: &Connection,
    user_id: i64,
    address: &str,
    label: &str,
    encrypted_key: Option<&str>,
    is_default: bool,
) -> Result<Wallet> {
    if is_default {
        conn.execute(
            "UPDATE wallets SET is_default = 0 WHERE user_id = ?1",
            params![user_id],
        )
        .await?;
    }
    conn.execute(
        "INSERT INTO wallets (user_id, address, label, encrypted_key, is_default)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (user_id, address) DO UPDATE SET
            label = excluded.label,
            encrypted_key = COALESCE(excluded.encrypted_key, wallets.encrypted_key),
            is_default = excluded.is_default",
        params![user_id, address, label, encrypted_key, is_default as i64],
    )
    .await?;
    let mut rows = conn
        .query(
            "SELECT id, user_id, address, network, label, encrypted_key, is_default, created_at
             FROM wallets WHERE user_id = ?1 AND address = ?2",
            params![user_id, address],
        )
        .await?;
    let row = rows.next().await?.context("wallet insert failed")?;
    Ok(Wallet {
        id: row.get(0)?,
        user_id: row.get(1)?,
        address: row.get(2)?,
        network: row.get(3)?,
        label: row.get(4)?,
        encrypted_key: row.get(5)?,
        is_default: row.get::<i64>(6)? != 0,
        created_at: row.get(7)?,
    })
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

pub async fn upsert_token(conn: &Connection, t: &Token) -> Result<()> {
    conn.execute(
        "INSERT INTO tokens (address, network, name, symbol, decimals, risk_score,
                             is_honeypot, liquidity, last_price, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (address) DO UPDATE SET
            name = COALESCE(excluded.name, tokens.name),
            symbol = COALESCE(excluded.symbol, tokens.symbol),
            risk_score = excluded.risk_score,
            is_honeypot = excluded.is_honeypot,
            liquidity = COALESCE(excluded.liquidity, tokens.liquidity),
            last_price = COALESCE(excluded.last_price, tokens.last_price),
            updated_at = excluded.updated_at",
        params![
            t.address.clone(),
            t.network.clone(),
            t.name.clone(),
            t.symbol.clone(),
            t.decimals,
            t.risk_score,
            t.is_honeypot as i64,
            t.liquidity,
            t.last_price,
            t.updated_at.clone()
        ],
    )
    .await?;
    Ok(())
}

pub async fn get_token(conn: &Connection, address: &str) -> Result<Option<Token>> {
    let mut rows = conn
        .query(
            "SELECT address, network, name, symbol, decimals, risk_score,
                    is_honeypot, liquidity, last_price, updated_at
             FROM tokens WHERE address = ?1",
            params![address],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Token {
        address: row.get(0)?,
        network: row.get(1)?,
        name: row.get(2)?,
        symbol: row.get(3)?,
        decimals: row.get(4)?,
        risk_score: row.get(5)?,
        is_honeypot: row.get::<i64>(6)? != 0,
        liquidity: row.get(7)?,
        last_price: row.get(8)?,
        updated_at: row.get(9)?,
    }))
}

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

pub struct NewOrder<'a> {
    pub user_id: i64,
    pub wallet_id: Option<i64>,
    pub network: &'a str,
    pub token_address: &'a str,
    pub side: &'a str,
    pub amount_in: Option<f64>,
    pub amount_out: Option<f64>,
    pub price: Option<f64>,
    pub slippage: f64,
    pub status: &'a str,
    pub tx_hash: Option<&'a str>,
    pub error: Option<&'a str>,
}

pub async fn insert_order(conn: &Connection, o: &NewOrder<'_>) -> Result<i64> {
    conn.execute(
        "INSERT INTO orders (user_id, wallet_id, network, token_address, side, amount_in,
                             amount_out, price, slippage, status, tx_hash, error,
                             executed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 CASE WHEN ?10 = 'executed' THEN datetime('now') ELSE NULL END)",
        params![
            o.user_id,
            o.wallet_id,
            o.network,
            o.token_address,
            o.side,
            o.amount_in,
            o.amount_out,
            o.price,
            o.slippage,
            o.status,
            o.tx_hash,
            o.error
        ],
    )
    .await?;
    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id = rows.next().await?.context("order insert failed")?.get::<i64>(0)?;
    Ok(id)
}

pub async fn get_order(conn: &Connection, id: i64) -> Result<Option<Order>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, wallet_id, network, token_address, side, amount_in,
                    amount_out, price, slippage, status, tx_hash, error, created_at, executed_at
             FROM orders WHERE id = ?1",
            params![id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Order {
        id: row.get(0)?,
        user_id: row.get(1)?,
        wallet_id: row.get(2)?,
        network: row.get(3)?,
        token_address: row.get(4)?,
        side: row.get(5)?,
        amount_in: row.get(6)?,
        amount_out: row.get(7)?,
        price: row.get(8)?,
        slippage: row.get(9)?,
        status: row.get(10)?,
        tx_hash: row.get(11)?,
        error: row.get(12)?,
        created_at: row.get(13)?,
        executed_at: row.get(14)?,
    }))
}

pub async fn list_orders(conn: &Connection, user_id: i64, limit: i64) -> Result<Vec<Order>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, wallet_id, network, token_address, side, amount_in,
                    amount_out, price, slippage, status, tx_hash, error, created_at, executed_at
             FROM orders WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2",
            params![user_id, limit],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Order {
            id: row.get(0)?,
            user_id: row.get(1)?,
            wallet_id: row.get(2)?,
            network: row.get(3)?,
            token_address: row.get(4)?,
            side: row.get(5)?,
            amount_in: row.get(6)?,
            amount_out: row.get(7)?,
            price: row.get(8)?,
            slippage: row.get(9)?,
            status: row.get(10)?,
            tx_hash: row.get(11)?,
            error: row.get(12)?,
            created_at: row.get(13)?,
            executed_at: row.get(14)?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Positions
// ---------------------------------------------------------------------------

pub async fn get_position(
    conn: &Connection,
    user_id: i64,
    token_address: &str,
) -> Result<Option<Position>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, wallet_id, token_address, network, quantity,
                    avg_price, realized_pnl, updated_at
             FROM positions WHERE user_id = ?1 AND token_address = ?2",
            params![user_id, token_address],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Position {
        id: row.get(0)?,
        user_id: row.get(1)?,
        wallet_id: row.get(2)?,
        token_address: row.get(3)?,
        network: row.get(4)?,
        quantity: row.get(5)?,
        avg_price: row.get(6)?,
        realized_pnl: row.get(7)?,
        updated_at: row.get(8)?,
    }))
}

pub async fn list_positions(conn: &Connection, user_id: i64) -> Result<Vec<Position>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, wallet_id, token_address, network, quantity,
                    avg_price, realized_pnl, updated_at
             FROM positions WHERE user_id = ?1 AND quantity > 0 ORDER BY id ASC",
            params![user_id],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Position {
            id: row.get(0)?,
            user_id: row.get(1)?,
            wallet_id: row.get(2)?,
            token_address: row.get(3)?,
            network: row.get(4)?,
            quantity: row.get(5)?,
            avg_price: row.get(6)?,
            realized_pnl: row.get(7)?,
            updated_at: row.get(8)?,
        });
    }
    Ok(out)
}

/// Add `qty` tokens at `price` to a position (weighted-average cost basis).
pub async fn add_to_position(
    conn: &Connection,
    user_id: i64,
    wallet_id: Option<i64>,
    network: &str,
    token_address: &str,
    qty: f64,
    price: f64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO positions (user_id, wallet_id, network, token_address, quantity, avg_price, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'))
         ON CONFLICT (user_id, token_address) DO UPDATE SET
            quantity = positions.quantity + excluded.quantity,
            avg_price = CASE
                WHEN positions.quantity + excluded.quantity = 0 THEN 0
                ELSE (positions.quantity * positions.avg_price + excluded.quantity * excluded.avg_price)
                     / (positions.quantity + excluded.quantity)
            END,
            wallet_id = COALESCE(excluded.wallet_id, positions.wallet_id),
            updated_at = datetime('now')",
        params![user_id, wallet_id, network, token_address, qty, price],
    )
    .await?;
    Ok(())
}

/// Remove `qty` tokens from a position; computes realized PnL. Removes the row
/// entirely when the position reaches zero.
pub async fn reduce_position(
    conn: &Connection,
    user_id: i64,
    token_address: &str,
    qty: f64,
    price: f64,
) -> Result<f64> {
    let Some(pos) = get_position(conn, user_id, token_address).await? else {
        bail!("no open position for this token");
    };
    if qty > pos.quantity + 1e-12 {
        bail!(
            "not enough tokens: you hold {:.6}, tried to sell {:.6}",
            pos.quantity,
            qty
        );
    }
    let realized = (price - pos.avg_price) * qty;

    let remaining = pos.quantity - qty;
    if remaining <= 1e-12 {
        conn.execute(
            "DELETE FROM positions WHERE user_id = ?1 AND token_address = ?2",
            params![user_id, token_address],
        )
        .await?;
    } else {
        conn.execute(
            "UPDATE positions SET quantity = ?3, realized_pnl = realized_pnl + ?4,
                                   updated_at = datetime('now')
             WHERE user_id = ?1 AND token_address = ?2",
            params![user_id, token_address, remaining, realized],
        )
        .await?;
    }
    Ok(realized)
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

pub async fn insert_alert(
    conn: &Connection,
    user_id: i64,
    network: &str,
    token_address: &str,
    condition: &str,
    target_price: f64,
) -> Result<Alert> {
    conn.execute(
        "INSERT INTO alerts (user_id, network, token_address, condition, target_price)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user_id, network, token_address, condition, target_price],
    )
    .await?;
    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id = rows.next().await?.context("alert insert failed")?.get::<i64>(0)?;
    get_alert(conn, id).await?.context("alert not found after insert")
}

pub async fn get_alert(conn: &Connection, id: i64) -> Result<Option<Alert>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, network, token_address, condition, target_price,
                    is_triggered, created_at, triggered_at
             FROM alerts WHERE id = ?1",
            params![id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Alert {
        id: row.get(0)?,
        user_id: row.get(1)?,
        network: row.get(2)?,
        token_address: row.get(3)?,
        condition: row.get(4)?,
        target_price: row.get(5)?,
        is_triggered: row.get::<i64>(6)? != 0,
        created_at: row.get(7)?,
        triggered_at: row.get(8)?,
    }))
}

pub async fn list_alerts(conn: &Connection, user_id: i64) -> Result<Vec<Alert>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, network, token_address, condition, target_price,
                    is_triggered, created_at, triggered_at
             FROM alerts WHERE user_id = ?1 ORDER BY is_triggered ASC, id DESC",
            params![user_id],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Alert {
            id: row.get(0)?,
            user_id: row.get(1)?,
            network: row.get(2)?,
            token_address: row.get(3)?,
            condition: row.get(4)?,
            target_price: row.get(5)?,
            is_triggered: row.get::<i64>(6)? != 0,
            created_at: row.get(7)?,
            triggered_at: row.get(8)?,
        });
    }
    Ok(out)
}

/// Mark every untriggered alert for `token_address` that is now satisfied.
/// Returns how many alerts fired.
pub async fn trigger_satisfied_alerts(
    conn: &Connection,
    user_id: i64,
    token_address: &str,
    price: f64,
) -> Result<i64> {
    let fired = conn
        .execute(
            "UPDATE alerts SET is_triggered = 1, triggered_at = datetime('now')
             WHERE user_id = ?1 AND token_address = ?2 AND is_triggered = 0
               AND ((condition = 'above' AND ?3 >= target_price)
                 OR (condition = 'below' AND ?3 <= target_price))",
            params![user_id, token_address, price],
        )
        .await?;
    Ok(fired as i64)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

pub async fn get_setting(conn: &Connection, user_id: i64, key: &str) -> Result<Option<String>> {
    let mut rows = conn
        .query(
            "SELECT value FROM settings WHERE user_id = ?1 AND key = ?2",
            params![user_id, key],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}


// ---------------------------------------------------------------------------
// Background-worker helpers
// ---------------------------------------------------------------------------

/// User lookup by internal id (for notifications).
pub async fn get_user_by_id(conn: &Connection, id: i64) -> Result<Option<User>> {
    let mut rows = conn
        .query(
            "SELECT id, telegram_id, username, first_name, created_at
             FROM users WHERE id = ?1",
            params![id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(User {
        id: row.get(0)?,
        telegram_id: row.get(1)?,
        username: row.get(2)?,
        first_name: row.get(3)?,
        created_at: row.get(4)?,
    }))
}

/// All pending limit orders across all users (for the matcher worker).
pub async fn list_pending_limits(conn: &Connection) -> Result<Vec<Order>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, wallet_id, network, token_address, side, amount_in,
                    amount_out, price, slippage, status, tx_hash, error, created_at, executed_at
             FROM orders WHERE side = 'limit' AND status = 'pending' ORDER BY id ASC",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Order {
            id: row.get(0)?,
            user_id: row.get(1)?,
            wallet_id: row.get(2)?,
            network: row.get(3)?,
            token_address: row.get(4)?,
            side: row.get(5)?,
            amount_in: row.get(6)?,
            amount_out: row.get(7)?,
            price: row.get(8)?,
            slippage: row.get(9)?,
            status: row.get(10)?,
            tx_hash: row.get(11)?,
            error: row.get(12)?,
            created_at: row.get(13)?,
            executed_at: row.get(14)?,
        });
    }
    Ok(out)
}

/// All untriggered alerts (for the alert poller worker).
pub async fn list_untriggered_alerts(conn: &Connection) -> Result<Vec<Alert>> {
    let mut rows = conn
        .query(
            "SELECT id, user_id, network, token_address, condition, target_price,
                    is_triggered, created_at, triggered_at
             FROM alerts WHERE is_triggered = 0 ORDER BY id ASC",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Alert {
            id: row.get(0)?,
            user_id: row.get(1)?,
            network: row.get(2)?,
            token_address: row.get(3)?,
            condition: row.get(4)?,
            target_price: row.get(5)?,
            is_triggered: row.get::<i64>(6)? != 0,
            created_at: row.get(7)?,
            triggered_at: row.get(8)?,
        });
    }
    Ok(out)
}

/// Fill a pending order in place with the executed amounts.
pub async fn execute_pending_order(
    conn: &Connection,
    id: i64,
    amount_out: f64,
    price: f64,
    tx_hash: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE orders SET status = 'executed', amount_out = ?2, price = ?3,
                           tx_hash = ?4, executed_at = datetime('now')
         WHERE id = ?1",
        params![id, amount_out, price, tx_hash],
    )
    .await?;
    Ok(())
}

/// Mark a single alert as triggered (used by the alert poller).
pub async fn mark_alert_triggered(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE alerts SET is_triggered = 1, triggered_at = datetime('now') WHERE id = ?1",
        params![id],
    )
    .await?;
    Ok(())
}

/// Mark an order failed with an error message.
pub async fn fail_order(conn: &Connection, id: i64, error: &str) -> Result<()> {
    conn.execute(
        "UPDATE orders SET status = 'failed', error = ?2 WHERE id = ?1",
        params![id, error],
    )
    .await?;
    Ok(())
}

/// Fetch the current chain setting for a user (canonical chain id).
pub async fn user_chain(conn: &Connection, user_id: i64, default: &str) -> Result<String> {
    Ok(get_setting(conn, user_id, "chain")
        .await?
        .unwrap_or_else(|| default.to_string()))
}

pub async fn set_setting(conn: &Connection, user_id: i64, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (user_id, key, value) VALUES (?1, ?2, ?3)
         ON CONFLICT (user_id, key) DO UPDATE SET value = excluded.value",
        params![user_id, key, value],
    )
    .await?;
    Ok(())
}

