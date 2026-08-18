//! Row models for the libSQL database. All types implement `Serialize` so they
//! can be returned directly from the HTTP API and logged by the bot.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub telegram_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Wallet {
    pub id: i64,
    pub user_id: i64,
    pub address: String,
    pub network: String,
    pub label: String,
    /// Encrypted private key (hex). `None` for imported watch-only wallets.
    /// Kept out of API responses; used later for signing live transactions.
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub encrypted_key: Option<String>,
    pub is_default: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Token {
    pub address: String,
    pub network: String,
    pub name: Option<String>,
    pub symbol: Option<String>,
    pub decimals: i64,
    pub risk_score: i64,
    pub is_honeypot: bool,
    pub liquidity: Option<f64>,
    pub last_price: Option<f64>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Order {
    pub id: i64,
    pub user_id: i64,
    pub wallet_id: Option<i64>,
    pub network: String,
    pub token_address: String,
    pub side: String,
    pub amount_in: Option<f64>,
    pub amount_out: Option<f64>,
    pub price: Option<f64>,
    pub slippage: f64,
    pub status: String,
    pub tx_hash: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub executed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub id: i64,
    pub user_id: i64,
    pub wallet_id: Option<i64>,
    pub token_address: String,
    pub network: String,
    pub quantity: f64,
    pub avg_price: f64,
    pub realized_pnl: f64,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub id: i64,
    pub user_id: i64,
    pub network: String,
    pub token_address: String,
    pub condition: String,
    pub target_price: f64,
    pub is_triggered: bool,
    pub created_at: String,
    pub triggered_at: Option<String>,
}
