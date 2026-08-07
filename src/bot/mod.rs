//! Telegram bot — Trojan-style command surface.
//!
//! Commands are defined once in [`Command`] (teloxide `BotCommands` derive)
//! and dispatched to the trading engine + libSQL store. All replies are HTML.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use teloxide::dispatching::{Dispatcher, HandlerExt, UpdateFilterExt};
use teloxide::prelude::*;
use teloxide::types::{Message, ParseMode, Update};
use teloxide::utils::command::BotCommands;

use crate::app::AppState;
use crate::crypto;
use crate::db::repo;
use crate::security::TokenReport;
use crate::trading::MarketSimulator;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Velocidad trading bot commands", parse_with = "split")]
pub enum Command {
    #[command(description = "start the bot")]
    Start,
    #[command(description = "show this help")]
    Help,
    #[command(description = "manage wallets: /wallet, /wallet new, /wallet import <privkey>")]
    Wallet,
    #[command(description = "instant buy: /buy <token> <amount_eth>")]
    Buy(String, f64),
    #[command(description = "sell: /sell <token> <quantity>")]
    Sell(String, f64),
    #[command(description = "snipe a launch: /snipe <token> <amount_eth>")]
    Snipe(String, f64),
    #[command(description = "limit order: /limit <token> <price> <amount_eth>")]
    Limit(String, f64, f64),
    #[command(description = "current price: /price <token>")]
    Price(String),
    #[command(description = "security scan: /scan <token>")]
    Scan(String),
    #[command(description = "price alert: /alert <token> <above|below> <price>")]
    Alert(String, String, f64),
    #[command(description = "list your alerts")]
    Alerts,
    #[command(description = "your open positions")]
    Portfolio,
    #[command(description = "recent order history")]
    History,
    #[command(description = "trading settings: /settings, /settings slippage 0.05")]
    Settings,
}

/// Start polling Telegram and handle commands until Ctrl+C.
pub async fn run(state: Arc<AppState>) -> Result<()> {
    let token = state.config.teloxide_token.clone();
    if token.trim().is_empty() {
        bail!(
            "TELOXIDE_TOKEN is not set — create a bot with @BotFather and \
             add its token to .env (see .env.example)"
        );
    }
    let bot = Bot::new(token);

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(dispatch_command),
        )
        .branch(dptree::endpoint(unknown_message));

    tracing::info!("telegram bot polling started");
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

async fn dispatch_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    state: Arc<AppState>,
) -> ResponseResult<()> {
    let reply = match cmd {
        Command::Start => cmd_start(&state, &msg).await,
        Command::Help => Ok(help_text().to_string()),
        Command::Wallet => {
            let arg = rest_of_command(&msg, "/wallet");
            cmd_wallet(&state, &msg, arg).await
        }
        Command::Buy(token, amount) => cmd_buy(&state, &msg, token, amount, "buy").await,
        Command::Sell(token, qty) => cmd_sell(&state, &msg, token, qty).await,
        Command::Snipe(token, amount) => cmd_buy(&state, &msg, token, amount, "snipe").await,
        Command::Limit(token, price, amount) => cmd_limit(&state, &msg, token, price, amount).await,
        Command::Price(token) => cmd_price(&state, &msg, token).await,
        Command::Scan(token) => cmd_scan(&state, &msg, token).await,
        Command::Alert(token, cond, price) => cmd_alert(&state, &msg, token, cond, price).await,
        Command::Alerts => cmd_alerts(&state, &msg).await,
        Command::Portfolio => cmd_portfolio(&state, &msg).await,
        Command::History => cmd_history(&state, &msg).await,
        Command::Settings => {
            let arg = rest_of_command(&msg, "/settings");
            cmd_settings(&state, &msg, arg).await
        }
    };

    match reply {
        Ok(text) => {
            let _ = bot
                .send_message(msg.chat.id, text)
                .parse_mode(ParseMode::Html)
                .await;
        }
        Err(err) => {
            tracing::warn!(error = %err, "command failed");
            let _ = bot.send_message(msg.chat.id, format!("❌ {err}")).await;
        }
    }
    Ok(())
}

