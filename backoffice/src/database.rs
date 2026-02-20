//! Database module for the Velocidad trading bot backend.
//!
//! This module provides database connectivity, models, and data access patterns
//! using SQLx with PostgreSQL.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::prelude::*;

/// Database connection pool wrapper
#[derive(Debug, Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Create a new database connection pool
    pub async fn new(config: &Config) -> Result<Self> {
        let db_config = &config.database;

        let connect_options = PgConnectOptions::new()
            .application_name("velocidad-trading-bot")
            .connect_timeout(Duration::from_secs(db_config.connect_timeout_secs));

        let pool = PgPoolOptions::new()
            .max_connections(db_config.max_connections)
            .min_connections(db_config.min_connections)
            .idle_timeout(Duration::from_secs(db_config.idle_timeout_secs))
            .max_lifetime(Duration::from_secs(db_config.max_lifetime_secs))
            .connect_with(connect_options)
            .await
            .map_err(|e| Error::Database(e))?;

        // Test the connection
        sqlx::query("SELECT 1")
            .execute(&pool)
            .await
            .map_err(|e| Error::Database(e))?;

        tracing::info!(
            "Database connection pool established (max_connections: {})",
            db_config.max_connections
        );

        Ok(Self { pool })
    }

    /// Get a reference to the connection pool
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Begin a transaction
    pub async fn begin(&self) -> Result<Transaction<'static, Postgres>> {
        self.pool
            .begin()
            .await
            .map_err(|e| Error::Database(e))
    }

    /// Run database migrations
    pub async fn run_migrations(&self) -> Result<()> {
        tracing::info!("Running database migrations...");

        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| Error::Migration(e.to_string()))?;

        tracing::info!("Database migrations completed successfully");
        Ok(())
    }
}

/// Database models

/// User model
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    /// Unique user ID
    pub id: Uuid,
    /// Telegram user ID (nullable for web-only users)
    pub telegram_id: Option<i64>,
    /// Wallet address for the primary chain
    pub wallet_address: Option<String>,
    /// Encrypted private key (encrypted with user password)
    pub encrypted_private_key: Option<Vec<u8>>,
    /// User settings as JSON
    pub settings: serde_json::Value,
    /// When the user was created
    pub created_at: DateTime<Utc>,
    /// When the user was last updated
    pub updated_at: DateTime<Utc>,
    /// When the user was deleted (soft delete)
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Trading strategy model
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Strategy {
    /// Unique strategy ID
    pub id: Uuid,
    /// User who owns this strategy
    pub user_id: Uuid,
    /// Strategy name
    pub name: String,
    /// Strategy type
    pub strategy_type: StrategyType,
    /// Strategy configuration as JSON
    pub config: serde_json::Value,
    /// Whether the strategy is active
    pub is_active: bool,
    /// Performance metrics as JSON
    pub performance_metrics: serde_json::Value,
    /// When the strategy was created
    pub created_at: DateTime<Utc>,
    /// When the strategy was last updated
    pub updated_at: DateTime<Utc>,
}

/// Strategy type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "strategy_type", rename_all = "snake_case")]
pub enum StrategyType {
    /// Token sniping strategy
    Sniping,
    /// Arbitrage strategy
    Arbitrage,
    /// High-frequency trading strategy
    Hft,
    /// Market making strategy
    MarketMaking,
    /// Mean reversion strategy
    MeanReversion,
    /// Momentum trading strategy
    Momentum,
}

/// Trade model
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Trade {
    /// Unique trade ID
    pub id: Uuid,
    /// User who executed the trade
    pub user_id: Uuid,
    /// Strategy used (if any)
    pub strategy_id: Option<Uuid>,
    /// Chain ID (Ethereum = 1, Solana = 0, etc.)
    pub chain_id: i32,
    /// Input token address
    pub token_in: String,
    /// Output token address
    pub token_out: String,
    /// Amount of input token (in smallest units)
    pub amount_in: rust_decimal::Decimal,
    /// Amount of output token (in smallest units)
    pub amount_out: rust_decimal::Decimal,
    /// Gas used (for EVM chains)
    pub gas_used: Option<i64>,
    /// Gas price (for EVM chains)
    pub gas_price: Option<rust_decimal::Decimal>,
    /// Transaction hash
    pub tx_hash: Option<String>,
    /// Trade status
    pub status: TradeStatus,
    /// When the trade was executed
    pub execution_time: DateTime<Utc>,
    /// Profit/loss in USD
    pub profit_loss: Option<rust_decimal::Decimal>,
    /// Slippage percentage
    pub slippage_percent: Option<rust_decimal::Decimal>,
    /// Additional trade metadata as JSON
    pub metadata: serde_json::Value,
    /// When the trade was created
    pub created_at: DateTime<Utc>,
}

