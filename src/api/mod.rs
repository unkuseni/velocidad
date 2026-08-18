//! HTTP API (Axum) — mirrors the bot's trading surface for web dashboards.
//!
//! Routes:
//! - `GET  /health`
//! - `GET  /api/v1/chains`                     — supported EVM chains
//! - `GET  /api/v1/users/:telegram_id`          — user + wallets + stats
//! - `GET  /api/v1/portfolio/:telegram_id`      — open positions with live PnL
//! - `GET  /api/v1/balances/:telegram_id`       — on-chain balances
//! - `GET  /api/v1/trades/:telegram_id`         — order history
//! - `POST /api/v1/trades`                      — execute a trade (chain-aware)
//! - `GET  /api/v1/tokens/:address?chain=bsc`   — price + security report
//! - `GET  /api/v1/alerts/:telegram_id`         — user alerts
//! - `POST /api/v1/alerts`                      — create an alert

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::app::AppState;
use crate::chains;
use crate::db::repo;

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/chains", get(chain_list))
        .route("/api/v1/users/:telegram_id", get(user_info))
        .route("/api/v1/portfolio/:telegram_id", get(portfolio))
        .route("/api/v1/balances/:telegram_id", get(balances))
        .route("/api/v1/trades/:telegram_id", get(trade_history))
        .route("/api/v1/trades", post(create_trade))
        .route("/api/v1/tokens/:address", get(token_info))
        .route("/api/v1/alerts/:telegram_id", get(alerts))
        .route("/api/v1/alerts", post(create_alert))
        .with_state(state)
}

/// Run the HTTP server until cancelled.
pub async fn serve(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP API listening on http://0.0.0.0:{port}");
    axum::serve(listener, app).await?;
    Ok(())
}

type ApiResult = (StatusCode, Json<serde_json::Value>);

fn ok(v: serde_json::Value) -> ApiResult {
    (StatusCode::OK, Json(v))
}

