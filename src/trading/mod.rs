//! Trading engine.
//!
//! In **paper-trading mode** (the default) every order is filled immediately
//! against a deterministic mock market, so the whole system — Telegram bot,
//! API, portfolio accounting, alerts — works end-to-end without any RPC nodes.
//! Swap the `MarketSimulator` for a real DEX/RPC integration later; the rest of
//! the pipeline (risk checks → fill → position accounting → alert checks →
//! order record) stays identical.

use anyhow::{bail, Context, Result};
use libsql::Connection;
use serde::Serialize;

use crate::db::models::Order;
use crate::db::{repo, Db};

pub mod market;
pub mod risk;

pub use market::MarketSimulator;
pub use risk::RiskManager;

/// Result of a filled order, ready to be rendered to the user.
#[derive(Debug, Clone, Serialize)]
pub struct TradeReceipt {
    pub order: Order,
    pub token_symbol: Option<String>,
    pub position_quantity: f64,
    pub position_avg_price: f64,
    pub realized_pnl: Option<f64>,
    pub alerts_fired: i64,
}

pub struct TradingEngine {
    pub sim: MarketSimulator,
    pub risk: RiskManager,
}

impl TradingEngine {
    pub fn new() -> Self {
        Self {
            sim: MarketSimulator::new(),
            risk: RiskManager::default(),
        }
    }

    /// Execute a market buy (or snipe) at the current simulated price.
    ///
    /// `side` must be `"buy"` or `"snipe"` — both fill identically in paper mode.
    pub async fn buy(
        &self,
        db: &Db,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        amount_in: f64,
        slippage: f64,
        side: &str,
    ) -> Result<TradeReceipt> {
        self.risk.validate_trade(token_address, amount_in, slippage)?;

        let price = self.sim.price(token_address);
        let quantity = amount_in / price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        repo::add_to_position(&tx, user_id, wallet_id, token_address, quantity, price).await?;
        let order_id = repo::insert_order(
            &tx,
            &repo::NewOrder {
                user_id,
                wallet_id,
                token_address,
                side,
                amount_in: Some(amount_in),
                amount_out: Some(quantity),
                price: Some(price),
                slippage,
                status: "executed",
                tx_hash: Some("paper"),
                error: None,
            },
        )
        .await?;
        let alerts_fired = repo::trigger_satisfied_alerts(&tx, user_id, token_address, price).await?;
        tx.commit().await?;

        self.receipt(&conn, order_id, token_address, Some(0.0))
            .await
            .map(|mut r| {
                r.alerts_fired = alerts_fired;
                r
            })
    }

    /// Execute a market sell at the current simulated price.
    pub async fn sell(
        &self,
        db: &Db,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        quantity: f64,
        slippage: f64,
    ) -> Result<TradeReceipt> {
        self.risk.validate_trade(token_address, quantity, slippage)?;
        if quantity <= 0.0 {
            bail!("quantity must be positive");
        }

        let price = self.sim.price(token_address);
        let proceeds = quantity * price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        let realized = repo::reduce_position(&tx, user_id, token_address, quantity, price).await?;
        let order_id = repo::insert_order(
            &tx,
            &repo::NewOrder {
                user_id,
                wallet_id,
                token_address,
                side: "sell",
                amount_in: Some(quantity),
                amount_out: Some(proceeds),
                price: Some(price),
                slippage,
                status: "executed",
                tx_hash: Some("paper"),
                error: None,
            },
        )
        .await?;
        let alerts_fired = repo::trigger_satisfied_alerts(&tx, user_id, token_address, price).await?;
        tx.commit().await?;

        let mut receipt = self.receipt(&conn, order_id, token_address, Some(realized)).await?;
        receipt.alerts_fired = alerts_fired;
        Ok(receipt)
    }

    /// Record a pending limit order (not auto-filled in paper mode).
    pub async fn place_limit(
        &self,
        db: &Db,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        limit_price: f64,
        amount_in: f64,
    ) -> Result<Order> {
        if limit_price <= 0.0 || amount_in <= 0.0 {
            bail!("price and amount must be positive");
        }
        let conn = db.conn();
        let order_id = repo::insert_order(
            conn,
            &repo::NewOrder {
                user_id,
                wallet_id,
                token_address,
                side: "limit",
                amount_in: Some(amount_in),
                amount_out: None,
                price: Some(limit_price),
                slippage: 0.0,
                status: "pending",
                tx_hash: None,
                error: None,
            },
        )
        .await?;
        repo::list_orders(conn, user_id, i64::MAX)
            .await?
            .into_iter()
            .find(|o| o.id == order_id)
            .ok_or_else(|| anyhow::anyhow!("order not found after insert"))
    }

    /// Build a human/API-friendly receipt from a stored order row.
    async fn receipt(
        &self,
        conn: &Connection,
        order_id: i64,
        token_address: &str,
        realized_pnl: Option<f64>,
    ) -> Result<TradeReceipt> {
        let order = repo::get_order(conn, order_id).await?.context("order not found")?;
        let symbol = repo::get_token(conn, token_address)
            .await?
            .and_then(|t| t.symbol)
            .or_else(|| Some(short_address(token_address)));
        let pos = repo::get_position(conn, order.user_id, token_address).await?;
        Ok(TradeReceipt {
            order,
            token_symbol: symbol,
            position_quantity: pos.as_ref().map(|p| p.quantity).unwrap_or(0.0),
            position_avg_price: pos.as_ref().map(|p| p.avg_price).unwrap_or(0.0),
            realized_pnl,
            alerts_fired: 0,
        })
    }
}

fn short_address(addr: &str) -> String {
    if addr.len() > 10 {
        format!("{}…{}", &addr[..6], &addr[addr.len() - 4..])
    } else {
        addr.to_string()
    }
}
