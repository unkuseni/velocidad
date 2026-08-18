//! Velocidad — Trojan-style multi-chain Telegram trading bot backed by libSQL / Turso.
//!
//! Entry point: loads config, opens the database, then runs the Telegram bot,
//! the HTTP API and the background workers (limit matching + alerts) concurrently.

mod api;
mod app;
mod bot;
mod chains;
mod config;
mod crypto;
mod db;
mod eip7702;
mod market;
mod rpc;
mod security;
mod solana;
mod swap;
mod trading;
mod workers;

use std::sync::Arc;

use anyhow::Context;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = config::Config::load().context("failed to load configuration")?;
    tracing::info!(
        database = %config.database_url,
        remote = config.is_remote(),
        api_port = config.api_port,
        paper_trading = config.paper_trading,
        live_market = config.live_market,
        default_chain = %config.default_chain,
        live_swaps = config.zeroex_api_key.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
        "velocidad starting"
    );

    let db = db::Db::open(&config)
        .await
        .context("failed to open libSQL database")?;
    let keyring = crypto::Keyring::load(&config).context("failed to load keyring")?;
    let state = Arc::new(app::AppState::new(db, keyring, config));

    workers::spawn(Arc::clone(&state));

    // The Telegram bot runs as a task: if it dies (bad token, network split),
    // the HTTP API and workers keep serving. Log the failure, don't crash.
    let bot_state = Arc::clone(&state);
    tokio::spawn(async move {
        match bot::run(bot_state).await {
            Ok(()) => tracing::warn!("telegram bot stopped cleanly"),
            Err(e) => tracing::error!(error = %e, "telegram bot terminated"),
        }
    });

    let api_port = state.config.api_port;
    api::serve(state, api_port)
        .await
        .context("HTTP API terminated")
}