/// Trade status enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "trade_status", rename_all = "snake_case")]
pub enum TradeStatus {
    /// Trade is pending execution
    Pending,
    /// Trade is being executed
    Executing,
    /// Trade executed successfully
    Executed,
    /// Trade failed
    Failed,
    /// Trade was reverted
    Reverted,
    /// Trade was cancelled
    Cancelled,
}

/// Market data model (time-series)
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MarketData {
    /// Token address
    pub token_address: String,
    /// DEX name
    pub dex: String,
    /// Price in USD
    pub price: rust_decimal::Decimal,
    /// Liquidity in USD
    pub liquidity: rust_decimal::Decimal,
    /// 24-hour volume in USD
    pub volume_24h: Option<rust_decimal::Decimal>,
    /// Timestamp of the data point
    pub timestamp: DateTime<Utc>,
}

/// Alert model
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Alert {
    /// Unique alert ID
    pub id: Uuid,
    /// User who created the alert
    pub user_id: Uuid,
    /// Alert type
    pub alert_type: AlertType,
    /// Token address to monitor
    pub token_address: String,
    /// Trigger condition as JSON
    pub condition: serde_json::Value,
    /// Whether the alert is active
    pub is_active: bool,
    /// Last triggered time (if any)
    pub last_triggered_at: Option<DateTime<Utc>>,
    /// When the alert was created
    pub created_at: DateTime<Utc>,
    /// When the alert was last updated
    pub updated_at: DateTime<Utc>,
}

/// Alert type enum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "alert_type", rename_all = "snake_case")]
pub enum AlertType {
    /// Price reaches a certain level
    Price,
    /// Volume exceeds a threshold
    Volume,
    /// Liquidity reaches a level
    Liquidity,
    /// Token listed on a new DEX
    NewListing,
    /// Large wallet movement
    WhaleMovement,
}

/// Portfolio position model
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PortfolioPosition {
    /// Unique position ID
    pub id: Uuid,
    /// User who owns this position
    pub user_id: Uuid,
    /// Chain ID
    pub chain_id: i32,
    /// Token address
    pub token_address: String,
    /// Token amount (in smallest units)
    pub amount: rust_decimal::Decimal,
    /// Average entry price in USD
    pub avg_entry_price: rust_decimal::Decimal,
    /// Current market price in USD
    pub current_price: rust_decimal::Decimal,
    /// Unrealized P&L in USD
    pub unrealized_pnl: rust_decimal::Decimal,
    /// When the position was opened
    pub opened_at: DateTime<Utc>,
    /// When the position was last updated
    pub updated_at: DateTime<Utc>,
}

/// Repository traits

/// User repository for data access
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Create a new user
    async fn create(&self, user: CreateUser) -> Result<User>;

    /// Find a user by ID
    async fn find_by_id(&self, id: Uuid) -> Result<Option<User>>;

    /// Find a user by Telegram ID
    async fn find_by_telegram_id(&self, telegram_id: i64) -> Result<Option<User>>;

    /// Find a user by wallet address
    async fn find_by_wallet_address(&self, wallet_address: &str) -> Result<Option<User>>;

    /// Update user settings
    async fn update_settings(&self, id: Uuid, settings: serde_json::Value) -> Result<User>;

    /// Soft delete a user
    async fn delete(&self, id: Uuid) -> Result<()>;
}

/// Create user DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateUser {
    /// Telegram user ID (optional)
    pub telegram_id: Option<i64>,
    /// Wallet address (optional)
    pub wallet_address: Option<String>,
    /// Initial settings
    pub settings: serde_json::Value,
}

/// Trade repository for data access
#[async_trait]
pub trait TradeRepository: Send + Sync {
    /// Create a new trade
    async fn create(&self, trade: CreateTrade) -> Result<Trade>;

