//! Telegram bot — Trojan/Photon/Axiom-style command surface, now multi-chain.
//!
//! Every token command accepts `chain:address` (e.g. `/buy bsc:0x… 0.5`);
//! without a chain prefix the user's default chain (or `DEFAULT_CHAIN`) is
//! used. Prices come from DexScreener with an offline fallback. When
//! `PAPER_TRADING=false` and `ZEROEX_API_KEY` is set, buys/sells execute
//! real on-chain swaps signed with the user's stored wallet key.

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use teloxide::dispatching::{Dispatcher, HandlerExt, UpdateFilterExt};
use teloxide::prelude::*;
use teloxide::types::{
    CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, ParseMode, Update,
};
use teloxide::utils::command::BotCommands;

use crate::app::AppState;
use crate::chains::{self, Chain};
use crate::crypto;
use crate::db::repo;
use crate::exec::{self, sponsor_account_for, wallet_for_chain, wallet_secret};
use crate::market::{format_native, format_price, format_qty, format_usd, short_addr};
use crate::security::TokenReport;

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "Velocidad trading bot commands",
    parse_with = "split"
)]
pub enum Command {
    #[command(description = "start the bot")]
    Start,
    #[command(description = "show this help")]
    Help,
    #[command(description = "manage wallets: /wallet, /wallet new, /wallet import <privkey>")]
    Wallet,
    #[command(description = "instant buy: /buy <token> <amount>")]
    Buy(String, f64),
    #[command(description = "sell: /sell <token> <qty|all|max>")]
    Sell(String, String),
    #[command(description = "snipe a launch: /snipe <token> <amount>")]
    Snipe(String, f64),
    #[command(description = "limit order: /limit <token> <price> <amount>")]
    Limit(String, f64, f64),
    #[command(description = "current price: /price <token>")]
    Price(String),
    #[command(description = "full market info: /token <token>")]
    Token(String),
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
    #[command(description = "on-chain balances: /balance")]
    Balance,
    #[command(description = "set or show default chain: /chain, /chain bsc")]
    Chain,
    #[command(description = "list supported chains")]
    Chains,
    #[command(description = "trending tokens")]
    Trending,
    #[command(description = "search tokens by name/symbol: /find pepe")]
    Find(String),
    #[command(description = "top boosted tokens")]
    Boosts,
    #[command(description = "LP suggestions & new-pool watch: /lp, /lp watch bsc 20000")]
    Lp,
    #[command(description = "take-profit: /tp <token> <price|pct|off>")]
    Tp(String, String),
    #[command(description = "stop-loss: /sl <token> <price|pct|off>")]
    Sl(String, String),
    #[command(description = "cancel a pending limit or alert: /cancel <id>")]
    Cancel(i64),
    #[command(description = "gas sponsorship: /sponsor, /sponsor on|off|setup|status")]
    Sponsor,
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

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(dispatch_command),
                )
                .branch(dptree::endpoint(unknown_message)),
        )
        .branch(Update::filter_callback_query().endpoint(handle_callback));

    tracing::info!("telegram bot polling started");
    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .build()
        .dispatch()
        .await;
    Ok(())
}

/// Quick-action buttons for a buy fill: partial sells + TP/SL presets.
fn quick_sell_actions(chain: &crate::chains::Chain, token: &str) -> Vec<(String, String)> {
    vec![
        (
            "Sell 25%".to_string(),
            format!("sell|25|{}|{}", chain.id, token),
        ),
        (
            "Sell 50%".to_string(),
            format!("sell|50|{}|{}", chain.id, token),
        ),
        (
            "Sell 100%".to_string(),
            format!("sell|100|{}|{}", chain.id, token),
        ),
        (
            "🎯 TP +20%".to_string(),
            format!("tp|20|{}|{}", chain.id, token),
        ),
        (
            "🛟 SL -20%".to_string(),
            format!("sl|20|{}|{}", chain.id, token),
        ),
    ]
}

/// Send an inline-keyboard message with the given (label, callback_data) rows.
async fn send_actions(bot: &Bot, chat: ChatId, actions: Vec<(String, String)>) {
    let buttons: Vec<InlineKeyboardButton> = actions
        .into_iter()
        .map(|(label, data)| InlineKeyboardButton::callback(label, data))
        .collect();
    let rows: Vec<Vec<InlineKeyboardButton>> =
        buttons.chunks(3).map(|chunk| chunk.to_vec()).collect();
    let _ = bot
        .send_message(chat, "⚡ Quick actions")
        .reply_markup(InlineKeyboardMarkup::new(rows))
        .await;
}

/// Handle inline-button taps: partial sells + TP/SL presets.
async fn handle_callback(bot: Bot, q: CallbackQuery, state: Arc<AppState>) -> ResponseResult<()> {
    let result: Result<String> = async {
        let data = q.data.clone().context("callback has no data")?;
        let mut parts = data.split('|');
        let action = parts.next().context("empty callback data")?;
        let user = repo::get_or_create_user(
            state.db.conn(),
            q.from.id.0 as i64,
            q.from.username.as_deref(),
            Some(q.from.first_name.as_str()),
        )
        .await?;
        match action {
            "sell" => {
                let pct = parts
                    .next()
                    .and_then(|p| p.parse::<f64>().ok())
                    .context("bad sell percentage")?;
                let chain = parts
                    .next()
                    .and_then(crate::chains::Chain::resolve)
                    .context("bad chain")?;
                let token = parts.next().context("missing token")?;
                let wallet = crate::exec::wallet_for_chain(&state, user.id, chain).await?;
                let position = repo::get_position(state.db.conn(), user.id, chain.id, token)
                    .await?
                    .context("no open position for this token")?;
                let qty = position.quantity * (pct / 100.0).clamp(0.0, 1.0);
                if qty <= 0.0 {
                    anyhow::bail!("position is closed");
                }
                let slippage = user_slippage(&state, user.id).await?;
                let outcome =
                    crate::exec::sell(&state, chain, user.id, &wallet, token, qty, slippage)
                        .await?;
                Ok(format!(
                    "✅ Sold {:.0}% of <code>{}</code> — qty {}",
                    pct,
                    short_addr(token),
                    format_qty(outcome.qty())
                ))
            }
            "tp" | "sl" => {
                let pct = parts
                    .next()
                    .and_then(|p| p.parse::<f64>().ok())
                    .context("bad percentage")?;
                if !pct.is_finite() || pct <= 0.0 {
                    anyhow::bail!("percentage must be positive");
                }
                let chain = parts
                    .next()
                    .and_then(crate::chains::Chain::resolve)
                    .context("bad chain")?;
                let token = parts.next().context("missing token")?;
                let position = repo::get_position(state.db.conn(), user.id, chain.id, token)
                    .await?
                    .context("no open position for this token")?;
                let price = if action == "tp" {
                    position.avg_price * (1.0 + pct / 100.0)
                } else {
                    position.avg_price * (1.0 - pct / 100.0)
                };
                let (tp, sl) = if action == "tp" {
                    (Some(price), position.sl_price)
                } else {
                    (position.tp_price, Some(price))
                };
                repo::set_position_tp_sl(state.db.conn(), user.id, chain.id, token, tp, sl).await?;
                Ok(format!(
                    "{} set at <b>{}</b> for <code>{}</code>",
                    if action == "tp" {
                        "🎯 Take-profit"
                    } else {
                        "🛟 Stop-loss"
                    },
                    format_native(price),
                    short_addr(token)
                ))
            }
            other => anyhow::bail!("unknown action {other}"),
        }
    }
    .await;

    if let Some(m) = q.message.as_ref() {
        let text = match &result {
            Ok(t) => t.clone(),
            Err(e) => format!("❌ {}", esc(&e.to_string())),
        };
        let chat = m.chat();
        send_reply(&bot, chat.id, &text).await;
    }
    let _ = bot.answer_callback_query(q.id).await;
    Ok(())
}

