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

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Query, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::app::AppState;
use crate::chains;
use crate::db::repo;
use crate::exec;

pub fn router(state: Arc<AppState>) -> Router {
    let api_key = state.config.api_key.clone();
    if api_key
        .as_ref()
        .map(|k| k.trim().is_empty())
        .unwrap_or(true)
    {
        tracing::warn!(
            "API_KEY is not set — the HTTP API is running OPEN. Set API_KEY to require \
             Authorization: Bearer <key> on /api/v1/* endpoints."
        );
    }
    let api = Router::new()
        .route("/api/v1/chains", get(chain_list))
        .route("/api/v1/users/:telegram_id", get(user_info))
        .route("/api/v1/portfolio/:telegram_id", get(portfolio))
        .route("/api/v1/balances/:telegram_id", get(balances))
        .route("/api/v1/trades/:telegram_id", get(trade_history))
        .route("/api/v1/trades", post(create_trade))
        .route("/api/v1/tokens/:address", get(token_info))
        .route("/api/v1/search", get(token_search))
        .route("/api/v1/sponsor/:telegram_id", get(sponsor_status))
        .route("/api/v1/boosts", get(token_boosts))
        .route("/api/v1/alerts/:telegram_id", get(alerts))
        .route("/api/v1/alerts", post(create_alert))
        // Everything under /api/v1 requires the bearer token when configured.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ))
        // Per-IP rate limit (outermost layer: checked before auth).
        .route_layer(middleware::from_fn_with_state(state.clone(), rate_limit))
        .with_state(state);

    // /health stays open for load balancers/probes.
    Router::new().route("/health", get(health)).merge(api)
}

/// Rejects requests without a matching `Authorization: Bearer <API_KEY>` when
/// `API_KEY` is configured. Open (dev) mode when unset.
async fn require_api_key(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let Some(expected) = state.config.api_key.as_ref() else {
        return next.run(req).await;
    };
    if expected.trim().is_empty() {
        return next.run(req).await;
    }
    let supplied = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|k| k.trim());
    // Constant-time comparison so key length can't leak through timing.
    let ok = match supplied {
        Some(k) => bool::from(expected.as_bytes().ct_eq(k.as_bytes())),
        None => false,
    };
    if ok {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [("WWW-Authenticate", "Bearer realm=\"velocidad\"")],
        Json(json!({ "error": "unauthorized — set Authorization: Bearer <API_KEY>" })),
    )
        .into_response()
}

/// Run the HTTP server until cancelled.
pub async fn serve(state: Arc<AppState>, port: u16) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port)).await?;
    tracing::info!(port, "HTTP API listening on http://0.0.0.0:{port}");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// 429s requests from an IP exceeding the configured per-minute limit.
async fn rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if let Err(retry) = state.api_rate.check(&format!("ip:{}", addr.ip())) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", retry.to_string())],
            Json(json!({ "error": format!("rate limit exceeded — retry in {retry}s") })),
        )
            .into_response();
    }
    next.run(req).await
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

async fn user_info(State(state): State<Arc<AppState>>, Path(telegram_id): Path<i64>) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let wallets = repo::list_wallets(state.db.conn(), user.id)
                .await
                .unwrap_or_default();
            let orders = repo::list_orders(state.db.conn(), user.id, 100)
                .await
                .unwrap_or_default();
            let positions = repo::list_positions(state.db.conn(), user.id)
                .await
                .unwrap_or_default();
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

