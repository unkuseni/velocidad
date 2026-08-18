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
mod exec;
mod market;
mod rate;
mod rpc;
mod security;
mod solana;
mod swap;
mod trading;
mod workers;

use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use futures::FutureExt;
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

    // The Telegram bot runs as a supervised task: a panic (e.g. teloxide's
    // invalid-token panic) or an error restarts it with exponential backoff.
    // The HTTP API and workers keep serving meanwhile; /health reports the
    // bot's readiness so operators see the failure instead of silence.
    let bot_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut backoff = 2u64;
        loop {
            bot_state.bot_alive.store(true, AtomicOrdering::Relaxed);
            let result = std::panic::AssertUnwindSafe(bot::run(Arc::clone(&bot_state)))
                .catch_unwind()
                .await;
            match result {
                Ok(Ok(())) => {
                    tracing::warn!("telegram bot stopped cleanly");
                    break;
                }
                Ok(Err(e)) => {
                    tracing::error!(error = %e, backoff, "telegram bot terminated — restarting");
                }
                Err(_) => {
                    tracing::error!(backoff, "telegram bot panicked — restarting");
                }
            }
            bot_state.bot_alive.store(false, AtomicOrdering::Relaxed);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(60);
        }
    });

    let api_port = state.config.api_port;
    api::serve(state, api_port)
        .await
        .context("HTTP API terminated")?;
    tracing::info!("shutdown complete");
    Ok(())
}

/// Resolve on Ctrl+C or SIGTERM so the HTTP API can drain gracefully.
pub async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {},
            _ = term.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