async fn dispatch_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    state: Arc<AppState>,
) -> ResponseResult<()> {
    if let Some(user) = msg.from.as_ref() {
        if let Err(retry) = state.bot_rate.check(&format!("u{}", user.id.0)) {
            let _ = bot
                .send_message(msg.chat.id, format!("⏳ Slow down — try again in {retry}s"))
                .await;
            return Ok(());
        }
    }
    // Quick-action buttons attached to buy fills (buy/snipe only).
    let quick_token: Option<String> = match &cmd {
        Command::Buy(t, _) | Command::Snipe(t, _) => Some(t.clone()),
        _ => None,
    };
    let reply = match cmd {
        Command::Start => cmd_start(&state, &msg).await,
        Command::Help => Ok(help_text().to_string()),
        Command::Wallet => {
            let arg = rest_of_command(&msg, "/wallet");
            cmd_wallet(&state, &msg, arg).await
        }
        Command::Buy(token, amount) => cmd_buy(&state, &msg, &token, amount, "buy").await,
        Command::Sell(token, qty) => cmd_sell(&state, &msg, &token, &qty).await,
        Command::Snipe(token, amount) => cmd_buy(&state, &msg, &token, amount, "snipe").await,
        Command::Limit(token, price, amount) => {
            cmd_limit(&state, &msg, &token, price, amount).await
        }
        Command::Price(token) => cmd_price(&state, &msg, &token).await,
        Command::Token(token) => cmd_token(&state, &msg, &token).await,
        Command::Scan(token) => cmd_scan(&state, &msg, &token).await,
        Command::Alert(token, cond, price) => cmd_alert(&state, &msg, &token, &cond, price).await,
        Command::Alerts => cmd_alerts(&state, &msg).await,
        Command::Portfolio => cmd_portfolio(&state, &msg).await,
        Command::History => cmd_history(&state, &msg).await,
        Command::Balance => {
            let arg = rest_of_command(&msg, "/balance");
            cmd_balance(&state, &msg, arg).await
        }
        Command::Chain => {
            let arg = rest_of_command(&msg, "/chain");
            cmd_chain(&state, &msg, arg).await
        }
        Command::Chains => cmd_chains(&state, &msg).await,
        Command::Trending => cmd_trending(&state, &msg).await,
        Command::Find(q) => cmd_find(&state, &msg, &q).await,
        Command::Boosts => cmd_boosts(&state, &msg).await,
        Command::Lp => {
            let arg = rest_of_command(&msg, "/lp");
            cmd_lp(&state, &msg, arg).await
        }
        Command::Tp(token, level) => cmd_tp_sl(&state, &msg, &token, &level, "tp").await,
        Command::Sl(token, level) => cmd_tp_sl(&state, &msg, &token, &level, "sl").await,
        Command::Cancel(id) => cmd_cancel(&state, &msg, id).await,
        Command::Sponsor => {
            let arg = rest_of_command(&msg, "/sponsor");
            cmd_sponsor(&state, &msg, arg).await
        }
        Command::Settings => {
            let arg = rest_of_command(&msg, "/settings");
            cmd_settings(&state, &msg, arg).await
        }
    };

    match reply {
        Ok(text) => {
            send_reply(&bot, msg.chat.id, &text).await;
            if let (Some(tok), Some(from)) = (&quick_token, msg.from.as_ref()) {
                let chain = state.user_chain(from.id.0 as i64).await;
                if let Ok((c, t)) = parse_token_arg(tok, chain) {
                    send_actions(&bot, msg.chat.id, quick_sell_actions(c, &t)).await;
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "command failed");
            send_reply(&bot, msg.chat.id, &format!("❌ {}", esc(&err.to_string()))).await;
        }
    }
    Ok(())
}

/// Everything after the command name in a message.
fn rest_of_command(msg: &Message, command: &str) -> Option<String> {
    msg.text().and_then(|t| {
        t.trim_start()
            .strip_prefix(command)
            .map(|rest| rest.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

async fn unknown_message(bot: Bot, msg: Message, state: Arc<AppState>) -> ResponseResult<()> {
    if let Some(user) = msg.from.as_ref() {
        // Separate budget: free-typing chat must not burn the trading quota.
        if state
            .bot_rate
            .check(&format!("u{}:misc", user.id.0))
            .is_err()
        {
            let _ = bot
                .send_message(msg.chat.id, "⏳ Too many messages — slow down.")
                .await;
            return Ok(());
        }
    }
    bot.send_message(msg.chat.id, help_text())
        .parse_mode(ParseMode::Html)
        .await?;
    Ok(())
}
// ---------------------------------------------------------------------------
// Handlers: start / wallet / chain
// ---------------------------------------------------------------------------

async fn cmd_start(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let wallet = repo::get_default_wallet(state.db.conn(), user_id).await?;
    let addr = wallet
        .map(|w| w.address)
        .unwrap_or_else(|| "none — create one with /wallet new".to_string());
    let chain = state.user_chain(user_id).await;
    Ok(format!(
        "⚡ <b>Velocidad</b> — Trojan-style trading bot\n\
         💾 Database: libSQL{} ({})\n\
         ⛓️ Default chain: <b>{}</b> (change with /chain)\n\
         👤 User: <code>{}</code>\n\
         👛 Wallet: <code>{}</code>\n\n\
         <b>Commands</b>\n{}",
        if state.config.is_remote() {
            " · Turso"
        } else {
            ""
        },
        esc(&state.config.database_url),
        chain.display(),
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
                let icon = if w.network == "solana" { "🪐" } else { "⛓️" };
                s.push_str(&format!(
                    "{icon} {}{} #<code>{}</code> <code>{}</code> · {}\n",
                    badge, w.label, w.id, w.address, w.network
                ));
            }
            s.push_str("\nEVM wallets work on every EVM chain; Solana needs its own (/wallet new solana).\nSwitch: /wallet use &lt;id&gt;");
            Ok(s)
        }
        Some(rest) if rest.starts_with("new") => {
            let sub = rest.trim_start_matches("new").trim();
            if sub == "solana" || sub == "sol" {
                let sw = crypto::generate_solana_wallet()?;
                let enc = state.keyring.encrypt(hex::decode(&sw.seed_hex)?.as_slice())?;
                repo::insert_wallet(state.db.conn(), user_id, &sw.address, "Main", Some(&enc), true)
                    .await?;
                Ok(format!(
                    "🪐 <b>Solana wallet created</b>\n\n\",
                     📮 Address: <code>{}</code>\n\",
                     🔑 Private key (shown once): <code>{}</code>\n\n\",
                     ⚠️ Store it safely — it is encrypted at rest with your master key.",
                    sw.address, sw.private_key_base58
                ))
            } else {
                let gw = crypto::generate_wallet()?;
                let enc = state.keyring.encrypt(gw.private_key_hex.as_bytes())?;
                repo::insert_wallet(state.db.conn(), user_id, &gw.address, "Main", Some(&enc), true)
                    .await?;
                Ok(format!(
                    "✅ <b>Wallet created</b>\n\n\",
                     📮 Address: <code>{}</code>\n\",
                     🔑 Private key (shown once): <code>{}</code>\n\n\",
                     ⚠️ Store it safely — it is encrypted at rest with your master key.",
                    gw.address, gw.private_key_hex
                ))
            }
        }
        Some(rest) if rest.starts_with("use") => {
            let id = rest
                .trim_start_matches("use")
                .trim()
                .parse::<i64>()
                .context("usage: /wallet use <id>")?;
            if repo::set_default_wallet(state.db.conn(), user_id, id).await? {
                Ok(format!("⭐ Wallet #<code>{id}</code> is now the default."))
            } else {
                bail!("wallet #<code>{id}</code> not found for your account");
            }
        }
        Some(rest) if rest.starts_with("import") => {
            let key = rest.trim_start_matches("import").trim();
            // Auto-detect: EVM hex keys vs Solana base58 keys.
            match crypto::import_wallet(key) {
                Ok(gw) => {
                    let enc = state.keyring.encrypt(gw.private_key_hex.as_bytes())?;
                    repo::insert_wallet(state.db.conn(), user_id, &gw.address, "Imported", Some(&enc), true)
                        .await?;
                    Ok(format!("✅ Imported EVM wallet <code>{}</code>", gw.address))
                }
                Err(_) => {
                    let sw = crypto::import_solana_wallet(key)
                        .context("invalid private key — expected EVM hex (64 chars) or Solana base58 (Phantom-style)")?;
                    let enc = state.keyring.encrypt(hex::decode(&sw.seed_hex)?.as_slice())?;
                    repo::insert_wallet(state.db.conn(), user_id, &sw.address, "Imported", Some(&enc), true)
                        .await?;
                    Ok(format!("✅ Imported Solana wallet <code>{}</code>", sw.address))
                }
            }
        }
        Some(other) => Ok(format!(
            "Unknown wallet action <code>{}</code>.\nUse /wallet, /wallet new or /wallet import &lt;privkey&gt;.",
            esc(other)
        )),
    }
}

async fn cmd_chain(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    match arg.as_deref() {
        None => {
            let chain = state.user_chain(user_id).await;
            Ok(format!(
                "⛓️ <b>Default chain</b>: {}\n\nChange it with <b>/chain &lt;chain&gt;</b>\nor list all with <b>/chains</b>.",
                chain.display()
            ))
        }
        Some(name) => {
            let chain = Chain::resolve(name).context("unknown chain — see /chains")?;
            repo::set_setting(state.db.conn(), user_id, "chain", chain.id).await?;
            Ok(format!(
                "⛓️ Default chain set to <b>{}</b>",
                chain.display()
            ))
        }
    }
}

async fn cmd_chains(state: &AppState, msg: &Message) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let mut s = String::from("⛓️ <b>Supported chains</b>\n\n");
    for c in chains::CHAINS {
        s.push_str(&format!(
            "<code>{}</code> — {} ({}) · {}\n",
            c.id, c.name, c.chain_id, c.native
        ));
    }
    s.push_str("\nUse <code>chain:0x…</code> in any token command to override your default.");
    Ok(s)
}
async fn cmd_balance(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    // /balance [chain] | /balance [wallet-id] — a chain argument overrides
    // the user's default chain (previously silently ignored).
    let chain: &'static crate::chains::Chain = match arg.as_deref() {
        Some(a) if a.parse::<i64>().is_err() => match crate::chains::Chain::resolve(a) {
            Some(c) => c,
            None => state.user_chain(user_id).await,
        },
        _ => state.user_chain(user_id).await,
    };
    let wallets = repo::list_wallets(state.db.conn(), user_id).await?;
    if wallets.is_empty() {
        return Ok("👛 Create a wallet first: /wallet new".to_string());
    }

    // Optional <wallet-id> argument to pick a specific wallet.
    let wallet = if let Some(id_str) = arg.as_deref().and_then(|a| a.parse::<i64>().ok()) {
        wallets
            .iter()
            .find(|w| w.id == id_str)
            .context("wallet id not found")?
    } else if chain.kind == crate::chains::ChainKind::Solana {
        wallets
            .iter()
            .find(|w| w.network == "solana")
            .unwrap_or(&wallets[0])
    } else {
        wallets
            .iter()
            .find(|w| w.network != "solana")
            .unwrap_or(&wallets[0])
    };

    let native_usd = state.market.native_price_usd(chain).await;
    let mut s = format!("💰 <b>Balances</b> — {} · {}\n\n", chain.name, chain.native);

    // On-chain native balance.
    let native_bal: Option<f64> = match chain.kind {
        crate::chains::ChainKind::Evm => {
            match state.rpc.native_balance(chain, &wallet.address).await {
                Ok(wei) => Some(wei as f64 / 1e18),
                Err(e) => {
                    s.push_str(&format!(
                        "⚠️ native balance unavailable: {}\n",
                        esc(&e.to_string())
                    ));
                    None
                }
            }
        }
        crate::chains::ChainKind::Solana => {
            match state.solana.balance(chain, &wallet.address).await {
                Ok(lamports) => Some(lamports as f64 / 1e9),
                Err(e) => {
                    s.push_str(&format!(
                        "⚠️ SOL balance unavailable: {}\n",
                        esc(&e.to_string())
                    ));
                    None
                }
            }
        }
    };
    if let Some(bal) = native_bal {
        s.push_str(&format!(
            "⛽ {:.6} {} · <b>{}</b>\n",
            bal,
            chain.native,
            format_usd(bal * native_usd)
        ));
    }

    // Solana: show actual SPL holdings (top by USD value).
    if chain.kind == crate::chains::ChainKind::Solana {
        if let Ok(balances) = state.solana.spl_balances(chain, &wallet.address).await {
            let mut items: Vec<(String, f64, f64)> = Vec::new();
            for b in balances.iter().take(8) {
                let q = state.market.quote(chain, &b.mint).await;
                if q.price_usd > 0.0 {
                    items.push((q.symbol, b.amount, q.price_usd * b.amount));
                }
            }
            items.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
            for (symbol, amount, usd) in items.iter().take(6) {
                s.push_str(&format!(
                    "🪙 {:.4} <b>{}</b> · <b>{}</b>\n",
                    amount,
                    esc(symbol),
                    format_usd(*usd)
                ));
            }
        }
    }

    // Curated ERC20 balances.
    for (symbol, addr) in chain.default_erc20s {
        if let Ok(raw) = state.rpc.erc20_balance(chain, addr, &wallet.address).await {
            let decimals = state.rpc.erc20_decimals(chain, addr).await;
            let bal = raw as f64 / 10f64.powi(decimals as i32);
            if bal > 0.0 {
                s.push_str(&format!("🪙 {:.4} <b>{symbol}</b>\n", bal));
            }
        }
    }

    // Paper account value from open positions.
    let positions = repo::list_positions(state.db.conn(), user_id).await?;
    if !positions.is_empty() {
        let mut total = 0.0;
        let mut pnl = 0.0;
        for p in &positions {
            if let Some(pchain) = chains::by_id(&p.network) {
                let q = state.market.quote(pchain, &p.token_address).await;
                total += p.quantity * q.price_native;
                pnl += (q.price_native - p.avg_price) * p.quantity;
            }
        }
        s.push_str(&format!(
            "\n📂 Paper account: <b>{:.6} {}</b> ({} · PnL {})\n",
            total,
            chain.native,
            format_usd(total * native_usd),
            pnl_str(pnl, chain.native)
        ));
    }

    let mode = if state.config.paper_trading {
        "paper".to_string()
    } else if state.swap.enabled() {
        "live".to_string()
    } else {
        "paper (no 0x key)".to_string()
    };
    s.push_str(&format!(
        "\n⚙️ Mode: <b>{mode}</b> · Wallet: <code>{}</code>",
        wallet.address
    ));
    Ok(s)
}
// ---------------------------------------------------------------------------
// Handlers: trading
// ---------------------------------------------------------------------------

async fn cmd_buy(
    state: &AppState,
    msg: &Message,
    token_arg: &str,
    amount: f64,
    side: &str,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let token = chain.1;
    let chain = chain.0;
    let wallet = wallet_for_chain(state, user_id, chain).await?;
    let slippage = user_slippage(state, user_id).await?;

    if side == "snipe" {
        let report = state.scanner.scan(&state.db, chain, &token).await;
        if report.is_honeypot {
            bail!(
                "🚫 <code>{}</code> is flagged as a honeypot — snipe blocked",
                short_addr(&token)
            );
        }
    }

    let outcome = exec::buy(
        state, chain, user_id, &wallet, &token, amount, slippage, side,
    )
    .await?;
    Ok(match outcome {
        exec::TradeOutcome::Paper(receipt) => fmt_buy_paper(&receipt, chain, side),
        exec::TradeOutcome::SolanaLive(o) => format!(
            "{} <b>{}</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
             Chain: <b>solana</b>\n\
             Token: <code>{}</code> ({})\n\
             Qty: <b>{}</b>\n\
             Price: <b>{}</b>\n\
             Spent: <b>{:.9} SOL</b>\n\
             Slippage: {:.2}%{}",
            if side == "snipe" { "🛰️" } else { "✅" },
            side.to_uppercase(),
            o.result.explorer_link,
            &o.result.tx_signature[..o.result.tx_signature.len().min(10)],
            short_addr(&token),
            token_symbol(state, chain, &token).await,
            format_qty(o.qty),
            format_price(o.price),
            amount,
            slippage * 100.0,
            if o.sponsored {
                "\n⛽ gas sponsored"
            } else {
                ""
            }
        ),
        exec::TradeOutcome::EvmLive(o) => format!(
            "{} <b>{}</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
             Chain: <b>{}</b>\n\
             Token: <code>{}</code> ({})\n\
             Qty: <b>{}</b>\n\
             Price: <b>{}</b>\n\
             Spent: <b>{:.6} {}</b>\n\
             Slippage: {:.2}%{}",
            if side == "snipe" { "🛰️" } else { "✅" },
            side.to_uppercase(),
            o.result.explorer_link,
            &o.result.tx_hash[..o.result.tx_hash.len().min(10)],
            chain.id,
            short_addr(&token),
            token_symbol(state, chain, &token).await,
            format_qty(o.qty),
            format_price(o.price),
            amount,
            chain.native,
            slippage * 100.0,
            if o.sponsored {
                "\n⛽ gas sponsored"
            } else {
                ""
            }
        ),
    })
}

async fn cmd_sell(
    state: &AppState,
    msg: &Message,
    token_arg: &str,
    qty_arg: &str,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let token = chain.1;
    let chain = chain.0;
    let wallet = wallet_for_chain(state, user_id, chain).await?;
    let slippage = user_slippage(state, user_id).await?;

    let position = repo::get_position(state.db.conn(), user_id, chain.id, &token).await?;
    let live = crate::exec::live_enabled(state, chain);
    let qty = if qty_arg.eq_ignore_ascii_case("all") || qty_arg.eq_ignore_ascii_case("max") {
        if live {
            match chain.kind {
                crate::chains::ChainKind::Evm => {
                    let raw = state
                        .rpc
                        .erc20_balance(chain, &token, &wallet.address)
                        .await?;
                    let decimals = state.rpc.erc20_decimals(chain, &token).await;
                    raw as f64 / 10f64.powi(decimals as i32)
                }
                crate::chains::ChainKind::Solana => {
                    let balances = state.solana.spl_balances(chain, &wallet.address).await?;
                    let b = balances
                        .iter()
                        .find(|b| b.mint.eq_ignore_ascii_case(&token))
                        .context("no on-chain balance for this token")?;
                    b.amount
                }
            }
        } else {
            position
                .as_ref()
                .context("no open position for this token")?
                .quantity
        }
    } else {
        qty_arg
            .parse::<f64>()
            .context("quantity must be a number, 'all' or 'max'")?
    };
    if qty <= 0.0 {
        bail!("quantity must be positive");
    }

    let outcome = exec::sell(state, chain, user_id, &wallet, &token, qty, slippage).await?;
    Ok(match outcome {
        exec::TradeOutcome::Paper(receipt) => fmt_sell_paper(&receipt, chain),
        exec::TradeOutcome::SolanaLive(o) => format!(
            "💸 <b>SELL</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
             Chain: <b>solana</b>\n\
             Token: <code>{}</code> ({})\n\
             Qty: <b>{}</b>\n\
             Price: <b>{}</b>\n\
             Proceeds: <b>{:.9} SOL</b>{}",
            o.result.explorer_link,
            &o.result.tx_signature[..o.result.tx_signature.len().min(10)],
            short_addr(&token),
            token_symbol(state, chain, &token).await,
            format_qty(o.qty),
            format_price(o.price),
            o.result.native_amount as f64 / 1e9,
            if o.sponsored {
                "\n⛽ gas sponsored"
            } else {
                ""
            }
        ),
        exec::TradeOutcome::EvmLive(o) => format!(
            "💸 <b>SELL</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
             Chain: <b>{}</b>\n\
             Token: <code>{}</code> ({})\n\
             Qty: <b>{}</b>\n\
             Price: <b>{}</b>\n\
             Proceeds: <b>{:.6} {}</b>{}",
            o.result.explorer_link,
            &o.result.tx_hash[..o.result.tx_hash.len().min(10)],
            chain.id,
            short_addr(&token),
            token_symbol(state, chain, &token).await,
            format_qty(o.qty),
            format_price(o.price),
            o.result.native_amount as f64 / 1e18,
            chain.native,
            o.result
                .approval_tx
                .as_ref()
                .map(|h| format!("\n✅ allowance approved: <code>{}</code>", short_addr(h)))
                .unwrap_or_default(),
        ),
    })
}

async fn cmd_price(state: &AppState, msg: &Message, token_arg: &str) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let (chain, token) = (chain.0, chain.1);
    let quote = state.market.quote(chain, &token).await;
    let report = state.scanner.scan(&state.db, chain, &token).await;
    Ok(format!(
        "📈 <b>{}</b> <code>{}</code> · {}\n\n\
         Price: <b>{}</b> · {} {}\n\
         Liquidity: {}\n\
         Volume 24h: {}\n\
         Change 24h: <b>{:+.2}%</b>\n\
         Risk: <b>{}/100</b> {}\n\n{}",
        esc(&quote.name),
        short_addr(&token),
        chain.id,
        format_price(quote.price_usd),
        format_price(quote.price_native),
        chain.native,
        format_usd(quote.liquidity_usd),
        format_usd(quote.volume_24h_usd),
        quote.price_change_24h,
        report.risk_score,
        risk_badge(report.risk_score),
        checks_text(&report)
    ))
}

async fn cmd_token(state: &AppState, msg: &Message, token_arg: &str) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let (chain, token) = (chain.0, chain.1);
    let quote = state.market.quote(chain, &token).await;
    let age = quote
        .pair_created_at
        .map(|t| {
            let secs = (chrono::Utc::now().timestamp() - t).max(0);
            if secs < 3600 {
                format!("{}m", secs / 60)
            } else if secs < 86400 {
                format!("{}h", secs / 3600)
            } else {
                format!("{}d", secs / 86400)
            }
        })
        .unwrap_or_else(|| "—".to_string());
    let pair_link = quote
        .pair_address
        .as_ref()
        .map(|p| format!("https://dexscreener.com/{}/{}", chain.dex_segment, p))
        .unwrap_or_default();
    Ok(format!(
        "🔎 <b>{}</b> <code>{}</code>\n\
         Chain: <b>{}</b> · Pair age: {}\n\
         Price: <b>{}</b> · {} per {}\n\
         Liquidity: <b>{}</b>\n\
         Volume 24h: <b>{}</b>\n\
         Change 24h: <b>{:+.2}%</b> ({} buys / {} sells)\n\
         FDV: {}\n\
         Best pair: {} on {} · source: <b>{}</b>\n\
         🔗 <a href=\"{}\">View on DexScreener</a>",
        esc(&quote.name),
        short_addr(&token),
        chain.id,
        age,
        format_price(quote.price_usd),
        format_price(quote.price_native),
        chain.native,
        format_usd(quote.liquidity_usd),
        format_usd(quote.volume_24h_usd),
        quote.price_change_24h,
        quote.txns_buy_24h,
        quote.txns_sell_24h,
        quote.fdv.map(format_usd).unwrap_or_else(|| "—".to_string()),
        esc(&quote.dex.clone().unwrap_or_default()),
        short_addr(quote.pair_address.as_deref().unwrap_or_default()),
        quote.source,
        pair_link
    ))
}

