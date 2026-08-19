//! Shared application state passed to the Telegram bot, HTTP API and workers.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::crypto::Keyring;
use crate::db::Db;
use crate::market::MarketData;
use crate::rate::RateLimiter;
use crate::rpc::RpcClient;
use crate::security::TokenScanner;
use crate::solana::SolanaClient;
use crate::swap::SwapClient;
use crate::trading::TradingEngine;

pub struct AppState {
    pub db: Arc<Db>,
    pub engine: TradingEngine,
    pub market: Arc<MarketData>,
    pub scanner: TokenScanner,
    pub rpc: RpcClient,
    pub swap: SwapClient,
    pub solana: SolanaClient,
    pub keyring: Arc<Keyring>,
    pub config: Config,
    pub bot_rate: Arc<RateLimiter>,
    pub api_rate: Arc<RateLimiter>,
    /// True while the Telegram bot task is running (readiness).
    pub bot_alive: Arc<AtomicBool>,
    /// Last tick of either background worker (readiness).
    pub worker_heartbeat: Arc<Mutex<std::time::Instant>>,
    /// Idempotency-Key -> cached trade response (short TTL, pruned on write).
    pub idempotency:
        Arc<Mutex<std::collections::HashMap<String, (std::time::Instant, serde_json::Value)>>>,
}

impl AppState {
    pub fn new(db: Db, keyring: Keyring, config: Config) -> Self {
        let market = Arc::new(MarketData::new(config.live_market));
        Self {
            db: Arc::new(db),
            engine: TradingEngine::new(Arc::clone(&market)),
            market: Arc::clone(&market),
            scanner: TokenScanner::new(market, config.honeypot_api_key.clone()),
            rpc: RpcClient::new(),
            swap: SwapClient::new(
                config.zeroex_api_key.clone(),
                parse_sponsor_key(&config.sponsor_key),
            ),
            solana: SolanaClient::new(parse_solana_sponsor(&config.sponsor_solana_key)),
            keyring: Arc::new(keyring),
            bot_rate: Arc::new(RateLimiter::new(&[(
                config.bot_rate_per_min,
                std::time::Duration::from_secs(60),
            )])),
            api_rate: Arc::new(RateLimiter::new(&[(
                config.api_rate_per_min,
                std::time::Duration::from_secs(60),
            )])),
            bot_alive: Arc::new(AtomicBool::new(false)),
            worker_heartbeat: Arc::new(Mutex::new(std::time::Instant::now())),
            idempotency: Arc::new(Mutex::new(std::collections::HashMap::new())),
            config,
        }
    }

    /// Whether a Telegram user may run operator-only commands.
    pub fn is_admin(&self, telegram_id: i64) -> bool {
        self.config
            .admin_telegram_ids
            .as_deref()
            .map(|ids| {
                ids.split(',')
                    .any(|id| id.trim().parse::<i64>().ok() == Some(telegram_id))
            })
            .unwrap_or(false)
    }

    /// Resolve the user's default chain (falling back to config default).
    pub async fn user_chain(&self, user_id: i64) -> &'static crate::chains::Chain {
        match crate::db::repo::user_chain(self.db.conn(), user_id, &self.config.default_chain).await
        {
            Ok(id) => crate::chains::by_id(&id).unwrap_or_else(|| default_chain(&self.config)),
            Err(_) => default_chain(&self.config),
        }
    }
}

/// Parse the EVM sponsor operator key (64 hex chars) into (address, secret).
fn parse_sponsor_key(key: &Option<String>) -> Option<(String, [u8; 32])> {
    let hex_key = key.as_ref()?.trim();
    if hex_key.is_empty() {
        return None;
    }
    let bytes = hex::decode(hex_key.strip_prefix("0x").unwrap_or(hex_key)).ok()?;
    let secret: [u8; 32] = bytes.try_into().ok()?;
    let address = crate::crypto::derive_address(&secret).ok()?;
    Some((address, secret))
}

/// Parse the Solana sponsor operator key (base58/hex seed) into a SponsorCtx.
fn parse_solana_sponsor(key: &Option<String>) -> Option<crate::solana::SponsorCtx> {
    let k = key.as_ref()?.trim();
    if k.is_empty() {
        return None;
    }
    let bytes =
        if k.starts_with("0x") || (k.len() == 64 && k.chars().all(|c| c.is_ascii_hexdigit())) {
            hex::decode(k.trim_start_matches("0x")).ok()?
        } else {
            bs58::decode(k).into_vec().ok()?
        };
    let seed: [u8; 32] = match bytes.len() {
        32 => bytes.try_into().ok()?,
        64 => bytes[..32].try_into().ok()?,
        _ => return None,
    };
    Some(crate::solana::SponsorCtx::from_seed(seed))
}

fn default_chain(config: &Config) -> &'static crate::chains::Chain {
    crate::chains::by_id(&config.default_chain).unwrap_or(crate::chains::by_id("ethereum").unwrap())
}
