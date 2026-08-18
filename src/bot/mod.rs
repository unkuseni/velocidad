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
use teloxide::types::{Message, ParseMode, Update};
use teloxide::utils::command::BotCommands;

use crate::app::AppState;
use crate::chains::{self, Chain};
use crate::crypto;
use crate::db::repo;
use crate::market::{format_price, format_qty, format_usd, short_addr};
use crate::security::TokenReport;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Velocidad trading bot commands", parse_with = "split")]
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
        Command::Buy(token, amount) => cmd_buy(&state, &msg, &token, amount, "buy").await,
        Command::Sell(token, qty) => cmd_sell(&state, &msg, &token, &qty).await,
        Command::Snipe(token, amount) => cmd_buy(&state, &msg, &token, amount, "snipe").await,
        Command::Limit(token, price, amount) => cmd_limit(&state, &msg, &token, price, amount).await,
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

/// Everything after the command name in a message.
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
        if state.config.is_remote() { " · Turso" } else { "" },
        state.config.database_url,
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
                    "{icon} {}{} <code>{}</code> · {}\n",
                    badge, w.label, w.address, w.network
                ));
            }
            s.push_str("\nEVM wallets work on every EVM chain; Solana needs its own (/wallet new solana).");
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
            other
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
            Ok(format!("⛓️ Default chain set to <b>{}</b>", chain.display()))
        }
    }
}

