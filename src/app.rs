//! Shared application state passed to the Telegram bot, HTTP API and workers.

use std::sync::Arc;

use crate::config::Config;
use crate::crypto::Keyring;
use crate::db::Db;
use crate::market::MarketData;
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
            swap: SwapClient::new(config.zeroex_api_key.clone()),
            solana: SolanaClient::new(),
            keyring: Arc::new(keyring),
            config,
        }
    }

    /// Resolve the user's default chain (falling back to config default).
    pub async fn user_chain(&self, user_id: i64) -> &'static crate::chains::Chain {
        match crate::db::repo::user_chain(self.db.conn(), user_id, &self.config.default_chain).await {
            Ok(id) => crate::chains::by_id(&id).unwrap_or_else(|| default_chain(&self.config)),
            Err(_) => default_chain(&self.config),
        }
    }
}

fn default_chain(config: &Config) -> &'static crate::chains::Chain {
    crate::chains::by_id(&config.default_chain).unwrap_or(crate::chains::by_id("ethereum").unwrap())
}
