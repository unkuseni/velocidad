//! Velocidad — Trojan-style Telegram trading bot backed by libSQL / Turso.
//!
//! Entry point: loads config, opens the database, then runs the Telegram bot
//! and the HTTP API concurrently.

mod api;
mod app;
mod bot;
mod config;
mod crypto;
mod db;
mod security;
mod trading;

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
        "velocidad starting"
    );

    let db = db::Db::open(&config)
        .await
        .context("failed to open libSQL database")?;
    let keyring = crypto::Keyring::load(&config).context("failed to load keyring")?;
    let state = Arc::new(app::AppState::new(db, keyring, config));

    let api_state = Arc::clone(&state);
    let bot_state = Arc::clone(&state);

    tokio::select! {
        api = api::serve(api_state, state.config.api_port) => {
            api.context("HTTP API terminated")?;
        }
        bot = bot::run(bot_state) => {
            bot.context("telegram bot terminated")?;
        }
    }

    Ok(())
}