async fn cmd_scan(state: &AppState, msg: &Message, token_arg: &str) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let (chain, token) = (chain.0, chain.1);
    let report = state.scanner.scan(&state.db, chain, &token).await;
    Ok(format!(
        "🔬 <b>Security scan</b> · {}\n\n\
         Token: <code>{}</code>\n\
         Name/Symbol: {} / {}\n\
         Risk score: <b>{}/100</b> {}\n\
         Verdict: <b>{}</b>\n\
         Data: {}\n\n{}",
        chain.id,
        short_addr(&token),
        esc(&report.name),
        esc(&report.symbol),
        report.risk_score,
        risk_badge(report.risk_score),
        if report.is_honeypot {
            "🚫 HONEYPOT"
        } else if report.risk_score >= 70 {
            "⚠️ HIGH RISK"
        } else {
            "✅ tradable"
        },
        report.data_source,
        checks_text(&report)
    ))
}

async fn cmd_portfolio(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let positions = repo::list_positions(state.db.conn(), user_id).await?;
    if positions.is_empty() {
        return Ok(
            "📂 No open positions. Start with /buy &lt;token&gt; &lt;amount&gt;.".to_string(),
        );
    }

    // Group positions per network for a per-chain view.
    let mut groups: std::collections::BTreeMap<String, Vec<&crate::db::models::Position>> =
        Default::default();
    for p in &positions {
        groups.entry(p.network.clone()).or_default().push(p);
    }

    let mut s = String::from("📂 <b>Portfolio</b>\n\n");
    let mut grand_usd = 0.0;
    for (network, list) in &groups {
        let chain = chains::by_id(network).unwrap_or_else(|| chains::by_id("ethereum").unwrap());
        let native_usd = state.market.native_price_usd(chain).await;
        s.push_str(&format!("⛓️ <b>{}</b> · {}\n", chain.name, chain.native));
        let mut chain_usd = 0.0;
        for p in list {
            let quote = state.market.quote(chain, &p.token_address).await;
            let price = quote.price_native;
            let value = p.quantity * price;
            let value_usd = value * native_usd;
            let pnl = (price - p.avg_price) * p.quantity;
            chain_usd += value_usd;
            s.push_str(&format!(
                "  <b>{}</b> <code>{}</code>\n    qty {} @ {} · value <b>{:.6} {}</b> · PnL {}\n",
                esc(&quote.symbol),
                short_addr(&p.token_address),
                format_qty(p.quantity),
                format_price(p.avg_price),
                value,
                chain.native,
                pnl_str(pnl, chain.native)
            ));
        }
        s.push_str(&format!("  Σ <b>{}</b>\n\n", format_usd(chain_usd)));
        grand_usd += chain_usd;
    }
    s.push_str(&format!(
        "💼 <b>Total portfolio: {}</b>",
        format_usd(grand_usd)
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
            "limit" => "⏳",
            _ => "·",
        };
        let tx = o
            .tx_hash
            .as_ref()
            .map(|t| format!(" · <code>{}</code>", short_addr(t)))
            .unwrap_or_default();
        let err = o
            .error
            .as_ref()
            .map(|e| format!(" · ❌ {}", esc(e)))
            .unwrap_or_default();
        s.push_str(&format!(
            "{icon} #<code>{}</code> <b>{}</b> {} <code>{}</code> {} · {}{}{}\n",
            o.id,
            o.network,
            o.side.to_uppercase(),
            short_addr(&o.token_address),
            o.amount_in.map(|a| format!("{:.6}", a)).unwrap_or_default(),
            o.status,
            tx,
            err
        ));
    }
    Ok(s)
}

