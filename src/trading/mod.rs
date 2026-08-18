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

/// Why a limit order was not filled on a tick.
#[derive(Debug)]
pub enum LimitFillError {
    /// Transient: skip this tick; the order stays pending.
    Skip(String),
    /// Permanent: the order should be marked failed.
    Fail(String),
}

impl std::fmt::Display for LimitFillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LimitFillError::Skip(m) => write!(f, "skipped: {m}"),
            LimitFillError::Fail(m) => write!(f, "failed: {m}"),
        }
    }
}

impl std::error::Error for LimitFillError {}

impl From<anyhow::Error> for LimitFillError {
    fn from(e: anyhow::Error) -> Self {
        LimitFillError::Fail(e.to_string())
    }
}

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
    /// `"dexscreener"` (live data) or `"simulator"` (no live quote found).
    pub price_source: &'static str,
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
        self.risk
            .validate_trade(chain, token_address, amount_native, slippage)?;

        let quote = self.market.quote(chain, token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            bail!("no valid price for this token on {}", chain.id);
        }
        if quote.source == "simulator" {
            tracing::warn!(token = %token_address, chain = %chain.id, "no live price — simulated fill");
        }
        let quantity = amount_native / price;

        let conn = db.conn();
        let tx = conn
            .transaction()
            .await
            .map_err(|e| LimitFillError::Fail(e.to_string()))?;
        repo::add_to_position(
            &tx,
            user_id,
            wallet_id,
            chain.id,
            token_address,
            quantity,
            price,
        )
        .await?;
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
        let alerts_fired =
            repo::trigger_satisfied_alerts(&tx, user_id, chain.id, token_address, price).await?;
        tx.commit().await?;

        self.receipt(conn, order_id, token_address, Some(0.0), quote.source)
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
        self.risk
            .validate_trade(chain, token_address, quantity, slippage)?;
        if quantity <= 0.0 {
            bail!("quantity must be positive");
        }

        let quote = self.market.quote(chain, token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            bail!("no valid price for this token on {}", chain.id);
        }
        if quote.source == "simulator" {
            tracing::warn!(token = %token_address, chain = %chain.id, "no live price — simulated fill");
        }
        let proceeds = quantity * price;

        let conn = db.conn();
        let tx = conn.transaction().await?;
        let realized =
            repo::reduce_position(&tx, user_id, chain.id, token_address, quantity, price).await?;
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
        let alerts_fired =
            repo::trigger_satisfied_alerts(&tx, user_id, chain.id, token_address, price).await?;
        tx.commit().await?;

        let mut receipt = self
            .receipt(conn, order_id, token_address, Some(realized), quote.source)
            .await?;
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
    ///
    /// Returns LimitFillError::Skip for transient conditions (no live price,
    /// price above the limit, already filled) so the order stays pending, and
    /// LimitFillError::Fail for permanent problems.
    pub async fn fill_limit(
        &self,
        db: &Db,
        chain: &Chain,
        order: &Order,
    ) -> std::result::Result<TradeReceipt, LimitFillError> {
        let quote = self.market.quote(chain, &order.token_address).await;
        let price = quote.price_native;
        if price <= 0.0 {
            return Err(LimitFillError::Skip(format!(
                "no valid price for this token on {}",
                chain.id
            )));
        }
        if quote.source == "simulator" {
            return Err(LimitFillError::Skip(
                "no live price for this token — refusing to fill at a simulated price".to_string(),
            ));
        }
        // Re-check against the user's limit with the freshest quote: the
        // worker trigger can be up to a tick (~15s) stale.
        if let Some(limit) = order.price {
            if price > limit {
                return Err(LimitFillError::Skip(format!(
                    "price {} above the limit {} — waiting",
                    price, limit
                )));
            }
        }
        let amount = order.amount_in.unwrap_or(0.0);
        if amount <= 0.0 {
            return Err(LimitFillError::Fail(
                "limit order has no amount_in".to_string(),
            ));
        }
        let quantity = amount / price;

        let conn = db.conn();
        let tx = conn
            .transaction()
            .await
            .map_err(|e| LimitFillError::Fail(e.to_string()))?;
        let res = async {
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
            let affected =
                repo::execute_pending_order(&tx, order.id, quantity, price, "paper").await?;
            if affected == 0 {
                // Another tick already filled this order.
                return Err(LimitFillError::Skip("already filled".to_string()));
            }
            let alerts_fired = repo::trigger_satisfied_alerts(
                &tx,
                order.user_id,
                chain.id,
                &order.token_address,
                price,
            )
            .await?;
            tx.commit()
                .await
                .map_err(|e| LimitFillError::Fail(e.to_string()))?;
            Ok::<_, LimitFillError>(alerts_fired)
        }
        .await;

        let alerts_fired = match res {
            Ok(fired) => fired,
            Err(e) => return Err(e),
        };

        self.receipt(
            conn,
            order.id,
            &order.token_address,
            Some(0.0),
            quote.source,
        )
        .await
        .map(|mut r| {
            r.alerts_fired = alerts_fired;
            r
        })
        .map_err(|e| LimitFillError::Fail(e.to_string()))
    }

    /// Build a human/API-friendly receipt from a stored order row.
    async fn receipt(
        &self,
        conn: &Connection,
        order_id: i64,
        token_address: &str,
        realized_pnl: Option<f64>,
        price_source: &'static str,
    ) -> Result<TradeReceipt> {
        let order = repo::get_order(conn, order_id)
            .await?
            .context("order not found")?;
        let symbol = repo::get_token(conn, &order.network, token_address)
            .await?
            .and_then(|t| t.symbol)
            .or_else(|| Some(short_address(token_address)));
        let pos = repo::get_position(conn, order.user_id, &order.network, token_address).await?;
        Ok(TradeReceipt {
            order,
            token_symbol: symbol,
            position_quantity: pos.as_ref().map(|p| p.quantity).unwrap_or(0.0),
            position_avg_price: pos.as_ref().map(|p| p.avg_price).unwrap_or(0.0),
            realized_pnl,
            alerts_fired: 0,
            tx_ref: "paper".to_string(),
            explorer_link: None,
            price_source,
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