async fn portfolio(State(state): State<Arc<AppState>>, Path(telegram_id): Path<i64>) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let positions = match repo::list_positions(state.db.conn(), user.id).await {
                Ok(p) => p,
                Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
            };
            let mut items = Vec::new();
            let mut totals: std::collections::BTreeMap<String, f64> = Default::default();
            let mut total_usd = 0.0;
            for p in &positions {
                let chain =
                    chains::by_id(&p.network).unwrap_or_else(|| chains::by_id("ethereum").unwrap());
                let quote = state.market.quote(chain, &p.token_address).await;
                let price = quote.price_native;
                let value = p.quantity * price;
                let unrealized = (price - p.avg_price) * p.quantity;
                let native_usd = state.market.native_price_usd(chain).await;
                let value_usd = value * native_usd;
                total_usd += value_usd;
                *totals.entry(p.network.clone()).or_default() += value_usd;
                items.push(json!({
                    "token_address": p.token_address,
                    "network": p.network,
                    "quantity": p.quantity,
                    "avg_price": p.avg_price,
                    "current_price": price,
                    "price_usd": quote.price_usd,
                    "value_native": value,
                    "value_usd": value_usd,
                    "unrealized_pnl": unrealized,
                    "realized_pnl": p.realized_pnl,
                }));
            }
            ok(json!({
                "telegram_id": telegram_id,
                "positions": items,
                "totals_usd": totals,
                "total_value_usd": total_usd,
            }))
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
                None => chains::by_id(&state.config.default_chain)
                    .unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
            };
            let wallet = match repo::get_default_wallet_for(state.db.conn(), user.id, chain.id)
                .await
            {
                Ok(Some(w)) => w,
                Ok(None) => match repo::get_default_wallet(state.db.conn(), user.id).await {
                    Ok(Some(w)) => w,
                    _ => {
                        return ok(
                            json!({ "telegram_id": telegram_id, "chain": chain.id, "balances": [] }),
                        )
                    }
                },
                Err(e) => return fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
            };
            let native_usd = state.market.native_price_usd(chain).await;
            let mut tokens: Vec<serde_json::Value> = Vec::new();
            let (native_balance, native_divisor) = match chain.kind {
                chains::ChainKind::Evm => (
                    state
                        .rpc
                        .native_balance(chain, &wallet.address)
                        .await
                        .unwrap_or(0),
                    1e18,
                ),
                chains::ChainKind::Solana => (
                    state
                        .solana
                        .balance(chain, &wallet.address)
                        .await
                        .unwrap_or(0) as u128,
                    1e9,
                ),
            };
            if chain.kind == chains::ChainKind::Solana {
                if let Ok(balances) = state.solana.spl_balances(chain, &wallet.address).await {
                    let mut items: Vec<(String, f64, f64)> = Vec::new();
                    for b in balances.iter().take(8) {
                        let q = state.market.quote(chain, &b.mint).await;
                        if q.price_usd > 0.0 {
                            items.push((q.symbol, b.amount, q.price_usd * b.amount));
                        }
                    }
                    items
                        .sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
                    for (symbol, amount, usd) in items.iter().take(6) {
                        tokens
                            .push(json!({ "symbol": symbol, "balance": amount, "usd_value": usd }));
                    }
                }
            } else {
                for (symbol, addr) in chain.default_erc20s {
                    if let Ok(raw) = state.rpc.erc20_balance(chain, addr, &wallet.address).await {
                        let decimals = state.rpc.erc20_decimals(chain, addr).await;
                        let bal = raw as f64 / 10f64.powi(decimals as i32);
                        if bal > 0.0 {
                            tokens
                                .push(json!({ "symbol": symbol, "address": addr, "balance": bal }));
                        }
                    }
                }
            }
            ok(json!({
                "telegram_id": telegram_id,
                "chain": chain.id,
                "kind": match chain.kind { chains::ChainKind::Evm => "evm", chains::ChainKind::Solana => "solana" },
                "wallet": wallet.address,
                "native": { "symbol": chain.native, "balance": native_balance as f64 / native_divisor, "usd_value": native_balance as f64 / native_divisor * native_usd },
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
        let chain = match req.chain.as_deref().and_then(chains::Chain::resolve) {
            Some(c) => c,
            None => match repo::user_chain(state.db.conn(), user.id, &state.config.default_chain).await {
                Ok(id) => chains::by_id(&id).unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
                Err(_) => chains::by_id("ethereum").unwrap(),
            },
        };

        // Live mode needs a wallet; paper mode keeps working without one.
        let wallet = match exec::wallet_for_chain(&state, user.id, chain).await {
            Ok(w) => Some(w),
            Err(e) if state.config.paper_trading => {
                tracing::debug!(err = %e, "no wallet — falling back to paper engine fill");
                None
            }
            Err(e) => return Err(e),
        };

        match req.side.as_str() {
            "buy" | "snipe" => {
                if req.side == "snipe" {
                    let report = state.scanner.scan(&state.db, chain, &req.token_address).await;
                    if report.is_honeypot {
                        anyhow::bail!("token is flagged as a honeypot — snipe blocked");
                    }
                }
                match wallet {
                    Some(w) => {
                        let outcome = exec::buy(&state, chain, user.id, &w, &req.token_address, req.amount, req.slippage, &req.side).await?;
                        Ok(json!({ "status": "filled", "chain": chain.id, "outcome": serde_json::to_value(&outcome)? }))
                    }
                    None => {
                        let r = state
                            .engine
                            .buy(&state.db, chain, user.id, None, &req.token_address, req.amount, req.slippage, &req.side)
                            .await?;
                        Ok(json!({ "status": "filled", "mode": "paper", "chain": chain.id, "receipt": serde_json::to_value(&r)? }))
                    }
                }
            }
            "sell" => match wallet {
                Some(w) => {
                    let outcome = exec::sell(&state, chain, user.id, &w, &req.token_address, req.amount, req.slippage).await?;
                    Ok(json!({ "status": "filled", "chain": chain.id, "outcome": serde_json::to_value(&outcome)? }))
                }
                None => {
                    let r = state
                        .engine
                        .sell(&state.db, chain, user.id, None, &req.token_address, req.amount, req.slippage)
                        .await?;
                    Ok(json!({ "status": "filled", "mode": "paper", "chain": chain.id, "receipt": serde_json::to_value(&r)? }))
                }
            },
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

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Debug, Deserialize)]
struct SponsorQuery {
    chain: Option<String>,
}

async fn sponsor_status(
    State(state): State<Arc<AppState>>,
    Path(telegram_id): Path<i64>,
    Query(params): Query<SponsorQuery>,
) -> ApiResult {
    match repo::get_user(state.db.conn(), telegram_id).await {
        Ok(Some(user)) => {
            let chain = match params.chain.as_deref().and_then(chains::Chain::resolve) {
                Some(c) => c,
                None => {
                    match repo::user_chain(state.db.conn(), user.id, &state.config.default_chain)
                        .await
                    {
                        Ok(id) => {
                            chains::by_id(&id).unwrap_or_else(|| chains::by_id("ethereum").unwrap())
                        }
                        Err(_) => chains::by_id("ethereum").unwrap(),
                    }
                }
            };
            let wallet = repo::get_default_wallet_for(state.db.conn(), user.id, chain.id)
                .await
                .unwrap_or(None);
            let opt_in =
                repo::get_setting(state.db.conn(), user.id, &format!("sponsor:{}", chain.id))
                    .await
                    .unwrap_or(None);
            let mut status = json!({
                "telegram_id": telegram_id,
                "chain": chain.id,
                "opt_in": opt_in == Some("on".to_string()),
                "sponsor_configured": false,
                "wallet": wallet.as_ref().map(|w| w.address.clone()).unwrap_or_default(),
            });
            match chain.kind {
                chains::ChainKind::Evm => {
                    if let Some((addr, _)) = state.swap.sponsor.as_ref() {
                        status["sponsor_configured"] = json!(true);
                        status["sponsor"] = json!(addr);
                    }
                    if let Some(w) = &wallet {
                        status["delegation"] = json!(state
                            .rpc
                            .delegation_of(chain, &w.address)
                            .await
                            .unwrap_or(None));
                    }
                }
                chains::ChainKind::Solana => {
                    if let Some(sp) = state.solana.sponsor.as_ref() {
                        status["sponsor_configured"] = json!(true);
                        status["sponsor"] = json!(sp.address());
                    }
                }
            }
            ok(status)
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("user {telegram_id} not found") })),
        ),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn token_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> ApiResult {
    ok(json!({ "query": params.q, "results": state.market.search(&params.q).await }))
}

async fn token_boosts(State(state): State<Arc<AppState>>) -> ApiResult {
    ok(json!({ "boosts": state.market.boosts().await }))
}

async fn token_info(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<TokenQuery>,
) -> ApiResult {
    let chain = match params.chain.as_deref().and_then(chains::Chain::resolve) {
        Some(c) => c,
        None => chains::by_id(&state.config.default_chain)
            .unwrap_or_else(|| chains::by_id("ethereum").unwrap()),
    };
    let quote = state.market.quote(chain, &address).await;
    let report = state.scanner.scan(&state.db, chain, &address).await;
    let cached = repo::get_token(state.db.conn(), chain.id, &address)
        .await
        .unwrap_or(None);
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

async fn alerts(State(state): State<Arc<AppState>>, Path(telegram_id): Path<i64>) -> ApiResult {
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
            None => match repo::user_chain(state.db.conn(), user.id, &state.config.default_chain)
                .await
            {
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