async fn cmd_chains(state: &AppState, msg: &Message) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let mut s = String::from("⛓️ <b>Supported chains</b>\n\n");
    for c in chains::CHAINS {
        s.push_str(&format!("<code>{}</code> — {} ({}) · {}\n", c.id, c.name, c.chain_id, c.native));
    }
    s.push_str("\nUse <code>chain:0x…</code> in any token command to override your default.");
    Ok(s)
}
async fn cmd_balance(state: &AppState, msg: &Message, arg: Option<String>) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let chain = state.user_chain(user_id).await;
    let wallets = repo::list_wallets(state.db.conn(), user_id).await?;
    if wallets.is_empty() {
        return Ok("👛 Create a wallet first: /wallet new".to_string());
    }

    // Optional <wallet-id> argument to pick a specific wallet.
    let wallet = if let Some(id_str) = arg.as_deref().and_then(|a| a.parse::<i64>().ok()) {
        wallets.iter().find(|w| w.id == id_str).context("wallet id not found")?
    } else if chain.kind == crate::chains::ChainKind::Solana {
        wallets.iter().find(|w| w.network == "solana").unwrap_or(&wallets[0])
    } else {
        wallets.iter().find(|w| w.network != "solana").unwrap_or(&wallets[0])
    };

    let native_usd = state.market.native_price_usd(chain).await;
    let mut s = format!(
        "💰 <b>Balances</b> — {} · {}\n\n",
        chain.name,
        chain.native
    );

    // On-chain native balance.
    let native_bal: Option<f64> = match chain.kind {
        crate::chains::ChainKind::Evm => match state.rpc.native_balance(chain, &wallet.address).await {
            Ok(wei) => Some(wei as f64 / 1e18),
            Err(e) => {
                s.push_str(&format!("⚠️ native balance unavailable: {e}\n"));
                None
            }
        },
        crate::chains::ChainKind::Solana => match state.solana.balance(chain, &wallet.address).await {
            Ok(lamports) => Some(lamports as f64 / 1e9),
            Err(e) => {
                s.push_str(&format!("⚠️ SOL balance unavailable: {e}\n"));
                None
            }
        },
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
                s.push_str(&format!("🪙 {:.4} <b>{}</b> · <b>{}</b>\n", amount, symbol, format_usd(*usd)));
            }
        }
    }

    // Curated ERC20 balances.
    for (symbol, addr) in chain.default_erc20s {
        match state.rpc.erc20_balance(chain, addr, &wallet.address).await {
            Ok(raw) => {
                let decimals = state.rpc.erc20_decimals(chain, addr).await;
                let bal = raw as f64 / 10f64.powi(decimals as i32);
                if bal > 0.0 {
                    s.push_str(&format!("🪙 {:.4} <b>{symbol}</b>\n", bal));
                }
            }
            Err(_) => {},
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
    s.push_str(&format!("\n⚙️ Mode: <b>{mode}</b> · Wallet: <code>{}</code>", wallet.address));
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
            bail!("🚫 <code>{}</code> is flagged as a honeypot — snipe blocked", short_addr(&token));
        }
    }

    // Paper trading fills at live prices on every chain kind.
    if state.config.paper_trading || (chain.kind == crate::chains::ChainKind::Evm && !state.swap.enabled()) {
        let receipt = state
            .engine
            .buy(
                &state.db,
                chain,
                user_id,
                Some(wallet.id),
                &token,
                amount,
                slippage,
                side,
            )
            .await?;
        return Ok(fmt_buy_paper(&receipt, chain, side));
    }

    match chain.kind {
        crate::chains::ChainKind::Solana => {
            // Live Solana swap via Jupiter (keyless).
            let secret = wallet_secret(state, &wallet)?;
            let res = state.solana.buy(chain, &token, amount, slippage, &wallet.address, &secret).await?;
            if !res.success {
                bail!("swap failed — tx {}", res.tx_signature);
            }
            let decimals = state.solana.token_decimals(chain, &token).await.unwrap_or(9);
            let qty = res.token_amount as f64 / 10f64.powi(decimals as i32);
            let price = res.price_native;
            record_live_order(state, user_id, Some(wallet.id), chain, &token, side, amount, qty, price, slippage, &res.tx_signature).await?;
            Ok(format!(
                "{} <b>{}</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
                 Chain: <b>solana</b>\n\
                 Token: <code>{}</code> ({})\n\
                 Qty: <b>{}</b>\n\
                 Price: <b>{}</b>\n\
                 Spent: <b>{:.9} SOL</b>\n\
                 Slippage: {:.2}%",
                if side == "snipe" { "🛰️" } else { "✅" },
                side.to_uppercase(),
                res.explorer_link,
                &res.tx_signature[..res.tx_signature.len().min(10)],
                short_addr(&token),
                token_symbol(state, chain, &token).await,
                format_qty(qty),
                format_price(price),
                amount,
                slippage * 100.0
            ))
        }
        crate::chains::ChainKind::Evm => {
            // Live EVM swap via 0x.
            let secret = wallet_secret(state, &wallet)?;
            let amount_wei = (amount * 1e18) as u128;
            if amount_wei == 0 {
                bail!("amount too small for a live swap");
            }
            let res = state
                .swap
                .buy_native(chain, &token, amount_wei, slippage, &wallet.address, &secret)
                .await?;
            if !res.success {
                bail!("swap reverted — tx {}", res.tx_hash);
            }
            let decimals = state.rpc.erc20_decimals(chain, &token).await;
            let qty = res.token_amount as f64 / 10f64.powi(decimals as i32);
            let price = res.price_native;
            record_live_order(state, user_id, Some(wallet.id), chain, &token, side, amount, qty, price, slippage, &res.tx_hash).await?;
            Ok(format!(
                "{} <b>{}</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
                 Chain: <b>{}</b>\n\
                 Token: <code>{}</code> ({})\n\
                 Qty: <b>{}</b>\n\
                 Price: <b>{}</b>\n\
                 Spent: <b>{:.6} {}</b>\n\
                 Slippage: {:.2}%",
                if side == "snipe" { "🛰️" } else { "✅" },
                side.to_uppercase(),
                res.explorer_link,
                &res.tx_hash[..res.tx_hash.len().min(10)],
                chain.id,
                short_addr(&token),
                token_symbol(state, chain, &token).await,
                format_qty(qty),
                format_price(price),
                amount,
                chain.native,
                slippage * 100.0
            ))
        }
    }
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

    let position = repo::get_position(state.db.conn(), user_id, &token).await?;
    let live = !state.config.paper_trading;
    let qty = if qty_arg == "all" || qty_arg == "max" {
        if live {
            match chain.kind {
                crate::chains::ChainKind::Evm => {
                    let raw = state.rpc.erc20_balance(chain, &token, &wallet.address).await?;
                    let decimals = state.rpc.erc20_decimals(chain, &token).await;
                    raw as f64 / 10f64.powi(decimals as i32)
                }
                crate::chains::ChainKind::Solana => {
                    let balances = state.solana.spl_balances(chain, &wallet.address).await?;
                    let b = balances.iter().find(|b| b.mint.eq_ignore_ascii_case(&token))
                        .context("no on-chain balance for this token")?;
                    b.amount
                }
            }
        } else {
            position.as_ref().context("no open position for this token")?.quantity
        }
    } else {
        qty_arg
            .parse::<f64>()
            .context("quantity must be a number, 'all' or 'max'")?
    };
    if qty <= 0.0 {
        bail!("quantity must be positive");
    }

    if !live || (chain.kind == crate::chains::ChainKind::Evm && !state.swap.enabled()) {
        let receipt = state
            .engine
            .sell(
                &state.db,
                chain,
                user_id,
                Some(wallet.id),
                &token,
                qty,
                slippage,
            )
            .await?;
        return Ok(fmt_sell_paper(&receipt, chain));
    }

    match chain.kind {
        crate::chains::ChainKind::Solana => {
            let secret = wallet_secret(state, &wallet)?;
            let res = state.solana.sell(chain, &token, qty, slippage, &wallet.address, &secret).await?;
            if !res.success {
                bail!("swap failed — tx {}", res.tx_signature);
            }
            let proceeds = res.native_amount as f64 / 1e9;
            let price = res.price_native;
            record_live_order(state, user_id, Some(wallet.id), chain, &token, "sell", qty, proceeds, price, slippage, &res.tx_signature).await?;
            Ok(format!(
                "💸 <b>SELL</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
                 Chain: <b>solana</b>\n\
                 Token: <code>{}</code> ({})\n\
                 Qty: <b>{}</b>\n\
                 Price: <b>{}</b>\n\
                 Proceeds: <b>{:.9} SOL</b>{}",
                res.explorer_link,
                &res.tx_signature[..res.tx_signature.len().min(10)],
                short_addr(&token),
                token_symbol(state, chain, &token).await,
                format_qty(qty),
                format_price(price),
                proceeds,
                "",
            ))
        }
        crate::chains::ChainKind::Evm => {
            let secret = wallet_secret(state, &wallet)?;
            let res = state
                .swap
                .sell_tokens(chain, &token, qty, slippage, &wallet.address, &secret)
                .await?;
            if !res.success {
                bail!("swap reverted — tx {}", res.tx_hash);
            }
            let proceeds = res.native_amount as f64 / 1e18;
            let price = res.price_native;
            // record_live_order keeps paper accounting in sync (reduces when a position exists).
            record_live_order(state, user_id, Some(wallet.id), chain, &token, "sell", qty, proceeds, price, slippage, &res.tx_hash).await?;
            Ok(format!(
                "💸 <b>SELL</b> filled <b>LIVE</b> 🔗 <a href=\"{}\" >{}</a>\n\n\
                 Chain: <b>{}</b>\n\
                 Token: <code>{}</code> ({})\n\
                 Qty: <b>{}</b>\n\
                 Price: <b>{}</b>\n\
                 Proceeds: <b>{:.6} {}</b>{}",
                res.explorer_link,
                &res.tx_hash[..res.tx_hash.len().min(10)],
                chain.id,
                short_addr(&token),
                token_symbol(state, chain, &token).await,
                format_qty(qty),
                format_price(price),
                proceeds,
                chain.native,
                res.approval_tx
                    .as_ref()
                    .map(|h| format!("\n✅ allowance approved: <code>{}</code>", short_addr(h)))
                    .unwrap_or_default(),
            ))
        }
    }
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
        quote.name,
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
    let age = quote.pair_created_at
        .map(|t| {
            let secs = (chrono::Utc::now().timestamp() - t).max(0);
            if secs < 3600 { format!("{}m", secs / 60) }
            else if secs < 86400 { format!("{}h", secs / 3600) }
            else { format!("{}d", secs / 86400) }
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
        quote.name,
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
        quote.fdv.map(|f| format_usd(f)).unwrap_or_else(|| "—".to_string()),
        quote.dex.clone().unwrap_or_default(),
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
        report.name,
        report.symbol,
        report.risk_score,
        risk_badge(report.risk_score),
        if report.is_honeypot { "🚫 HONEYPOT" }
        else if report.risk_score >= 70 { "⚠️ HIGH RISK" }
        else { "✅ tradable" },
        report.data_source,
        checks_text(&report)
    ))
}