/// Everything after the command name in a message, e.g. `"/wallet new 0x…"` → `"new 0x…"`.
fn rest_of_command(msg: &Message, command: &str) -> Option<String> {
    msg.text().and_then(|t| {
        t.trim_start()
            .strip_prefix(command)
            .map(|rest| rest.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

async fn unknown_message(bot: Bot, msg: Message, _state: Arc<AppState>) -> ResponseResult<()> {
    bot.send_message(msg.chat.id, help_text())
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn cmd_start(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let wallet = repo::get_default_wallet(state.db.conn(), user_id).await?;
    let addr = wallet
        .map(|w| w.address.clone())
        .unwrap_or_else(|| "none — create one with /wallet new".to_string());
    Ok(format!(
        "⚡ <b>Velocidad</b> — Trojan-style trading bot\n\
         💾 Database: libSQL{} ({})\n\n\
         👤 User: <code>{}</code>\n\
         👛 Wallet: <code>{}</code>\n\n\
         <b>Commands</b>\n{}",
        if state.config.is_remote() { " · Turso" } else { "" },
        state.config.database_url,
        user_id,
        addr,
        help_text()
    ))
}

async fn cmd_wallet(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    match arg.as_deref() {
        None => {
            let wallets = repo::list_wallets(state.db.conn(), user_id).await?;
            if wallets.is_empty() {
                return Ok(
                    "👛 No wallets yet.\nCreate one: <b>/wallet new</b>\nImport one: <b>/wallet import &lt;privkey&gt;</b>"
                        .to_string(),
                );
            }
            let mut s = String::from("👛 <b>Wallets</b>\n");
            for w in &wallets {
                let badge = if w.is_default { "⭐ " } else { "   " };
                s.push_str(&format!(
                    "{}{} <code>{}</code> ({})\n",
                    badge, w.label, w.address, w.network
                ));
            }
            Ok(s)
        }
        Some("new") => {
            let gw = crypto::generate_wallet()?;
            let enc = state.keyring.encrypt(gw.private_key_hex.as_bytes())?;
            repo::insert_wallet(state.db.conn(), user_id, &gw.address, "Main", Some(&enc), true)
                .await?;
            Ok(format!(
                "✅ <b>Wallet created</b>\n\n\
                 📮 Address: <code>{}</code>\n\
                 🔑 Private key (shown once): <code>{}</code>\n\n\
                 ⚠️ Store it safely — it is encrypted at rest with your master key.",
                gw.address, gw.private_key_hex
            ))
        }
        Some(rest) if rest.starts_with("import") => {
            let key = rest.trim_start_matches("import").trim();
            let gw = crypto::import_wallet(key).context("invalid private key — expected 64 hex chars")?;
            let enc = state.keyring.encrypt(gw.private_key_hex.as_bytes())?;
            repo::insert_wallet(state.db.conn(), user_id, &gw.address, "Imported", Some(&enc), true)
                .await?;
            Ok(format!("✅ Imported wallet <code>{}</code>", gw.address))
        }
        Some(other) => Ok(format!(
            "Unknown wallet action <code>{}</code>.\nUse /wallet, /wallet new or /wallet import &lt;privkey&gt;.",
            other
        )),
    }
}

async fn cmd_buy(
    state: &AppState,
    msg: &Message,
    token: String,
    amount: f64,
    side: &str,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let wallet = repo::get_default_wallet(state.db.conn(), user_id)
        .await?
        .context("create a wallet first with /wallet new")?;

    if side == "snipe" {
        let price = state.engine.sim.price(&token);
        let report = state.scanner.scan(&state.db, &token, price).await;
        if report.is_honeypot {
            bail!("🚫 <code>{}</code> is flagged as a honeypot — snipe blocked", short(&token));
        }
    }

    let receipt = state
        .engine
        .buy(
            &state.db,
            user_id,
            Some(wallet.id),
            &token,
            amount,
            user_slippage(state, user_id).await?,
            side,
        )
        .await?;
    let o = &receipt.order;
    let emoji = if side == "snipe" { "🛰️" } else { "✅" };
    Ok(format!(
        "{emoji} <b>{}</b> filled (paper)\n\n\
         Token: <code>{}</code> ({})\n\
         Qty: <b>{:.6}</b>\n\
         Price: <b>{}</b>\n\
         Spent: <b>{:.4} ETH</b>\n\
         Position: {:.6} @ {}\n\
         Slippage: {:.2}%{}\n{}",
        side.to_uppercase(),
        short(&token),
        receipt.token_symbol.clone().unwrap_or_default(),
        o.amount_out.unwrap_or(0.0),
        MarketSimulator::format_price(o.price.unwrap_or(0.0)),
        o.amount_in.unwrap_or(0.0),
        receipt.position_quantity,
        MarketSimulator::format_price(receipt.position_avg_price),
        o.slippage * 100.0,
        alert_note(receipt.alerts_fired),
        tx_footer("paper")
    ))
}

async fn cmd_sell(state: &AppState, msg: &Message, token: String, qty: f64) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let wallet = repo::get_default_wallet(state.db.conn(), user_id).await?;
    let receipt = state
        .engine
        .sell(
            &state.db,
            user_id,
            wallet.as_ref().map(|w| w.id),
            &token,
            qty,
            user_slippage(state, user_id).await?,
        )
        .await?;
    let o = &receipt.order;
    Ok(format!(
        "💸 <b>SELL</b> filled (paper)\n\n\
         Token: <code>{}</code> ({})\n\
         Qty: <b>{:.6}</b>\n\
         Price: <b>{}</b>\n\
         Proceeds: <b>{:.4} ETH</b>\n\
         Realized PnL: <b>{}</b>\n{}",
        short(&token),
        receipt.token_symbol.clone().unwrap_or_default(),
        o.amount_in.unwrap_or(0.0),
        MarketSimulator::format_price(o.price.unwrap_or(0.0)),
        o.amount_out.unwrap_or(0.0),
        pnl_str(receipt.realized_pnl.unwrap_or(0.0)),
        tx_footer("paper")
    ))
}

async fn cmd_limit(
    state: &AppState,
    msg: &Message,
    token: String,
    price: f64,
    amount: f64,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let wallet = repo::get_default_wallet(state.db.conn(), user_id).await?;
    let order = state
        .engine
        .place_limit(
            &state.db,
            user_id,
            wallet.as_ref().map(|w| w.id),
            &token,
            price,
            amount,
        )
        .await?;
    Ok(format!(
        "⏳ <b>LIMIT order placed</b>\n\n\
         Token: <code>{}</code>\n\
         Buy at: <b>{}</b>\n\
         Amount: <b>{:.4} ETH</b>\n\
         Order #<code>{}</code> (status: pending)\n\
         \nℹ️ Paper trading fills market orders instantly; limit fills require a real matching engine.",
        short(&token),
        MarketSimulator::format_price(price),
        amount,
        order.id
    ))
}

async fn cmd_price(state: &AppState, msg: &Message, token: String) -> Result<String> {
    let _user_id = ensure_user(state, msg).await?;
    let price = state.engine.sim.price(&token);
    let report = state.scanner.scan(&state.db, &token, price).await;
    Ok(format!(
        "📈 <b>{}</b> <code>{}</code>\n\n\
         Price: <b>{}</b>\n\
         Liquidity: {:.2} ETH\n\
         Risk: <b>{}/100</b> {}\n\n{}",
        report.name,
        short(&token),
        MarketSimulator::format_price(price),
        report.liquidity_eth,
        report.risk_score,
        risk_badge(report.risk_score),
        checks_text(&report)
    ))
}

async fn cmd_scan(state: &AppState, msg: &Message, token: String) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let price = state.engine.sim.price(&token);
    let report = state.scanner.scan(&state.db, &token, price).await;
    Ok(format!(
        "🔬 <b>Security scan</b>\n\n\
         Token: <code>{}</code>\n\
         Name/Symbol: {} / {}\n\
         Risk score: <b>{}/100</b> {}\n\
         Verdict: <b>{}</b>\n\n{}",
        short(&token),
        report.name,
        report.symbol,
        report.risk_score,
        risk_badge(report.risk_score),
        if report.is_honeypot { "🚫 HONEYPOT" } else if report.risk_score >= 70 { "⚠️ HIGH RISK" } else { "✅ tradable" },
        checks_text(&report)
    ))
}

async fn cmd_alert(
    state: &AppState,
    msg: &Message,
    token: String,
    cond: String,
    price: f64,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let cond = cond.to_lowercase();
    if cond != "above" && cond != "below" {
        bail!("condition must be <code>above</code> or <code>below</code>");
    }
    let alert = repo::insert_alert(state.db.conn(), user_id, &token, &cond, price).await?;
    Ok(format!(
        "🔔 <b>Alert created</b> #<code>{}</code>\n\n\
         Token: <code>{}</code>\n\
         When price goes {cond} <b>{}</b>\n\
         Current: <b>{}</b>",
        alert.id,
        short(&token),
        MarketSimulator::format_price(price),
        MarketSimulator::format_price(state.engine.sim.price(&token))
    ))
}

async fn cmd_alerts(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let alerts = repo::list_alerts(state.db.conn(), user_id).await?;
    if alerts.is_empty() {
        return Ok("🔔 No alerts yet. Create one with /alert &lt;token&gt; &lt;above|below&gt; &lt;price&gt;.".to_string());
    }
    let mut s = String::from("🔔 <b>Alerts</b>\n");
    for a in &alerts {
        let state_icon = if a.is_triggered { "✔" } else { "⏳" };
        s.push_str(&format!(
            "{state_icon} #<code>{}</code> <code>{}</code> {}{}\n",
            a.id,
            short(&a.token_address),
            if a.condition == "above" { "↑ above" } else { "↓ below" },
            MarketSimulator::format_price(a.target_price)
        ));
    }
    Ok(s)
}

async fn cmd_portfolio(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let positions = repo::list_positions(state.db.conn(), user_id).await?;
    if positions.is_empty() {
        return Ok("📂 No open positions. Start with /buy &lt;token&gt; &lt;amount&gt;.".to_string());
    }
    let mut s = String::from("📂 <b>Portfolio</b>\n\n");
    let mut total_value = 0.0;
    let mut total_unrealized = 0.0;
    for p in &positions {
        let price = state.engine.sim.price(&p.token_address);
        let value = p.quantity * price;
        let unrealized = (price - p.avg_price) * p.quantity;
        total_value += value;
        total_unrealized += unrealized;
        s.push_str(&format!(
            "<code>{}</code>\n  qty {:.4} @ {}\n  value <b>{:.4} ETH</b> · PnL {}\n",
            short(&p.token_address),
            p.quantity,
            MarketSimulator::format_price(p.avg_price),
            value,
            pnl_str(unrealized)
        ));
    }
    s.push_str(&format!(
        "\n💼 Total value: <b>{:.4} ETH</b>\n📊 Unrealized PnL: <b>{}</b>",
        total_value,
        pnl_str(total_unrealized)
    ));
    Ok(s)
}

async fn cmd_history(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let orders = repo::list_orders(state.db.conn(), user_id, 10).await?;
    if orders.is_empty() {
        return Ok("🧾 No orders yet.".to_string());
    }
    let mut s = String::from("🧾 <b>Recent orders</b>\n");
    for o in &orders {
        let icon = match o.side.as_str() {
            "buy" => "🟢",
            "snipe" => "🛰️",
            "sell" => "🔴",
            _ => "⏳",
        };
        s.push_str(&format!(
            "{icon} #<code>{}</code> {} <code>{}</code> {} · {}\n",
            o.id,
            o.side.to_uppercase(),
            short(&o.token_address),
            o.amount_in
                .map(|a| format!("{:.4}", a))
                .unwrap_or_default(),
            o.status
        ));
    }
    Ok(s)
}

async fn cmd_settings(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    match arg.as_deref() {
        None => {
            let slippage: f64 = user_slippage(state, user_id).await?;
            Ok(format!(
                "⚙️ <b>Settings</b>\n\n\
                 Slippage: <b>{:.2}%</b>\n\
                 Paper trading: <b>{}</b>\n\n\
                 Change slippage: <b>/settings slippage 0.05</b>",
                slippage * 100.0,
                if state.config.paper_trading { "ON" } else { "OFF" }
            ))
        }
        Some(rest) => {
            let mut parts = rest.split_whitespace();
            match parts.next() {
                Some("slippage") => {
                    let v: f64 = parts
                        .next()
                        .context("usage: /settings slippage 0.05")?
                        .parse()
                        .context("slippage must be a number, e.g. 0.05")?;
                    if !(0.0..=0.50).contains(&v) {
                        bail!("slippage must be between 0 and 0.5");
                    }
                    repo::set_setting(state.db.conn(), user_id, "slippage", &v.to_string()).await?;
                    Ok(format!("⚙️ Slippage set to <b>{:.2}%</b>", v * 100.0))
                }
                other => Ok(format!("Unknown setting <code>{other:?}</code>. Try: /settings slippage 0.05")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn ensure_user(state: &AppState, msg: &Message) -> Result<i64> {
    let from = msg.from.as_ref().context("message has no sender")?;
    let user = repo::get_or_create_user(
        state.db.conn(),
        from.id.0 as i64,
        from.username.as_deref(),
        Some(from.first_name.as_str()),
    )
    .await?;
    Ok(user.id)
}

async fn user_slippage(state: &AppState, user_id: i64) -> Result<f64> {
    match repo::get_setting(state.db.conn(), user_id, "slippage").await? {
        Some(v) => Ok(v.parse().unwrap_or(0.05)),
        None => Ok(0.05),
    }
}

fn short(addr: &str) -> String {
    let a = addr.trim();
    if a.len() > 10 {
        format!("{}…{}", &a[..6], &a[a.len() - 4..])
    } else {
        a.to_string()
    }
}

fn pnl_str(pnl: f64) -> String {
    if pnl >= 0.0 {
        format!("<b>+{:.4} ETH</b>", pnl)
    } else {
        format!("<b>-{:.4} ETH</b>", pnl.abs())
    }
}

fn risk_badge(score: u8) -> &'static str {
    match score {
        0..=29 => "✅",
        30..=69 => "⚠️",
        _ => "🚫",
    }
}

fn checks_text(report: &TokenReport) -> String {
    report
        .checks
        .iter()
        .map(|c| {
            format!(
                "{} <b>{}</b> — {}",
                if c.passed { "✔" } else { "✖" },
                c.name,
                c.note
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn alert_note(fired: i64) -> String {
    if fired > 0 {
        format!("\n🔔 {fired} price alert(s) fired!")
    } else {
        String::new()
    }
}

fn tx_footer(tx: &str) -> String {
    format!("🔗 tx: <code>{tx}</code>")
}

fn help_text() -> &'static str {
    "⚡ <b>Velocidad commands</b>\n\n\
     👛 /wallet — list wallets\n\
     ➕ /wallet new — create a wallet\n\
     📥 /wallet import &lt;privkey&gt; — import a wallet\n\
     🟢 /buy &lt;token&gt; &lt;amount&gt; — instant buy\n\
     🔴 /sell &lt;token&gt; &lt;qty&gt; — sell\n\
     🛰️ /snipe &lt;token&gt; &lt;amount&gt; — snipe a launch\n\
     ⏳ /limit &lt;token&gt; &lt;price&gt; &lt;amount&gt; — limit order\n\
     📈 /price &lt;token&gt; — current price\n\
     🔬 /scan &lt;token&gt; — security scan\n\
     🔔 /alert &lt;token&gt; &lt;above|below&gt; &lt;price&gt; — price alert\n\
     📂 /portfolio — open positions\n\
     🧾 /history — recent orders\n\
     ⚙️ /settings — trading settings\n\
     ❓ /help — this help"
}
