//! Shared application state passed to both the Telegram bot and the HTTP API.

use std::sync::Arc;

use crate::config::Config;
use crate::crypto::Keyring;
use crate::db::Db;
use crate::security::TokenScanner;
use crate::trading::TradingEngine;

pub struct AppState {
    pub db: Arc<Db>,
    pub engine: TradingEngine,
    pub scanner: TokenScanner,
    pub keyring: Arc<Keyring>,
    pub config: Config,
}

impl AppState {
    pub fn new(db: Db, keyring: Keyring, config: Config) -> Self {
        Self {
            db: Arc::new(db),
            engine: TradingEngine::new(),
            scanner: TokenScanner::new(),
            keyring: Arc::new(keyring),
            config,
        }
    }
}