async fn cmd_portfolio(state: &AppState, msg: &Message) -> Result<String> {
    let user_id = ensure_user(state, msg).await?;
    let positions = repo::list_positions(state.db.conn(), user_id).await?;
    if positions.is_empty() {
        return Ok("📂 No open positions. Start with /buy &lt;token&gt; &lt;amount&gt;.".to_string());
    }

    // Group positions per network for a per-chain view.
    let mut groups: std::collections::BTreeMap<String, Vec<&crate::db::models::Position>> = Default::default();
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
                quote.symbol,
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
    s.push_str(&format!("💼 <b>Total portfolio: {}</b>", format_usd(grand_usd)));
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
        s.push_str(&format!(
            "{icon} #<code>{}</code> <b>{}</b> {} <code>{}</code> {} · {}{}\n",
            o.id,
            o.network,
            o.side.to_uppercase(),
            short_addr(&o.token_address),
            o.amount_in.map(|a| format!("{:.6}", a)).unwrap_or_default(),
            o.status,
            tx
        ));
    }
    Ok(s)
}

async fn cmd_trending(state: &AppState, msg: &Message) -> Result<String> {
    let _ = ensure_user(state, msg).await?;
    let list = state.market.trending().await;
    if list.is_empty() {
        return Ok("🔥 No trending data right now (offline or rate-limited). Try again later.".to_string());
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
            label,
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
        return Ok(format!("🔍 No results for <code>{}</code>. Try a ticker like /find pepe.", sanitize_html(q)));
    }
    let mut s = format!("🔍 <b>Search: {}</b>\n\n", sanitize_html(q));
    for r in results.iter().take(10) {
        s.push_str(&format!(
            "<b>{}</b> · {} · <code>{}</code>\n",
            r.symbol, r.chain, r.address
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

fn sanitize_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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
                if state.config.paper_trading { "ON" } else { "OFF" },
                if state.swap.enabled() { "ON" } else { "OFF (set ZEROEX_API_KEY)" }
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
                    Ok(format!("⛓️ Default chain set to <b>{}</b>", chain.display()))
                }
                other => Ok(format!(
                    "Unknown setting <code>{other:?}</code>. Try: /settings slippage 0.05",
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
    let alert = repo::insert_alert(state.db.conn(), user_id, chain.id, &token, &cond, price).await?;
    let quote = state.market.quote(chain, &token).await;
    Ok(format!(
        "🔔 <b>Alert created</b> #<code>{}</code>\n\n\
         Chain: <b>{}</b>\n\
         Token: <code>{}</code>\n\
         When price goes {cond} <b>{}</b>\n\
         Current: <b>{}</b>\n\
         ℹ️ You'll be notified automatically.",
        alert.id,
        chain.id,
        short_addr(&token),
        format_price(price),
        format_price(quote.price_native)
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
            if a.condition == "above" { "↑ above" } else { "↓ below" },
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
    let wallet = repo::get_default_wallet(state.db.conn(), user_id).await?;
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
         ℹ️ A background worker fills it automatically and notifies you.",
        order.id,
        chain.id,
        short_addr(&token),
        format_price(price),
        amount,
        chain.native
    ))
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


/// Default wallet for a chain: Solana wallets are separate from EVM wallets.
async fn wallet_for_chain(
    state: &AppState,
    user_id: i64,
    chain: &Chain,
) -> Result<crate::db::models::Wallet> {
    let network = if chain.kind == crate::chains::ChainKind::Solana { "solana" } else { "ethereum" };
    match repo::get_default_wallet_for(state.db.conn(), user_id, network).await? {
        Some(w) => Ok(w),
        None => {
            let hint = if chain.kind == crate::chains::ChainKind::Solana {
                "create a Solana wallet first with /wallet new solana"
            } else {
                "create a wallet first with /wallet new"
            };
            Err(anyhow::anyhow!("{}", hint))
        }
    }
}
/// Parse `chain:0x…` / `0x…` token arguments.
fn parse_token_arg(input: &str, default: &'static Chain) -> Result<(&'static Chain, String)> {
    Chain::resolve_token_arg(input, default)
        .ok_or_else(|| anyhow::anyhow!("invalid token — expected <chain:0x…> or <0x…>"))
}

/// Decrypt the wallet's private key for live signing.
fn wallet_secret(state: &AppState, wallet: &crate::db::models::Wallet) -> Result<[u8; 32]> {
    let enc = wallet
        .encrypted_key
        .as_ref()
        .context("wallet is watch-only — import its private key with /wallet import <key> to trade live")?;
    let bytes = state.keyring.decrypt(enc)?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("decrypted key has wrong length"))
}

/// Record a live (on-chain) order + update paper accounting atomically.
async fn record_live_order(
    state: &AppState,
    user_id: i64,
    wallet_id: Option<i64>,
    chain: &Chain,
    token: &str,
    side: &str,
    amount_in: f64,
    amount_out: f64,
    price: f64,
    slippage: f64,
    tx_hash: &str,
) -> Result<()> {
    let conn = state.db.conn();
    let tx = conn.transaction().await?;
    if side == "sell" {
        let _ = repo::reduce_position(&tx, user_id, token, amount_in, price).await;
    } else {
        repo::add_to_position(&tx, user_id, wallet_id, chain.id, token, amount_out, price).await?;
    }
    repo::insert_order(
        &tx,
        &repo::NewOrder {
            user_id,
            wallet_id,
            network: chain.id,
            token_address: token,
            side,
            amount_in: Some(amount_in),
            amount_out: Some(amount_out),
            price: Some(price),
            slippage,
            status: "executed",
            tx_hash: Some(tx_hash),
            error: None,
        },
    )
    .await?;
    let _ = repo::trigger_satisfied_alerts(&tx, user_id, token, price).await?;
    tx.commit().await?;
    Ok(())
}

async fn token_symbol(state: &AppState, chain: &Chain, token: &str) -> String {
    let _ = chain;
    match repo::get_token(state.db.conn(), token).await.ok().flatten() {
        Some(t) => t.symbol.unwrap_or_else(|| short_addr(token)),
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
         Slippage: {:.2}%{}",
        side.to_uppercase(),
        chain.id,
        short_addr(&o.token_address),
        receipt.token_symbol.clone().unwrap_or_default(),
        format_qty(o.amount_out.unwrap_or(0.0)),
        format_price(o.price.unwrap_or(0.0)),
        o.amount_in.unwrap_or(0.0),
        chain.native,
        format_qty(receipt.position_quantity),
        format_price(receipt.position_avg_price),
        o.slippage * 100.0,
        alert_note(receipt.alerts_fired)
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
         Realized PnL: <b>{}</b>{}",
        chain.id,
        short_addr(&o.token_address),
        receipt.token_symbol.clone().unwrap_or_default(),
        format_qty(o.amount_in.unwrap_or(0.0)),
        format_price(o.price.unwrap_or(0.0)),
        o.amount_out.unwrap_or(0.0),
        chain.native,
        pnl_str(receipt.realized_pnl.unwrap_or(0.0), chain.native),
        alert_note(receipt.alerts_fired)
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