fn fail(code: StatusCode, err: &anyhow::Error) -> ApiResult {
    (code, Json(json!({ "error": err.to_string() })))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "velocidad",
        "time": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn chain_list() -> Json<serde_json::Value> {
    let chains: Vec<_> = chains::CHAINS
        .iter()
        .map(|c| {
            json!({
                "id": c.id,
                "chain_id": c.chain_id,
                "name": c.name,
                "native": c.native,
                "explorer": c.explorer,
            })
        })
        .collect();
    Json(json!({ "chains": chains }))
}

async fn user_info(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let wallets = repo::list_wallets(state.db.conn(), user.id).await.unwrap_or_default();
            let orders = repo::list_orders(state.db.conn(), user.id, 100).await.unwrap_or_default();
            let positions = repo::list_positions(state.db.conn(), user.id).await.unwrap_or_default();
            ok(json!({
                "user": user,
                "wallets": wallets,
                "trade_count": orders.len(),
                "open_positions": positions.len(),
            }))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn portfolio(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let positions = match repo::list_positions(state.db.conn(), user.id).await {
                Ok(p) => p,
                Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
            };
            let mut items = Vec::new();
            for p in &positions {
                let chain = chains::by_id(&p.network).unwrap_or_else(|| chains::by_id("ethereum").unwrap());
                let quote = state.market.quote(chain, &p.token_address).await;
                let price = quote.price_native;
                let value = p.quantity * price;
                let unrealized = (price - p.avg_price) * p.quantity;
                items.push(json!({
                    "token_address": p.token_address,
                    "network": p.network,
                    "quantity": p.quantity,
                    "avg_price": p.avg_price,
                    "current_price": price,
                    "price_usd": quote.price_usd,
                    "value_native": value,
                    "value_usd": value * state.market.native_price_usd(chain).await,
                    "unrealized_pnl": unrealized,
                    "realized_pnl": p.realized_pnl,
                }));
            }
            ok(json!({ "telegram_id": telegram_id, "positions": items }))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn balances(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
    Query(params): Query<BalanceQuery>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let chain = match params.chain.as_deref().and_then(chains::Chain::resolve) {
                Some(c) => c,
                None => chains::by_id(&state.config.default_chain).unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
            };
            let wallet = match repo::get_default_wallet(state.db.conn(), user.id).await {
                Ok(Some(w)) => w,
                Ok(None) => return ok(json!({ "telegram_id": telegram_id, "chain": chain.id, "balances": [] })),
                Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
            };
            let native_usd = state.market.native_price_usd(chain).await;
            let native = state.rpc.native_balance(chain, &wallet.address).await.unwrap_or(0);
            let mut tokens = Vec::new();
            for (symbol, addr) in chain.default_erc20s {
                if let Ok(raw) = state.rpc.erc20_balance(chain, addr, &wallet.address).await {
                    let decimals = state.rpc.erc20_decimals(chain, addr).await;
                    let bal = raw as f64 / 10f64.powi(decimals as i32);
                    if bal > 0.0 {
                        tokens.push(json!({ "symbol": symbol, "address": addr, "balance": bal }));
                    }
                }
            }
            ok(json!({
                "telegram_id": telegram_id,
                "chain": chain.id,
                "wallet": wallet.address,
                "native": { "symbol": chain.native, "balance": native as f64 / 1e18, "usd_value": native as f64 / 1e18 * native_usd },
                "tokens": tokens,
            }))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

#[derive(Debug, Deserialize)]
struct BalanceQuery {
    chain: Option<String>,
}

async fn trade_history(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => match repo::list_orders(state.db.conn(), user.id, 50).await {
            Ok(orders) => ok(json!({ "telegram_id": telegram_id, "orders": orders })),
            Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

#[derive(Debug, Deserialize)]
struct TradeRequest {
    telegram_id: i64,
    token_address: String,
    #[serde(rename = "side")]
    side: String,
    amount: f64,
    #[serde(default = "default_slippage")]
    slippage: f64,
    /// Optional chain override (default: user's chain setting).
    chain: Option<String>,
}

fn default_slippage() -> f64 {
    0.05
}

async fn create_trade(
    State(state): State<Arc<AppState>>,
    Json(req): Json<TradeRequest>,
) -> ApiResult {
    let result = async {
        let user = repo::get_or_create_user(state.db.conn(), req.telegram_id, None, None).await?;
        let wallet = repo::get_default_wallet(state.db.conn(), user.id).await?;
        let wallet_id = wallet.map(|w| w.id);
        let chain = match req.chain.as_deref().and_then(chains::Chain::resolve) {
            Some(c) => c,
            None => match repo::user_chain(state.db.conn(), user.id, &state.config.default_chain).await {
                Ok(id) => chains::by_id(&id).unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
                Err(_) => chains::by_id("ethereum").unwrap(),
            },
        };

        match req.side.as_str() {
            "buy" | "snipe" => {
                let r = state
                    .engine
                    .buy(
                        &state.db,
                        chain,
                        user.id,
                        wallet_id,
                        &req.token_address,
                        req.amount,
                        req.slippage,
                        &req.side,
                    )
                    .await?;
                Ok(json!({ "status": "filled", "mode": "paper", "chain": chain.id, "receipt": serde_json::to_value(&r)? }))
            }
            "sell" => {
                let r = state
                    .engine
                    .sell(
                        &state.db,
                        chain,
                        user.id,
                        wallet_id,
                        &req.token_address,
                        req.amount,
                        req.slippage,
                    )
                    .await?;
                Ok(json!({ "status": "filled", "mode": "paper", "chain": chain.id, "receipt": serde_json::to_value(&r)? }))
            }
            other => Err(anyhow::anyhow!("unknown side '{other}' (expected buy|sell|snipe)")),
        }
    }
    .await;

    match result {
        Ok(v) => ok(v),
        Err(e) => fail(StatusCode::BAD_REQUEST, &e),
    }
}

#[derive(Debug, Deserialize)]
struct TokenQuery {
    chain: Option<String>,
}

async fn token_info(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<TokenQuery>,
) -> ApiResult {
    let chain = match params.chain.as_deref().and_then(chains::Chain::resolve) {
        Some(c) => c,
        None => chains::by_id(&state.config.default_chain).unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
    };
    let quote = state.market.quote(chain, &address).await;
    let report = state.scanner.scan(&state.db, chain, &address).await;
    let cached = repo::get_token(state.db.conn(), &address).await.unwrap_or(None);
    ok(json!({
        "address": address,
        "chain": chain.id,
        "quote": quote,
        "price_usd": quote.price_usd,
        "price_native": quote.price_native,
        "report": report,
        "cached_token": cached,
    }))
}

async fn alerts(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => match repo::list_alerts(state.db.conn(), user.id).await {
            Ok(items) => ok(json!({ "telegram_id": telegram_id, "alerts": items })),
            Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

#[derive(Debug, Deserialize)]
struct AlertRequest {
    telegram_id: i64,
    token_address: String,
    condition: String,
    target_price: f64,
    chain: Option<String>,
}

async fn create_alert(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AlertRequest>,
) -> ApiResult {
    let result = async {
        let user = repo::get_or_create_user(state.db.conn(), req.telegram_id, None, None).await?;
        let cond = req.condition.to_lowercase();
        if cond != "above" && cond != "below" {
            anyhow::bail!("condition must be 'above' or 'below'");
        }
        let network = match req.chain.as_deref().and_then(chains::Chain::resolve) {
            Some(c) => c.id.to_string(),
            None => match repo::user_chain(state.db.conn(), user.id, &state.config.default_chain).await {
                Ok(id) => id,
                Err(_) => "ethereum".to_string(),
            },
        };
        let alert = repo::insert_alert(
            state.db.conn(),
            user.id,
            &network,
            &req.token_address,
            &cond,
            req.target_price,
        )
        .await?;
        Ok(json!({ "alert": alert }))
    }
    .await;

    match result {
        Ok(v) => ok(v),
        Err(e) => fail(StatusCode::BAD_REQUEST, &e),
    }
}
