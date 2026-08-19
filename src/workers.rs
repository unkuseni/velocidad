//! Background workers:
//! - **limit-order matcher** — fills pending limit orders when the market
//!   price reaches the trigger, then notifies the user via Telegram.
//! - **alert poller** — checks untriggered price alerts against live quotes
//!   and notifies users when they fire.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};

use crate::app::AppState;
use crate::chains;
use crate::db::repo;
use crate::market;

/// Spawn both workers; they run until the process exits. Matching and
/// alerting work headless (API-only deployments); notifications are a
/// no-op without a Telegram token.
pub fn spawn(state: Arc<AppState>) {
    let token = state.config.teloxide_token.clone();
    let bot = if token.trim().is_empty() {
        tracing::warn!("TELOXIDE_TOKEN missing — notifications disabled, workers still run");
        None
    } else {
        Some(Bot::new(token))
    };
    tokio::spawn(limit_matcher(Arc::clone(&state), bot.clone()));
    tokio::spawn(alert_poller(Arc::clone(&state), bot.clone()));
    tokio::spawn(lp_watcher(state, bot));
    tracing::info!("background workers started (limits + alerts + lp watch)");
}

async fn limit_matcher(state: Arc<AppState>, bot: Option<Bot>) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Ok(mut hb) = state.worker_heartbeat.lock() {
            *hb = std::time::Instant::now();
        }
        match match_limits(&state, &bot).await {
            Ok(n) if n > 0 => tracing::info!(filled = n, "limit orders filled"),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "limit matcher error"),
        }
    }
}

async fn match_limits(state: &AppState, bot: &Option<Bot>) -> anyhow::Result<usize> {
    let orders = repo::list_pending_limits(state.db.conn()).await?;
    if orders.is_empty() {
        return Ok(0);
    }
    // One batched DexScreener request per chain instead of one per order.
    let items: Vec<(String, String)> = orders
        .iter()
        .map(|o| (o.network.clone(), o.token_address.clone()))
        .collect();
    let quotes = batch_quotes(state, &items).await;
    let mut filled = 0;
    for order in orders {
        let Some(chain) = chains::by_id(&order.network) else {
            tracing::warn!(network = %order.network, "unknown chain on limit order — skipping");
            continue;
        };
        let Some(quote) = quotes.get(&(order.network.clone(), order.token_address.to_lowercase()))
        else {
            tracing::warn!(network = %order.network, token = %order.token_address, "no quote for limit order token — skipping");
            continue;
        };
        let price = quote.price_native;
        let limit_price = order.price.unwrap_or(0.0);
        // Zero/unpriced/simulated quotes must never satisfy a trigger.
        if limit_price <= 0.0 || price <= 0.0 || price > limit_price {
            continue;
        }
        if quote.source == "simulator" {
            tracing::warn!(
                order = order.id,
                token = %order.token_address,
                "limit trigger on a simulated quote — skipping"
            );
            continue;
        }

        match state.engine.fill_limit(&state.db, chain, &order).await {
            Ok(receipt) => {
                filled += 1;
                notify(
                    bot,
                    state,
                    order.user_id,
                    format!(
                        "⏳ <b>LIMIT ORDER FILLED</b>\n\nToken: <code>{}</code> ({})\nBuy at: <b>{}</b> (filled {})\nQty: <b>{}</b>\nSpent: <b>{:.6} {}</b>\n{}\n\n{}",
                        market::short_addr(&order.token_address),
                        esc(&receipt.token_symbol.clone().unwrap_or_default()),
                        market::format_price(limit_price),
                        market::format_price(price),
                        market::format_qty(receipt.order.amount_out.unwrap_or(0.0)),
                        receipt.order.amount_in.unwrap_or(0.0),
                        chain.native,
                        market::format_usd(quote.price_usd * receipt.order.amount_out.unwrap_or(0.0)),
                        tx_footer(chain, &receipt.order.tx_hash.clone().unwrap_or_default()),
                    ),
                )
                .await;
            }
            Err(crate::trading::LimitFillError::Skip(reason)) => {
                tracing::debug!(order = order.id, reason = %reason, "limit fill deferred");
            }
            Err(crate::trading::LimitFillError::Fail(reason)) => {
                let _ = repo::fail_order(state.db.conn(), order.id, &reason).await;
                tracing::warn!(order = order.id, error = %reason, "limit fill failed");
                notify(
                    bot,
                    state,
                    order.user_id,
                    format!(
                        "❌ <b>LIMIT ORDER #{} FAILED</b>\n\nToken: <code>{}</code>\nReason: {}",
                        order.id,
                        market::short_addr(&order.token_address),
                        esc(&reason)
                    ),
                )
                .await;
            }
        }
    }
    Ok(filled)
}

async fn alert_poller(state: Arc<AppState>, bot: Option<Bot>) {
    let mut interval = tokio::time::interval(Duration::from_secs(20));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Ok(mut hb) = state.worker_heartbeat.lock() {
            *hb = std::time::Instant::now();
        }
        if let Err(e) = poll_alerts(&state, &bot).await {
            tracing::warn!(error = %e, "alert poller error");
        }
    }
}