async fn cmd_trending(state: &AppState, msg: &Message) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let list = state.market.trending().await;
    if list.is_empty() {
        return Ok(
            "🔥 No trending data right now (offline or rate-limited). Try again later.".to_string(),
        );
    }
    let mut s = String::from("🔥 <b>Trending tokens</b> (DexScreener profiles)\n\n");
    for t in list.iter().take(8) {
        let label = t
            .symbol
            .clone()
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| short_addr(&t.token_address));
        s.push_str(&format!(
            "<code>{}</code> <b>{}</b> — <code>{}</code>\n",
            t.chain,
            esc(&label),
            t.token_address
        ));
    }
    s.push_str("\nInspect any with /token &lt;chain:address&gt;");
    Ok(s)
}

async fn cmd_find(state: &AppState, msg: &Message, q: &str) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let results = state.market.search(q).await;
    if results.is_empty() {
        return Ok(format!(
            "🔍 No results for <code>{}</code>. Try a ticker like /find pepe.",
            esc(q)
        ));
    }
    let mut s = format!("🔍 <b>Search: {}</b>\n\n", esc(q));
    for r in results.iter().take(10) {
        s.push_str(&format!(
            "<b>{}</b> · {} · <code>{}</code>\n",
            esc(&r.symbol),
            esc(&r.chain),
            r.address
        ));
        s.push_str(&format!(
            "   {} · liq {} · vol {} · 24h <b>{:+.1}%</b>\n",
            format_price(r.price_usd),
            format_usd(r.liquidity_usd),
            format_usd(r.volume_24h_usd),
            r.price_change_24h
        ));
    }
    s.push_str("\nTrade: /buy &lt;chain:address&gt; &lt;amount&gt;");
    Ok(s)
}