    /// Find a trade by ID
    async fn find_by_id(&self, id: Uuid) -> Result<Option<Trade>>;

    /// Find trades by user ID with pagination
    async fn find_by_user_id(
        &self,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Trade>>;

    /// Update trade status
    async fn update_status(&self, id: Uuid, status: TradeStatus) -> Result<Trade>;

    /// Update trade with transaction hash
    async fn update_with_tx_hash(
        &self,
        id: Uuid,
        tx_hash: String,
        gas_used: Option<i64>,
        gas_price: Option<rust_decimal::Decimal>,
    ) -> Result<Trade>;

    /// Update trade profit/loss
    async fn update_profit_loss(&self, id: Uuid, profit_loss: rust_decimal::Decimal) -> Result<Trade>;
}

/// Create trade DTO
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTrade {
    /// User ID
    pub user_id: Uuid,
    /// Strategy ID (optional)
    pub strategy_id: Option<Uuid>,
    /// Chain ID
    pub chain_id: i32,
    /// Input token address
    pub token_in: String,
    /// Output token address
    pub token_out: String,
    /// Amount of input token
    pub amount_in: rust_decimal::Decimal,
    /// Amount of output token
    pub amount_out: rust_decimal::Decimal,
    /// Trade status
    pub status: TradeStatus,
    /// Slippage percentage
    pub slippage_percent: Option<rust_decimal::Decimal>,
    /// Additional metadata
    pub metadata: serde_json::Value,
}

/// Market data repository for data access
#[async_trait]
pub trait MarketDataRepository: Send + Sync {
    /// Insert or update market data
    async fn upsert(&self, data: Vec<MarketData>) -> Result<()>;

    /// Get latest price for a token
    async fn get_latest_price(&self, token_address: &str, dex: Option<&str>) -> Result<Option<MarketData>>;

    /// Get price history for a token
    async fn get_price_history(
        &self,
        token_address: &str,
        dex: Option<&str>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        interval: &str,
    ) -> Result<Vec<MarketData>>;

    /// Search tokens by symbol or name
    async fn search_tokens(&self, query: &str, limit: i64) -> Result<Vec<MarketData>>;
}

/// SQLx implementations

/// SQLx user repository implementation
pub struct SqlxUserRepository {
    pool: PgPool,
}

impl SqlxUserRepository {
    /// Create a new SQLx user repository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn create(&self, user: CreateUser) -> Result<User> {
        let record = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (telegram_id, wallet_address, settings)
            VALUES ($1, $2, $3)
            RETURNING *
            "#,
        )
        .bind(user.telegram_id)
        .bind(user.wallet_address)
        .bind(user.settings)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(record)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT * FROM users
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(user)
    }

    async fn find_by_telegram_id(&self, telegram_id: i64) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT * FROM users
            WHERE telegram_id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(telegram_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(user)
    }

    async fn find_by_wallet_address(&self, wallet_address: &str) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT * FROM users
            WHERE wallet_address = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(wallet_address)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(user)
    }

    async fn update_settings(&self, id: Uuid, settings: serde_json::Value) -> Result<User> {
        let user = sqlx::query_as::<_, User>(
            r#"
            UPDATE users
            SET settings = $2, updated_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(settings)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(user)
    }

    async fn delete(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE users
            SET deleted_at = NOW()
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(())
    }
}

/// SQLx trade repository implementation
pub struct SqlxTradeRepository {
    pool: PgPool,
}

