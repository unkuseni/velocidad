//! Application configuration.
//!
//! Loaded from environment variables / `.env` via `figment`:
//! - `DATABASE_URL`      — `file:./velocidad.db` (local libSQL) or `libsql://<db>.turso.io` (Turso)
//! - `TURSO_AUTH_TOKEN`  — auth token for remote Turso databases
//! - `TELOXIDE_TOKEN`    — Telegram bot token from @BotFather
//! - `API_PORT`          — HTTP API port (default `8080`)
//! - `PAPER_TRADING`     — `true` = simulated fills, no real blockchain calls (default `true`)
//! - `MASTER_KEY`        — 64-hex-char AES key used to encrypt wallet private keys.
//!                         If absent, a key is generated and persisted to `./velocidad.key`.

use figment::providers::Env;
use figment::Figment;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// libSQL database. Either a local path (`file:./velocidad.db`) or a Turso
    /// URL (`libsql://my-db.turso.io`).
    #[serde(default = "default_database_url")]
    pub database_url: String,

    /// Auth token required when `database_url` is a remote Turso URL.
    #[serde(default)]
    pub turso_auth_token: Option<String>,

    /// Telegram bot token from @BotFather.
    #[serde(default)]
    pub teloxide_token: String,

    /// Port for the Axum HTTP API.
    #[serde(default = "default_api_port")]
    pub api_port: u16,

    /// When `true`, trades are simulated against a mock market (no RPC calls).
    #[serde(default = "default_true")]
    pub paper_trading: bool,

    /// Optional 64-hex-char master key for encrypting wallet private keys.
    #[serde(default)]
    pub master_key: Option<String>,

    /// Default EVM chain for new users and chain-less commands (e.g. `bsc`).
    #[serde(default = "default_chain")]
    pub default_chain: String,

    /// When `true`, prices/liquidity come from DexScreener (with an offline
    /// simulator fallback). Set `false` for fully deterministic paper trading.
    #[serde(default = "default_true")]
    pub live_market: bool,

    /// 0x Swap API v2 key — enables real on-chain swaps (`/buy`, `/sell`).
    #[serde(default)]
    pub zeroex_api_key: Option<String>,

    /// Optional honeypot.is API key for live honeypot simulations in /scan.
    #[serde(default)]
    pub honeypot_api_key: Option<String>,

    /// EIP-7702 gas-sponsorship operator key (64 hex chars, EVM). The derived
    /// address pays gas for delegated user executions; fund it per chain.
    #[serde(default)]
    pub sponsor_key: Option<String>,

    /// Solana gas-sponsorship operator key (base58 or hex seed). The derived
    /// address is used as the fee payer and SPL delegate.
    #[serde(default)]
    pub sponsor_solana_key: Option<String>,
}

fn default_chain() -> String {
    "ethereum".to_string()
}

fn default_database_url() -> String {
    "file:./velocidad.db".to_string()
}

fn default_api_port() -> u16 {
    8080
}

fn default_true() -> bool {
    true
}

impl Config {
    /// Load configuration from process env + `.env` (explicit vars win).
    ///
    /// `dotenvy` (in `main`) loads `.env` into the process environment first,
    /// so plain `Env` providers see those values too.
    pub fn load() -> anyhow::Result<Self> {
        let config: Config = Figment::new()
            .merge(Env::raw().only(&[
                "DATABASE_URL",
                "TURSO_AUTH_TOKEN",
                "TELOXIDE_TOKEN",
                "API_PORT",
                "PAPER_TRADING",
                "MASTER_KEY",
                "DEFAULT_CHAIN",
                "LIVE_MARKET",
                "ZEROEX_API_KEY",
                "HONEYPOT_API_KEY",
                "SPONSOR_KEY",
                "SPONSOR_SOLANA_KEY",
            ]))
            .merge(Env::prefixed("VELOCIDAD_").split("_"))
            .extract()?;
        Ok(config)
    }

    /// True when the configured database is a remote Turso instance.
    pub fn is_remote(&self) -> bool {
        self.database_url.starts_with("libsql://")
    }
}