async fn cmd_boosts(state: &AppState, msg: &Message) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let boosts = state.market.boosts().await;
    if boosts.is_empty() {
        return Ok("🚀 No boosted tokens right now (offline or rate-limited).".to_string());
    }
    let mut s = String::from("🚀 <b>Top boosted tokens</b> (DexScreener)\n\n");
    for b in boosts.iter().take(10) {
        s.push_str(&format!(
            "<code>{}</code> <b>{}</b> · <code>{}</code>\n",
            b.chain,
            short_addr(&b.address),
            b.address
        ));
    }
    s.push_str("\nInspect with /token &lt;chain:address&gt;");
    Ok(s)
}

async fn cmd_sponsor(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = state.user_chain(user_id).await;
    let wallet = match wallet_for_chain(state, user_id, chain).await {
        Ok(w) => w,
        Err(_) => {
            return Ok(
                "👛 Create a wallet first (EVM: /wallet new, Solana: /wallet new solana)."
                    .to_string(),
            )
        }
    };
    let secret = wallet_secret(state, &wallet)?;

    match arg.as_deref() {
        None | Some("status") => sponsorship_status(state, user_id, chain, &wallet).await,
        Some("on") => sponsorship_on(state, user_id, chain, &wallet, &secret).await,
        Some("off") => sponsorship_off(state, user_id, chain, &wallet, &secret).await,
        Some("setup") => {
            let from_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
            if !state.is_admin(from_id) {
                return Ok("⛔ /sponsor setup is an operator action.".to_string());
            }
            sponsorship_setup(state, chain).await
        }
        Some(other) => Ok(format!(
            "Unknown /sponsor action <code>{}</code>. Use: on · off · status · setup",
            esc(other)
        )),
    }
}

