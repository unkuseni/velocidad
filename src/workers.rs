//! Background workers:
//! - **limit-order matcher** — fills pending limit orders when the market
//!   price reaches the trigger, then notifies the user via Telegram.
//! - **alert poller** — checks untriggered price alerts against live quotes
//!   and notifies users when they fire.

use std::sync::Arc;
use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{ChatId, ParseMode};

use crate::app::AppState;
use crate::chains;
use crate::db::repo;
use crate::market;

/// Spawn both workers; they run until the process exits.
pub fn spawn(state: Arc<AppState>) {
    let token = state.config.teloxide_token.clone();
    if token.trim().is_empty() {
        tracing::warn!("TELOXIDE_TOKEN missing — notifications disabled");
        return;
    }
    let bot = Bot::new(token);
    tokio::spawn(limit_matcher(Arc::clone(&state), bot.clone()));
    tokio::spawn(alert_poller(state, bot));
    tracing::info!("background workers started (limits + alerts)");
}

async fn limit_matcher(state: Arc<AppState>, bot: Bot) {
    let mut interval = tokio::time::interval(Duration::from_secs(15));
    loop {
        interval.tick().await;
        match match_limits(&state, &bot).await {
            Ok(n) if n > 0 => tracing::info!(filled = n, "limit orders filled"),
            Ok(_) => {},
            Err(e) => tracing::warn!(error = %e, "limit matcher error"),
        }
    }
}

async fn match_limits(state: &AppState, bot: &Bot) -> anyhow::Result<usize> {
    let orders = repo::list_pending_limits(state.db.conn()).await?;
    if orders.is_empty() {
        return Ok(0);
    }
    let mut filled = 0;
    for order in orders {
        let Some(chain) = chains::by_id(&order.network) else {
            tracing::warn!(network = %order.network, "unknown chain on limit order — skipping");
            continue;
        };
        let quote = state.market.quote(chain, &order.token_address).await;
        let price = quote.price_native;
        let limit_price = order.price.unwrap_or(0.0);
        if limit_price <= 0.0 || price > limit_price {
            continue;
        }

        match state.engine.fill_limit(&state.db, chain, &order).await {
            Ok(receipt) => {
                filled += 1;
                notify(
                    &bot,
                    &state,
                    order.user_id,
                    format!(
                        "⏳ <b>LIMIT ORDER FILLED</b>\n\nToken: <code>{}</code> ({})\nBuy at: <b>{}</b> (filled {})\nQty: <b>{}</b>\nSpent: <b>{:.6} {}</b>\n{}\n\n{}",
                        market::short_addr(&order.token_address),
                        receipt.token_symbol.clone().unwrap_or_default(),
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
            Err(e) => {
                let _ = repo::fail_order(state.db.conn(), order.id, &e.to_string()).await;
                tracing::warn!(order = order.id, error = %e, "limit fill failed");
            }
        }
    }
    Ok(filled)
}

async fn alert_poller(state: Arc<AppState>, bot: Bot) {
    let mut interval = tokio::time::interval(Duration::from_secs(20));
    loop {
        interval.tick().await;
        if let Err(e) = poll_alerts(&state, &bot).await {
            tracing::warn!(error = %e, "alert poller error");
        }
    }
}

async fn poll_alerts(state: &AppState, bot: &Bot) -> anyhow::Result<usize> {
    let alerts = repo::list_untriggered_alerts(state.db.conn()).await?;
    let mut fired = 0;
    for alert in alerts {
        let Some(chain) = chains::by_id(&alert.network) else {
            continue;
        };
        let quote = state.market.quote(chain, &alert.token_address).await;
        let price = quote.price_native;
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
                quote.symbol,
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

/// Send a Telegram notification to a user; failures are logged, not fatal.
async fn notify(bot: &Bot, state: &AppState, user_id: i64, text: String) {
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
