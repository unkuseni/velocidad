//! Trading engine — chain-aware order execution against the market feed.
//!
//! Prices come from the shared [`MarketData`] (DexScreener with a simulator
//! fallback), so the same engine powers paper trading with real prices and,
//! once `ZEROEX_API_KEY` is configured, live swaps signed by [`crate::swap`].

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use libsql::Connection;
use serde::Serialize;

use crate::chains::Chain;
use crate::db::models::Order;
use crate::db::{repo, Db};
use crate::market::MarketData;

pub mod risk;

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
    /// `"paper"` for simulated fills, or the real tx hash for live swaps.
    pub tx_ref: String,
    /// Explorer link for live swaps.
    pub explorer_link: Option<String>,
}

pub struct TradingEngine {
    pub market: Arc<MarketData>,
    pub risk: RiskManager,
}

impl TradingEngine {
    pub fn new(market: Arc<MarketData>) -> Self {
        Self {
            market,
            risk: RiskManager::default(),
        }
    }

    /// Execute a market buy (or snipe) at the current price on `chain`.
    ///
    /// `amount_native` is the spend in the chain's native coin (ETH/BNB/…).
    pub async fn buy(
        &self,
        db: &Db,
        chain: &Chain,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        amount_native: f64,
        slippage: f64,
        side: &str,
    ) -> Result<TradeReceipt> {
        self.risk.validate_trade(chain, token_address, amount_native, slippage)?;

        let quote = self.market.quote(chain, token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            bail!("no valid price for this token on {}", chain.id);
        }
        let quantity = amount_native / price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        repo::add_to_position(&tx, user_id, wallet_id, chain.id, token_address, quantity, price).await?;
        let order_id = repo::insert_order(
            &tx,
            &repo::NewOrder {
                user_id,
                wallet_id,
                network: chain.id,
                token_address,
                side,
                amount_in: Some(amount_native),
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

    /// Execute a market sell at the current price on `chain`.
    pub async fn sell(
        &self,
        db: &Db,
        chain: &Chain,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        quantity: f64,
        slippage: f64,
    ) -> Result<TradeReceipt> {
        self.risk.validate_trade(chain, token_address, quantity, slippage)?;
        if quantity <= 0.0 {
            bail!("quantity must be positive");
        }

        let quote = self.market.quote(chain, token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            bail!("no valid price for this token on {}", chain.id);
        }
        let proceeds = quantity * price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        let realized = repo::reduce_position(&tx, user_id, token_address, quantity, price).await?;
        let order_id = repo::insert_order(
            &tx,
            &repo::NewOrder {
                user_id,
                wallet_id,
                network: chain.id,
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

    /// Record a pending limit order on `chain` (filled by the worker).
    pub async fn place_limit(
        &self,
        db: &Db,
        chain: &Chain,
        user_id: i64,
        wallet_id: Option<i64>,
        token_address: &str,
        limit_price: f64,
        amount_native: f64,
    ) -> Result<Order> {
        if limit_price <= 0.0 || amount_native <= 0.0 {
            bail!("price and amount must be positive");
        }
        let conn = db.conn();
        let order_id = repo::insert_order(
            conn,
            &repo::NewOrder {
                user_id,
                wallet_id,
                network: chain.id,
                token_address,
                side: "limit",
                amount_in: Some(amount_native),
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


    /// Fill a pending limit order at the current market price (called by the
    /// limit-order matcher worker). Updates the original order row in place.
    pub async fn fill_limit(
        &self,
        db: &Db,
        chain: &Chain,
        order: &Order,
    ) -> Result<TradeReceipt> {
        let quote = self.market.quote(chain, &order.token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            bail!("no valid price for this token on {}", chain.id);
        }
        let amount = order.amount_in.unwrap_or(0.0);
        if amount <= 0.0 {
            bail!("limit order has no amount_in");
        }
        let quantity = amount / price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        repo::add_to_position(
            &tx,
            order.user_id,
            order.wallet_id,
            chain.id,
            &order.token_address,
            quantity,
            price,
        )
        .await?;
        repo::execute_pending_order(&tx, order.id, quantity, price, "paper").await?;
        let alerts_fired = repo::trigger_satisfied_alerts(&tx, order.user_id, &order.token_address, price).await?;
        tx.commit().await?;

        self.receipt(&conn, order.id, &order.token_address, Some(0.0))
            .await
            .map(|mut r| {
                r.alerts_fired = alerts_fired;
                r
            })
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
            tx_ref: "paper".to_string(),
            explorer_link: None,
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