/// Opt in: EVM → EIP-7702 delegation tx; Solana → SPL delegate on all balances.
async fn sponsorship_on(
    state: &AppState,
    user_id: i64,
    chain: &Chain,
    wallet: &crate::db::models::Wallet,
    secret: &[u8; 32],
) -> Result<String> {
    let setting_key = format!("sponsor:{}", chain.id);
    match chain.kind {
        crate::chains::ChainKind::Evm => {
            let Some((sponsor_addr, _)) = state.swap.sponsor.as_ref() else {
                return Ok(
                    "⚠️ No sponsor configured — the operator must set SPONSOR_KEY.".to_string(),
                );
            };
            let account = sponsor_account_for(state, chain).await?;
            if account.is_empty() {
                return Ok(
                    "⚠️ No SponsorAccount deployed on this chain yet — run /sponsor setup first."
                        .to_string(),
                );
            }
            // Already delegated to THIS sponsor account?
            match state.rpc.delegation_of(chain, &wallet.address).await {
                Ok(Some(target)) if target == account => {
                    repo::set_setting(state.db.conn(), user_id, &setting_key, "on").await?;
                    return Ok(format!(
                        "✅ Already delegated to <code>{}</code> — sponsorship ON for {}.\nGas will be paid by <code>{}</code>.",
                        short_addr(&target),
                        chain.id,
                        short_addr(sponsor_addr)
                    ));
                }
                Ok(Some(target)) => {
                    return Ok(format!(
                        "⚠️ EOA is already delegated to <code>{}</code> — that is NOT the sponsor account.\nRun /sponsor off first, then /sponsor on.",
                        short_addr(&target)
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    return Ok(format!(
                        "⚠️ Could not check delegation on {}: {}. Try again.",
                        chain.id,
                        esc(&e.to_string())
                    ));
                }
            }
            let hash = state
                .swap
                .send_delegation(chain, &wallet.address, secret, &account)
                .await?;
            repo::set_setting(state.db.conn(), user_id, &setting_key, "on").await?;
            Ok(format!(
                "🪪 <b>EIP-7702 delegation set</b> ({})\n\n\
                 EOA: <code>{}</code>\n\
                 Delegate: <code>{}</code>\n\
                 Gas sponsor: <code>{}</code>\n\
                 🔗 <a href=\"{}\">{}</a>\n\n\
                 ℹ️ From now on, sponsored trades pay no gas — the sponsor covers fees.",
                chain.id,
                short_addr(&wallet.address),
                short_addr(&account),
                short_addr(sponsor_addr),
                chain.explorer_link(&hash),
                short_addr(&hash)
            ))
        }
        crate::chains::ChainKind::Solana => {
            let Some(sp) = state.solana.sponsor.as_ref() else {
                return Ok(
                    "⚠️ No Solana sponsor configured — the operator must set SPONSOR_SOLANA_KEY."
                        .to_string(),
                );
            };
            let blockhash = state.solana.latest_blockhash(chain).await?;
            let owner: [u8; 32] = bs58::decode(wallet.address.trim())
                .into_vec()
                .context("bad wallet")?
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad pubkey"))?;
            let balances = state.solana.spl_balances(chain, &wallet.address).await?;
            let mut txs: Vec<String> = Vec::new();
            for b in balances.iter().filter(|b| b.amount > 0.0) {
                let account_bytes: [u8; 32] = bs58::decode(b.account.trim())
                    .into_vec()
                    .ok()
                    .and_then(|v| v.try_into().ok())
                    .context("bad token account")?;
                let raw_amount = (b.amount * 10f64.powi(b.decimals as i32)) as u64;
                let (message, signers) = crate::solana::build_sponsored_approve(
                    &owner,
                    &account_bytes,
                    &sp.pubkey,
                    raw_amount,
                    &sp.pubkey,
                    &blockhash,
                );
                let sig = state
                    .solana
                    .send_sponsored(chain, &message, &signers, &[&sp.seed, secret])
                    .await?;
                txs.push(format!(
                    "  ✅ {} · <code>{}</code>",
                    b.mint,
                    short_addr(&sig)
                ));
            }
            if txs.is_empty() {
                return Ok("🪐 No non-zero token balances to delegate. Fund the wallet first, then /sponsor on.".to_string());
            }
            repo::set_setting(state.db.conn(), user_id, &setting_key, "on").await?;
            Ok(format!(
                "🪐 <b>Solana delegation set</b>\n\nSponsor (delegate + fee payer): <code>{}</code>\n\n{}\n\nℹ️ Your trades are now sponsored — the sponsor pays tx fees.\nTokens delegated: {} accounts",
                sp.address(),
                txs.join("\n"),
                txs.len()
            ))
        }
    }
}

async fn sponsorship_off(
    state: &AppState,
    user_id: i64,
    chain: &Chain,
    wallet: &crate::db::models::Wallet,
    secret: &[u8; 32],
) -> Result<String> {
    let setting_key = format!("sponsor:{}", chain.id);
    match chain.kind {
        crate::chains::ChainKind::Evm => {
            match state.rpc.delegation_of(chain, &wallet.address).await {
                Ok(Some(_)) => {
                    let hash = state
                        .swap
                        .clear_delegation(chain, &wallet.address, secret)
                        .await?;
                    repo::set_setting(state.db.conn(), user_id, &setting_key, "off").await?;
                    Ok(format!(
                        "🪪 Delegation cleared ({}). 🔗 {}",
                        chain.id,
                        chain.explorer_link(&hash)
                    ))
                }
                Ok(None) => {
                    repo::set_setting(state.db.conn(), user_id, &setting_key, "off").await?;
                    Ok(format!(
                        "🪪 No delegation set on {} — sponsorship OFF.",
                        chain.id
                    ))
                }
                Err(e) => Ok(format!(
                    "⚠️ Could not check delegation on {}: {}. Try again.",
                    chain.id,
                    esc(&e.to_string())
                )),
            }
        }
        crate::chains::ChainKind::Solana => {
            let Some(sp) = state.solana.sponsor.as_ref() else {
                repo::set_setting(state.db.conn(), user_id, &setting_key, "off").await?;
                return Ok("🪐 Solana sponsorship OFF (no sponsor configured).".to_string());
            };
            let blockhash = state.solana.latest_blockhash(chain).await?;
            let owner: [u8; 32] = bs58::decode(wallet.address.trim())
                .into_vec()
                .context("bad wallet")?
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad pubkey"))?;
            let balances = state.solana.spl_balances(chain, &wallet.address).await?;
            let mut txs: Vec<String> = Vec::new();
            for b in balances.iter().filter(|b| b.amount > 0.0) {
                let account_bytes: [u8; 32] = bs58::decode(b.account.trim())
                    .into_vec()
                    .ok()
                    .and_then(|v| v.try_into().ok())
                    .context("bad token account")?;
                let (message, signers) = crate::solana::build_sponsored_revoke(
                    &owner,
                    &account_bytes,
                    &sp.pubkey,
                    &blockhash,
                );
                let sig = state
                    .solana
                    .send_sponsored(chain, &message, &signers, &[&sp.seed, secret])
                    .await?;
                txs.push(format!(
                    "  ✅ revoked {} · <code>{}</code>",
                    b.mint,
                    short_addr(&sig)
                ));
            }
            repo::set_setting(state.db.conn(), user_id, &setting_key, "off").await?;
            if txs.is_empty() {
                Ok("🪐 Solana sponsorship OFF (no delegated balances found).".to_string())
            } else {
                Ok(format!(
                    "🪐 <b>Solana sponsorship OFF</b> — delegates revoked\n\n{}\n\nℹ️ The sponsor can no longer move your tokens.",
                    txs.join("\n")
                ))
            }
        }
    }
}

async fn sponsorship_status(
    state: &AppState,
    user_id: i64,
    chain: &Chain,
    wallet: &crate::db::models::Wallet,
) -> Result<String> {
    let setting_key = format!("sponsor:{}", chain.id);
    let opt_in = repo::get_setting(state.db.conn(), user_id, &setting_key)
        .await?
        .unwrap_or_default();
    match chain.kind {
        crate::chains::ChainKind::Evm => {
            let delegated = state
                .rpc
                .delegation_of(chain, &wallet.address)
                .await
                .unwrap_or(None);
            let sponsor_addr = state
                .swap
                .sponsor
                .as_ref()
                .map(|(a, _)| a.clone())
                .unwrap_or_default();
            let account = sponsor_account_for(state, chain).await.unwrap_or_default();
            Ok(format!(
                "🪪 <b>Gas sponsorship</b> — {}\n\n\
                 Opt-in: <b>{}</b>\n\
                 Delegation: <b>{}</b>\n\
                 Delegate contract: <code>{}</code>\n\
                 Sponsor: <code>{}</code>\n\n\
                 /sponsor on · /sponsor off · /sponsor setup",
                chain.id,
                if opt_in == "on" { "ON ✅" } else { "off" },
                match &delegated {
                    Some(t) => format!("set → <code>{}</code>", short_addr(t)),
                    None => "none".to_string(),
                },
                if account.is_empty() {
                    "— (run /sponsor setup)".to_string()
                } else {
                    account
                },
                if sponsor_addr.is_empty() {
                    "not configured".to_string()
                } else {
                    sponsor_addr
                },
            ))
        }
        crate::chains::ChainKind::Solana => {
            let sp_addr = state
                .solana
                .sponsor
                .as_ref()
                .map(|s| s.address())
                .unwrap_or_default();
            Ok(format!(
                "🪐 <b>Gas sponsorship</b> — solana\n\n\
                 Opt-in: <b>{}</b>\n\
                 Sponsor (fee payer / delegate): <code>{}</code>\n\n\
                 /sponsor on · /sponsor off",
                if opt_in == "on" { "ON ✅" } else { "off" },
                if sp_addr.is_empty() {
                    "not configured".to_string()
                } else {
                    sp_addr
                },
            ))
        }
    }
}

/// Deploy the SponsorAccount (operator action, once per chain) and store the
/// address in the global settings (user_id 0).
async fn sponsorship_setup(state: &AppState, chain: &Chain) -> Result<String> {
    if chain.kind != crate::chains::ChainKind::Evm {
        return Ok(
            "🪐 Sponsor setup is EVM-only; Solana sponsorship uses the operator key directly."
                .to_string(),
        );
    }
    let Some((sponsor_addr, sponsor_secret)) = state.swap.sponsor.as_ref() else {
        return Ok("⚠️ SPONSOR_KEY is not set — the operator must configure it.".to_string());
    };
    if let Ok(account) = sponsor_account_for(state, chain).await {
        if !account.is_empty() {
            return Ok(format!(
                "✅ SponsorAccount already deployed on {}: <code>{}</code>",
                chain.id, account
            ));
        }
    }
    let (address, hash) = state
        .swap
        .deploy_sponsor_account(chain, sponsor_addr, sponsor_secret)
        .await?;
    repo::set_setting(
        state.db.conn(),
        0,
        &format!("sponsor_account:{}", chain.id),
        &address,
    )
    .await?;
    Ok(format!(
        "🏗️ <b>SponsorAccount deployed</b> ({})\n\n\
         Address: <code>{}</code>\n\
         Sponsor: <code>{}</code>\n\
         🔗 {}\n\n\
         Users can now /sponsor on to delegate their EOA.",
        chain.id,
        address,
        short_addr(sponsor_addr),
        chain.explorer_link(&hash)
    ))
}
/// Escape a string for Telegram HTML messages (external data may contain
/// markup — token symbols/names come from DexScreener, errors from RPCs).
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn cmd_settings(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    match arg.as_deref() {
        None => {
            let slippage: f64 = user_slippage(state, user_id).await?;
            let chain = state.user_chain(user_id).await;
            Ok(format!(
                "⚙️ <b>Settings</b>\n\n\
                 Slippage: <b>{:.2}%</b>\n\
                 Default chain: <b>{}</b>\n\
                 Paper trading: <b>{}</b>\n\
                 Live swaps: <b>{}</b>\n\n\
                 Change: /settings slippage 0.05 · /chain bsc",
                slippage * 100.0,
                chain.display(),
                if state.config.paper_trading {
                    "ON"
                } else {
                    "OFF"
                },
                if state.swap.enabled() {
                    "ON"
                } else {
                    "OFF (set ZEROEX_API_KEY)"
                }
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
                Some("chain") => {
                    let name = parts.next().context("usage: /settings chain bsc")?;
                    let chain = Chain::resolve(name).context("unknown chain — see /chains")?;
                    repo::set_setting(state.db.conn(), user_id, "chain", chain.id).await?;
                    Ok(format!(
                        "⛓️ Default chain set to <b>{}</b>",
                        chain.display()
                    ))
                }
                other => Ok(format!(
                    "Unknown setting <code>{}</code>. Try: /settings slippage 0.05",
                    esc(other.unwrap_or_default())
                )),
            }
        }
    }
}

async fn cmd_alert(
    state: &AppState,
    msg: &Message,
    token_arg: &str,
    cond: &str,
    price: f64,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let (chain, token) = (chain.0, chain.1);
    let cond = cond.to_lowercase();
    if cond != "above" && cond != "below" {
        bail!("condition must be <code>above</code> or <code>below</code>");
    }
    let alert =
        repo::insert_alert(state.db.conn(), user_id, chain.id, &token, &cond, price).await?;
    let quote = state.market.quote(chain, &token).await;
    Ok(format!(
        "🔔 <b>Alert created</b> #<code>{}</code>\n\n\
         Chain: <b>{}</b>\n\
         Token: <code>{}</code>\n\
         When price goes {cond} <b>{}</b> {}\n\
         Current: <b>{}</b> {}\n\
         ℹ️ You'll be notified automatically.",
        alert.id,
        chain.id,
        short_addr(&token),
        format_native(price),
        chain.native,
        format_native(quote.price_native),
        chain.native
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
        let icon = if a.is_triggered { "✔" } else { "⏳" };
        s.push_str(&format!(
            "{icon} #<code>{}</code> <b>{}</b> <code>{}</code> {}{}\n",
            a.id,
            a.network,
            short_addr(&a.token_address),
            if a.condition == "above" {
                "↑ above"
            } else {
                "↓ below"
            },
            format_price(a.target_price)
        ));
    }
    Ok(s)
}

async fn cmd_limit(
    state: &AppState,
    msg: &Message,
    token_arg: &str,
    price: f64,
    amount: f64,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let token = chain.1;
    let chain = chain.0;
    let wallet = crate::exec::wallet_for_chain(state, user_id, chain)
        .await
        .ok();
    let order = state
        .engine
        .place_limit(
            &state.db,
            chain,
            user_id,
            wallet.as_ref().map(|w| w.id),
            &token,
            price,
            amount,
        )
        .await?;
    Ok(format!(
        "⏳ <b>LIMIT order placed</b> #<code>{}</code>\n\n\
         Chain: <b>{}</b>\n\
         Token: <code>{}</code>\n\
         Buy at: <b>{}</b> (when price ≤ limit)\n\
         Amount: <b>{:.6} {}</b>\n\
         ℹ️ A background worker fills it automatically and notifies you.\n\n\
         🧪 Paper simulation for now — limit orders don't execute on-chain yet.",
        order.id,
        chain.id,
        short_addr(&token),
        format_price(price),
        amount,
        chain.native
    ))
}

async fn cmd_lp(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let arg = arg.as_deref().unwrap_or("suggest").trim().to_string();
    if arg == "suggest" || arg.is_empty() {
        return lp_suggestions_text(state).await;
    }
    let mut parts = arg.split_whitespace();
    match parts.next() {
        Some("watch") => {
            let chain_arg = parts.next().unwrap_or("").to_string();
            if chain_arg.is_empty() || chain_arg == "off" {
                // /lp watch off — clear every chain for this user
                let watched: Vec<String> = repo::list_setting_keys(state.db.conn(), user_id)
                    .await?
                    .into_iter()
                    .filter(|k| k.starts_with("lp_watch:"))
                    .collect();
                for k in watched {
                    let _ = repo::delete_setting(state.db.conn(), user_id, &k).await;
                }
                return Ok("💧 New-pool alerts OFF.".to_string());
            }
            let chain = crate::chains::Chain::resolve(&chain_arg)
                .ok_or_else(|| anyhow::anyhow!("unknown chain '{}'", chain_arg))?;
            let min_usd = match parts.next() {
                Some(v) => v.parse::<f64>().map_err(|_| anyhow::anyhow!("min liquidity must be a number (USD)"))?,
                None => 20_000.0,
            };
            if !min_usd.is_finite() || min_usd < 1_000.0 {
                anyhow::bail!("min liquidity must be at least 1000 USD");
            }
            repo::set_setting(
                state.db.conn(),
                user_id,
                &format!("lp_watch:{}", chain.id),
                &min_usd.to_string(),
            )
            .await?;
            Ok(format!(
                "💧 New-pool alerts ON for <b>{}</b> — notify when a pool appears with ≥ {} liquidity.\nUse /lp watch off to stop.",
                chain.id,
                crate::market::format_usd(min_usd)
            ))
        }
        Some("chains") => {
            let watched = repo::list_setting_keys(state.db.conn(), user_id).await?;
            let active: Vec<String> = watched
                .into_iter()
                .filter(|k| k.starts_with("lp_watch:"))
                .map(|k| k.trim_start_matches("lp_watch:").to_string())
                .collect();
            if active.is_empty() {
                Ok("💧 No chains watched. Try /lp watch bsc 20000".to_string())
            } else {
                Ok(format!(
                    "💧 Watching: <b>{}</b>\n/lp watch off to stop.",
                    active.join(", ")
                ))
            }
        }
        other => Ok(format!(
            "Unknown /lp action <code>{}</code>.\nUse: suggest · watch &lt;chain&gt; &lt;min_usd&gt; · watch off · chains",
            esc(other.unwrap_or_default())
        )),
    }
}

async fn lp_suggestions_text(state: &AppState) -> Result<String> {
    let list = state.market.lp_suggestions().await;
    if list.is_empty() {
        return Ok(
            "💧 No LP-worthy tokens right now — markets are thin or volatile.\nTry /lp watch bsc 20000 for new-pool alerts."
                .to_string(),
        );
    }
    let mut s = String::from("💧 <b>LP candidates</b> (liquidity-friendly, low IL risk)\n\n");
    for t in list.iter().take(8) {
        s.push_str(&format!(
            "<b>{}</b> · {} · score <b>{}/100</b>\n  <code>{}</code>\n  {} — {}",
            esc(&t.symbol),
            t.chain,
            t.score,
            t.address,
            t.reasons.join(", "),
            crate::market::format_usd(t.liquidity_usd)
        ));
        s.push('\n');
    }
    s.push('\n');
    s.push_str(
        "⚠️ Scores rank <b>LP risk</b>, not token upside. Always /scan before providing liquidity.",
    );
    Ok(s)
}

async fn cmd_tp_sl(
    state: &AppState,
    msg: &Message,
    token_arg: &str,
    level: &str,
    kind: &str,
) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let parsed = parse_token_arg(token_arg, state.user_chain(user_id).await)?;
    let token = parsed.1;
    let chain = parsed.0;
    let position = repo::get_position(state.db.conn(), user_id, chain.id, &token)
        .await?
        .context("no open position for this token — buy first")?;
    if position.quantity <= 0.0 {
        anyhow::bail!("position is closed — nothing to protect");
    }

    let (tp, sl) = if level == "off" {
        (None, None)
    } else {
        let value = if let Some(pct) = level.strip_suffix('%') {
            let pct = pct
                .parse::<f64>()
                .context("percentage must be a number, e.g. 20%")?;
            if !pct.is_finite() || pct <= 0.0 {
                anyhow::bail!("percentage must be positive");
            }
            position.avg_price * (1.0 + pct / 100.0 * if kind == "tp" { 1.0 } else { -1.0 })
        } else {
            level
                .parse::<f64>()
                .context("level must be a price or a percentage like 20%")?
        };
        if !value.is_finite() || value <= 0.0 {
            anyhow::bail!("level must be positive");
        }
        if kind == "tp" {
            (Some(value), position.sl_price)
        } else {
            (position.tp_price, Some(value))
        }
    };

    repo::set_position_tp_sl(state.db.conn(), user_id, chain.id, &token, tp, sl).await?;
    let label = if kind == "tp" {
        "Take-profit"
    } else {
        "Stop-loss"
    };
    let state_line = match (tp, sl) {
        (Some(t), _) if kind == "tp" => format!(
            "🎯 {label}: <b>{}</b> ({}% up)",
            format_price(t),
            (t / position.avg_price - 1.0) * 100.0
        ),
        (_, Some(s)) if kind == "sl" => format!(
            "🛟 {label}: <b>{}</b> ({}% down)",
            format_price(s),
            (1.0 - s / position.avg_price) * 100.0
        ),
        _ => "🛡️ Protection OFF".to_string(),
    };
    Ok(format!(
        "{} on <code>{}</code> ({})\nEntry: <b>{}</b>\n{}\nℹ️ The bot sells automatically when the price crosses the level.",
        if kind == "tp" { "🎯" } else { "🛟" },
        short_addr(&token),
        chain.id,
        format_price(position.avg_price),
        state_line
    ))
}

async fn cmd_cancel(state: &AppState, msg: &Message, id: i64) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    if repo::cancel_pending_order(state.db.conn(), user_id, id).await? {
        return Ok(format!(
            "❌ Pending limit order #<code>{id}</code> cancelled."
        ));
    }
    if repo::delete_alert(state.db.conn(), user_id, id).await? {
        return Ok(format!("🗑️ Alert #<code>{id}</code> deleted."));
    }
    bail!("nothing to cancel: #<code>{id}</code> is not your pending limit order or alert");
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
/// Parse `chain:0x…` / `0x…` token arguments.
fn parse_token_arg(input: &str, default: &'static Chain) -> Result<(&'static Chain, String)> {
    Chain::resolve_token_arg(input, default)
        .ok_or_else(|| anyhow::anyhow!("invalid token — expected <chain:0x…> or <0x…>"))
}
async fn token_symbol(state: &AppState, chain: &Chain, token: &str) -> String {
    match repo::get_token(state.db.conn(), chain.id, token)
        .await
        .ok()
        .flatten()
    {
        Some(t) => t
            .symbol
            .map(|s| esc(&s))
            .unwrap_or_else(|| short_addr(token)),
        None => short_addr(token),
    }
}

fn fmt_buy_paper(receipt: &crate::trading::TradeReceipt, chain: &Chain, side: &str) -> String {
    let o = &receipt.order;
    let emoji = if side == "snipe" { "🛰️" } else { "✅" };
    format!(
        "{emoji} <b>{}</b> filled (paper) · {}\n\n\
         Token: <code>{}</code> ({})\n\
         Qty: <b>{}</b>\n\
         Price: <b>{}</b>\n\
         Spent: <b>{:.6} {}</b>\n\
         Position: {} @ {}\n\
         Slippage: {:.2}%{}{}",
        side.to_uppercase(),
        chain.id,
        short_addr(&o.token_address),
        esc(&receipt.token_symbol.clone().unwrap_or_default()),
        format_qty(o.amount_out.unwrap_or(0.0)),
        format_price(o.price.unwrap_or(0.0)),
        o.amount_in.unwrap_or(0.0),
        chain.native,
        format_qty(receipt.position_quantity),
        format_price(receipt.position_avg_price),
        o.slippage * 100.0,
        alert_note(receipt.alerts_fired),
        sim_note(receipt.price_source)
    )
}

fn fmt_sell_paper(receipt: &crate::trading::TradeReceipt, chain: &Chain) -> String {
    let o = &receipt.order;
    format!(
        "💸 <b>SELL</b> filled (paper) · {}\n\n\
         Token: <code>{}</code> ({})\n\
         Qty: <b>{}</b>\n\
         Price: <b>{}</b>\n\
         Proceeds: <b>{:.6} {}</b>\n\
         Realized PnL: <b>{}</b>{}{}",
        chain.id,
        short_addr(&o.token_address),
        esc(&receipt.token_symbol.clone().unwrap_or_default()),
        format_qty(o.amount_in.unwrap_or(0.0)),
        format_price(o.price.unwrap_or(0.0)),
        o.amount_out.unwrap_or(0.0),
        chain.native,
        pnl_str(receipt.realized_pnl.unwrap_or(0.0), chain.native),
        alert_note(receipt.alerts_fired),
        sim_note(receipt.price_source)
    )
}

fn pnl_str(pnl: f64, native: &str) -> String {
    if pnl >= 0.0 {
        format!("<b>+{:.6} {native}</b>", pnl)
    } else {
        format!("<b>-{:.6} {native}</b>", pnl.abs())
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
                esc(&c.note)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sim_note(source: &'static str) -> String {
    if source == "simulator" {
        "\n⚠️ <b>No live market data for this token — price is simulated.</b>".to_string()
    } else {
        String::new()
    }
}

fn alert_note(fired: i64) -> String {
    if fired > 0 {
        format!("\n🔔 {fired} price alert(s) fired!")
    } else {
        String::new()
    }
}

/// Send a reply as one or more Telegram messages. Telegram caps messages at
/// 4096 characters, so long replies (portfolios, trending, ...) are split on
/// line boundaries and each chunk is sent separately.
async fn send_reply(bot: &Bot, chat_id: ChatId, text: &str) {
    for chunk in chunk_message(text, 4096) {
        if let Err(e) = bot
            .send_message(chat_id, chunk)
            .parse_mode(ParseMode::Html)
            .await
        {
            tracing::warn!(error = %e, "reply send failed (message may be malformed HTML)");
        }
    }
}

/// Split a message into chunks of at most 'limit' chars, preferring newline
/// boundaries; an oversized single line is hard-split at a UTF-8 boundary.
fn chunk_message(text: &str, limit: usize) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if line.len() > limit {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            for part in hard_split(line, limit) {
                chunks.push(part);
            }
        } else if !current.is_empty() && current.len() + line.len() > limit {
            chunks.push(std::mem::take(&mut current));
            current.push_str(line);
        } else {
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Hard-split a single line at char boundaries of at most 'limit' bytes each.
fn hard_split(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while rest.len() > limit {
        let mut split = limit;
        while !rest.is_char_boundary(split) {
            split -= 1;
        }
        parts.push(rest[..split].to_string());
        rest = &rest[split..];
    }
    if !rest.is_empty() {
        parts.push(rest.to_string());
    }
    parts
}

fn help_text() -> &'static str {
    "⚡ <b>Velocidad commands</b>\n\n\
     👛 /wallet — list · /wallet new [solana] · /wallet import &lt;key&gt;\n\
     ⛓️ /chain — set default chain · /chains — list chains\n\
     🟢 /buy &lt;token&gt; &lt;amount&gt; — instant buy\n\
     🔴 /sell &lt;token&gt; &lt;qty|all&gt; — sell\n\
     🛰️ /snipe &lt;token&gt; &lt;amount&gt; — snipe with honeypot gate\n\
     ⏳ /limit &lt;token&gt; &lt;price&gt; &lt;amount&gt; — auto-filled limit order\n\
     📈 /price &lt;token&gt; · 🔎 /token &lt;token&gt; — market info\n\
     🔬 /scan &lt;token&gt; — security scan\n\
     🔔 /alert &lt;token&gt; &lt;above|below&gt; &lt;price&gt; — price alert\n\
     🔥 /trending — trending tokens\n\
     💧 /lp — LP suggestions · /lp watch &lt;chain&gt; &lt;min_usd&gt; — new-pool alerts\n\
     🎯 /tp &lt;token&gt; &lt;price|pct&gt; · 🛟 /sl &lt;token&gt; &lt;price|pct&gt; — auto-protect positions\n\
     ❌ /cancel &lt;id&gt; — cancel a pending limit order or delete an alert\n\
     🚀 /boosts — top boosted tokens\n\
     🔍 /find &lt;ticker&gt; — search tokens by name/symbol\n\
     💰 /balance — on-chain balances\n\
     📂 /portfolio — positions + PnL (all chains)\n\
     🧾 /history — recent orders\n\
     ⚙️ /settings — slippage etc.\n\n\
     <b>Multi-chain</b>: prefix any token with <code>chain:</code>\n\
     (<code>eth:</code> <code>bsc:</code> <code>sol:</code> …),\n\
     e.g. <code>/buy sol:EPjF… 0.5</code>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_escapes_html() {
        assert_eq!(esc("<b>bold</b> & x"), "&lt;b&gt;bold&lt;/b&gt; &amp; x");
        assert_eq!(esc("plain"), "plain");
        assert_eq!(esc("a<b"), "a&lt;b");
    }

    #[test]
    fn chunk_message_respects_limit_and_rejoins() {
        // Multi-line text that needs several chunks.
        let text: String = (0..50).map(|i| format!("line {i:02}\n")).collect();
        let chunks = chunk_message(&text, 60);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 60));
        assert_eq!(chunks.concat(), text);

        // A single oversized line is hard-split without breaking UTF-8.
        let long: String = "€".repeat(100) + "\n";
        let chunks = chunk_message(&long, 30);
        assert!(chunks.iter().all(|c| c.len() <= 30));
        assert_eq!(chunks.concat(), long);
        assert!(chunks.iter().all(|c| c.is_char_boundary(c.len())));
    }
}
