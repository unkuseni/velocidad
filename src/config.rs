//! Application configuration.
//!
//! Loaded from environment variables / `.env` via `figment`:
//! - `DATABASE_URL`      — `file:./velocidad.db` (local libSQL) or `libsql://<db>.turso.io` (Turso)
//! - `TURSO_AUTH_TOKEN`  — auth token for remote Turso databases
//! - `TELOXIDE_TOKEN`    — Telegram bot token from @BotFather
//! - `API_PORT`          — HTTP API port (default `8080`)
//! - `PAPER_TRADING`     — `true` = simulated fills, no real blockchain calls (default `true`)
//! - `MASTER_KEY`        — 64-hex-char AES key used to encrypt wallet private keys.
//!   If absent, a key is generated and persisted to `./velocidad.key`.

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

    /// Optional bearer token protecting the HTTP API (/api/v1/*). When unset,
    /// the API runs open (dev mode) with a startup warning.
    #[serde(default)]
    pub api_key: Option<String>,

    /// Max bot commands per user per minute (sliding window; 0 disables).
    #[serde(default = "default_bot_rate")]
    pub bot_rate_per_min: u64,

    /// Max HTTP API requests per IP per minute (sliding window; 0 disables).
    #[serde(default = "default_api_rate")]
    pub api_rate_per_min: u64,
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

fn default_bot_rate() -> u64 {
    30
}

fn default_api_rate() -> u64 {
    120
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
                "API_KEY",
                "BOT_RATE_PER_MIN",
                "API_RATE_PER_MIN",
            ]))
            .merge(Env::prefixed("VELOCIDAD_").split("_"))
            .extract()?;
        config.validate();
        Ok(config)
    }

    /// Non-fatal startup warnings for misconfigurations that would otherwise
    /// surface as confusing runtime errors.
    pub fn validate(&self) {
        if !self.paper_trading
            && self
                .zeroex_api_key
                .as_deref()
                .map(|k| k.trim().is_empty())
                .unwrap_or(true)
        {
            tracing::warn!(
                "PAPER_TRADING=false but ZEROEX_API_KEY is not set — live EVM swaps will fail"
            );
        }
        if self.paper_trading && (self.sponsor_key.is_some() || self.sponsor_solana_key.is_some()) {
            tracing::warn!(
                "gas-sponsorship keys are set but PAPER_TRADING=true — sponsorship is unused"
            );
        }
        if self.sponsor_key.is_some() && self.zeroex_api_key.is_none() {
            tracing::warn!(
                "SPONSOR_KEY is set but ZEROEX_API_KEY is missing — sponsored EVM swaps will fail"
            );
        }
        if !self.default_chain.trim().is_empty()
            && crate::chains::by_id(&self.default_chain.trim().to_lowercase()).is_none()
        {
            tracing::warn!(
                chain = %self.default_chain,
                "DEFAULT_CHAIN is not a known chain — falling back to ethereum"
            );
        }
        if self.teloxide_token.trim().is_empty() {
            tracing::warn!("TELOXIDE_TOKEN is empty — the Telegram bot is disabled");
        }
        if self
            .api_key
            .as_deref()
            .map(|k| k.trim().is_empty())
            .unwrap_or(true)
        {
            tracing::warn!(
                "API_KEY is not set — the HTTP API runs open. Set API_KEY to require a bearer token."
            );
        }
    }

    /// True when the configured database is a remote Turso instance.
    pub fn is_remote(&self) -> bool {
        self.database_url.starts_with("libsql://")
    }
}