async fn poll_alerts(state: &AppState, bot: &Option<Bot>) -> anyhow::Result<usize> {
    let alerts = repo::list_untriggered_alerts(state.db.conn()).await?;
    let mut fired = 0;
    // One batched DexScreener request per chain instead of one per alert.
    let items: Vec<(String, String)> = alerts
        .iter()
        .map(|a| (a.network.clone(), a.token_address.clone()))
        .collect();
    let quotes = batch_quotes(state, &items).await;
    for alert in alerts {
        let Some(quote) = quotes.get(&(alert.network.clone(), alert.token_address.to_lowercase()))
        else {
            continue;
        };
        let price = quote.price_native;
        // Zero-price and fabricated (simulator) quotes must never fire alerts.
        if price <= 0.0 || quote.source == "simulator" {
            continue;
        }
        let hit = match alert.condition.as_str() {
            "above" => price >= alert.target_price,
            "below" => price <= alert.target_price,
            _ => false,
        };
        if !hit {
            continue;
        }
        repo::mark_alert_triggered(state.db.conn(), alert.id).await?;
        fired += 1;
        notify(
            bot,
            state,
            alert.user_id,
            format!(
                "🔔 <b>PRICE ALERT</b>\n\nToken: <code>{}</code> ({})\nCondition: {} <b>{}</b>\nCurrent: <b>{}</b>\nSource: {}",
                market::short_addr(&alert.token_address),
                esc(&quote.symbol),
                if alert.condition == "above" { "↑ above" } else { "↓ below" },
                market::format_price(alert.target_price),
                market::format_price(price),
                quote.source,
            ),
        )
        .await;
    }
    Ok(fired)
}

/// Fetch quotes for a set of (network, token) pairs with one request per chain.
async fn batch_quotes(
    state: &AppState,
    items: &[(String, String)],
) -> HashMap<(String, String), crate::market::TokenQuote> {
    let mut by_chain: HashMap<String, Vec<String>> = HashMap::new();
    for (network, token) in items {
        let entry = by_chain.entry(network.clone()).or_default();
        let lower = token.to_lowercase();
        if !entry.contains(&lower) {
            entry.push(lower);
        }
    }
    let mut out = HashMap::new();
    for (network, tokens) in by_chain {
        let Some(chain) = chains::by_id(&network) else {
            continue;
        };
        for (token, quote) in state.market.quotes_batch(chain, &tokens).await {
            out.insert((network.clone(), token), quote);
        }
    }
    out
}

/// Watches for newly-created liquidity pools on user-watched chains and
/// notifies subscribers (deduped in memory). Opt-in via /lp watch <chain> <min_usd>.
async fn lp_watcher(state: Arc<AppState>, bot: Option<Bot>) {
    let mut interval = tokio::time::interval(Duration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // (user_id, chain, pair_address) seen before — bounded dedupe set.
    let mut seen: std::collections::HashSet<(i64, String, String)> =
        std::collections::HashSet::new();
    loop {
        interval.tick().await;
        let Ok(watchers) = repo::list_lp_watchers(state.db.conn()).await else {
            continue;
        };
        if watchers.is_empty() {
            continue;
        }
        // Group watchers by chain; remember the lowest threshold per chain.
        let mut by_chain: std::collections::HashMap<String, (f64, Vec<i64>)> =
            std::collections::HashMap::new();
        for (user_id, chain, min_usd) in &watchers {
            let e = by_chain
                .entry(chain.clone())
                .or_insert((f64::MAX, Vec::new()));
            e.0 = e.0.min(*min_usd);
            e.1.push(*user_id);
        }
        for (network, (min_liq, users)) in by_chain {
            let Some(chain) = chains::by_id(&network) else {
                continue;
            };
            let pairs = state.market.new_pairs_on(chain).await;
            for p in pairs {
                if p.liquidity_usd < min_liq {
                    continue;
                }
                for uid in &users {
                    let key = (*uid, network.clone(), p.pair_address.clone());
                    if !seen.insert(key.clone()) {
                        continue;
                    }
                    notify(
                        &bot,
                        &state,
                        *uid,
                        format!(
                            "🚀 <b>NEW POOL</b>: {} on {}\n\nToken: <code>{}</code>\nLiquidity: <b>{}</b> · 24h vol <b>{}</b>\n/scan {}:{}",
                            esc(&p.symbol),
                            chain.id,
                            market::short_addr(&p.token_address),
                            market::format_usd(p.liquidity_usd),
                            market::format_usd(p.volume_24h_usd),
                            chain.id,
                            p.token_address
                        ),
                    )
                    .await;
                }
            }
        }
        // Bound the dedupe set to the most recent 10k signals.
        if seen.len() > 10_000 {
            seen.clear();
        }
    }
}

/// Escape a string for Telegram HTML messages.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Send a Telegram notification to a user; failures are logged, not fatal.
/// A no-op when the bot token is not configured (headless mode).
async fn notify(bot: &Option<Bot>, state: &AppState, user_id: i64, text: String) {
    let Some(bot) = bot else {
        return;
    };
    let Ok(Some(user)) = repo::get_user_by_id(state.db.conn(), user_id).await else {
        return;
    };
    let res = bot
        .send_message(ChatId(user.telegram_id), text)
        .parse_mode(ParseMode::Html)
        .await;
    if let Err(e) = res {
        tracing::warn!(user = user_id, error = %e, "notification failed");
    }
}

fn tx_footer(chain: &crate::chains::Chain, tx: &str) -> String {
    format!("🔗 <a href=\"{}\">{}</a>", chain.explorer_link(tx), tx)
}