impl SqlxTradeRepository {
    /// Create a new SQLx trade repository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl TradeRepository for SqlxTradeRepository {
    async fn create(&self, trade: CreateTrade) -> Result<Trade> {
        let record = sqlx::query_as::<_, Trade>(
            r#"
            INSERT INTO trades (
                user_id, strategy_id, chain_id, token_in, token_out,
                amount_in, amount_out, status, slippage_percent, metadata
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING *
            "#,
        )
        .bind(trade.user_id)
        .bind(trade.strategy_id)
        .bind(trade.chain_id)
        .bind(trade.token_in)
        .bind(trade.token_out)
        .bind(trade.amount_in)
        .bind(trade.amount_out)
        .bind(trade.status)
        .bind(trade.slippage_percent)
        .bind(trade.metadata)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(record)
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<Trade>> {
        let trade = sqlx::query_as::<_, Trade>(
            r#"
            SELECT * FROM trades
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(trade)
    }

    async fn find_by_user_id(
        &self,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Trade>> {
        let trades = sqlx::query_as::<_, Trade>(
            r#"
            SELECT * FROM trades
            WHERE user_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(trades)
    }

    async fn update_status(&self, id: Uuid, status: TradeStatus) -> Result<Trade> {
        let trade = sqlx::query_as::<_, Trade>(
            r#"
            UPDATE trades
            SET status = $2
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(status)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(trade)
    }

    async fn update_with_tx_hash(
        &self,
        id: Uuid,
        tx_hash: String,
        gas_used: Option<i64>,
        gas_price: Option<rust_decimal::Decimal>,
    ) -> Result<Trade> {
        let trade = sqlx::query_as::<_, Trade>(
            r#"
            UPDATE trades
            SET tx_hash = $2, gas_used = $3, gas_price = $4
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(tx_hash)
        .bind(gas_used)
        .bind(gas_price)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(trade)
    }

    async fn update_profit_loss(&self, id: Uuid, profit_loss: rust_decimal::Decimal) -> Result<Trade> {
        let trade = sqlx::query_as::<_, Trade>(
            r#"
            UPDATE trades
            SET profit_loss = $2
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(profit_loss)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Database(e))?;

        Ok(trade)
    }
}

/// Database error types
#[derive(Error, Debug)]
pub enum DatabaseError {
    /// Connection error
    #[error("Database connection error: {0}")]
    Connection(String),

    /// Query execution error
    #[error("Query execution error: {0}")]
    QueryExecution(String),

    /// Migration error
    #[error("Migration error: {0}")]
    Migration(String),

    /// Constraint violation
    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    /// Data not found
    #[error("Data not found: {0}")]
    NotFound(String),

    /// Transaction error
    #[error("Transaction error: {0}")]
    Transaction(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_strategy_type_serialization() {
        let strategy_type = StrategyType::Sniping;
        let serialized = serde_json::to_string(&strategy_type).unwrap();
        assert_eq!(serialized, "\"sniping\"");

        let deserialized: StrategyType = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, StrategyType::Sniping);
    }

    #[test]
    fn test_trade_status_serialization() {
        let status = TradeStatus::Executed;
        let serialized = serde_json::to_string(&status).unwrap();
        assert_eq!(serialized, "\"executed\"");

        let deserialized: TradeStatus = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, TradeStatus::Executed);
    }

    #[test]
    fn test_alert_type_serialization() {
        let alert_type = AlertType::Price;
        let serialized = serde_json::to_string(&alert_type).unwrap();
        assert_eq!(serialized, "\"price\"");

        let deserialized: AlertType = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, AlertType::Price);
    }

    #[test]
    fn test_create_user_dto() {
        let create_user = CreateUser {
            telegram_id: Some(123456789),
            wallet_address: Some("0x1234abcd".to_string()),
            settings: serde_json::json!({
                "notifications_enabled": true,
                "theme": "dark"
            }),
        };

        let serialized = serde_json::to_string(&create_user).unwrap();
        let deserialized: CreateUser = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.telegram_id, Some(123456789));
        assert_eq!(deserialized.wallet_address, Some("0x1234abcd".to_string()));
    }

    #[test]
    fn test_create_trade_dto() {
        let create_trade = CreateTrade {
            user_id: Uuid::new_v4(),
            strategy_id: Some(Uuid::new_v4()),
            chain_id: 1,
            token_in: "0xTokenIn".to_string(),
            token_out: "0xTokenOut".to_string(),
            amount_in: rust_decimal::Decimal::from_str("1000000").unwrap(),
            amount_out: rust_decimal::Decimal::from_str("2000000").unwrap(),
            status: TradeStatus::Pending,
            slippage_percent: Some(rust_decimal::Decimal::from_str("0.5").unwrap()),
            metadata: serde_json::json!({
                "route": ["0xPool1", "0xPool2"],
                "dex": "uniswap_v3"
            }),
        };

        let serialized = serde_json::to_string(&create_trade).unwrap();
        let deserialized: CreateTrade = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.chain_id, 1);
        assert_eq!(deserialized.token_in, "0xTokenIn");
        assert_eq!(deserialized.status, TradeStatus::Pending);
    }
}
