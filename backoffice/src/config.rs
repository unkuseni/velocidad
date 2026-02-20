//! Configuration module for the Velocidad trading bot backend.
//!
//! This module defines the configuration structure for the entire application,
//! with support for multiple environments (development, staging, production),
//! environment variable overrides, and configuration validation.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use figment::{
    providers::{Env, Format, Json, Toml, Yaml},
    Figment, Profile,
};
use serde::{Deserialize, Serialize};

/// Application environment
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Environment {
    /// Development environment
    #[serde(rename = "development")]
    Development,
    /// Staging environment
    #[serde(rename = "staging")]
    Staging,
    /// Production environment
    #[serde(rename = "production")]
    Production,
}

impl Default for Environment {
    fn default() -> Self {
        Self::Development
    }
}

impl Environment {
    /// Get the environment as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Environment::Development => "development",
            Environment::Staging => "staging",
            Environment::Production => "production",
        }
    }

    /// Check if running in development
    pub fn is_development(&self) -> bool {
        matches!(self, Environment::Development)
    }

    /// Check if running in production
    pub fn is_production(&self) -> bool {
        matches!(self, Environment::Production)
    }
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL
    pub url: String,
    /// Maximum number of connections in the pool
    #[serde(default = "default_pool_max_size")]
    pub max_connections: u32,
    /// Minimum number of connections in the pool
    #[serde(default = "default_pool_min_size")]
    pub min_connections: u32,
    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connect_timeout_secs: u64,
    /// Connection idle timeout in seconds
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
    /// Maximum lifetime of a connection in seconds
    #[serde(default = "default_max_lifetime")]
    pub max_lifetime_secs: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgresql://localhost:5432/velocidad".to_string(),
            max_connections: default_pool_max_size(),
            min_connections: default_pool_min_size(),
            connect_timeout_secs: default_connection_timeout(),
            idle_timeout_secs: default_idle_timeout(),
            max_lifetime_secs: default_max_lifetime(),
        }
    }
}

/// Redis configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    /// Redis connection URL
    pub url: String,
    /// Maximum number of connections in the pool
    #[serde(default = "default_redis_pool_size")]
    pub max_connections: u32,
    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connect_timeout_secs: u64,
    /// Read timeout in seconds
    #[serde(default = "default_read_timeout")]
    pub read_timeout_secs: u64,
    /// Write timeout in seconds
    #[serde(default = "default_write_timeout")]
    pub write_timeout_secs: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://localhost:6379".to_string(),
            max_connections: default_redis_pool_size(),
            connect_timeout_secs: default_connection_timeout(),
            read_timeout_secs: default_read_timeout(),
            write_timeout_secs: default_write_timeout(),
        }
    }
}

/// Ethereum blockchain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthereumConfig {
    /// JSON-RPC endpoint URL
    pub rpc_url: String,
    /// WebSocket endpoint URL for real-time events
    pub ws_url: Option<String>,
    /// Chain ID
    pub chain_id: u64,
    /// Gas price oracle URL (optional)
    pub gas_price_oracle_url: Option<String>,
    /// Default gas limit for transactions
    #[serde(default = "default_gas_limit")]
    pub default_gas_limit: u64,
    /// Default priority fee (in wei)
    #[serde(default = "default_priority_fee")]
    pub default_priority_fee: u64,
    /// Maximum gas price (in wei)
    #[serde(default = "default_max_gas_price")]
    pub max_gas_price: u64,
    /// Transaction confirmation timeout in seconds
    #[serde(default = "default_tx_timeout")]
    pub transaction_timeout_secs: u64,
    /// Number of confirmations required
    #[serde(default = "default_confirmations")]
    pub required_confirmations: u32,
}

impl Default for EthereumConfig {
    fn default() -> Self {
        Self {
            rpc_url: "http://localhost:8545".to_string(),
            ws_url: Some("ws://localhost:8546".to_string()),
            chain_id: 1,
            gas_price_oracle_url: None,
            default_gas_limit: default_gas_limit(),
            default_priority_fee: default_priority_fee(),
            max_gas_price: default_max_gas_price(),
            transaction_timeout_secs: default_tx_timeout(),
            required_confirmations: default_confirmations(),
        }
    }
}

/// Solana blockchain configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaConfig {
    /// JSON-RPC endpoint URL
    pub rpc_url: String,
    /// WebSocket endpoint URL for real-time events
    pub ws_url: Option<String>,
    /// Commitment level
    #[serde(default = "default_solana_commitment")]
    pub commitment: String,
    /// Maximum retries for failed transactions
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Transaction confirmation timeout in seconds
    #[serde(default = "default_tx_timeout")]
    pub transaction_timeout_secs: u64,
}

impl Default for SolanaConfig {
    fn default() -> Self {
        Self {
            rpc_url: "http://localhost:8899".to_string(),
            ws_url: Some("ws://localhost:8900".to_string()),
            commitment: default_solana_commitment(),
            max_retries: default_max_retries(),
            transaction_timeout_secs: default_tx_timeout(),
        }
    }
}

/// Blockchain configuration for all supported chains
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlockchainConfig {
    /// Ethereum configuration
    pub ethereum: Option<EthereumConfig>,
    /// Solana configuration
    pub solana: Option<SolanaConfig>,
    /// Additional chain configurations (keyed by chain name)
    #[serde(default)]
    pub additional_chains: std::collections::HashMap<String, serde_json::Value>,
}

/// Telegram bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot API token
    pub token: String,
    /// Webhook URL (if using webhooks)
    pub webhook_url: Option<String>,
    /// Allowed user IDs (empty means all users are allowed)
    #[serde(default)]
    pub allowed_users: Vec<i64>,
    /// Admin user IDs
    #[serde(default)]
    pub admin_users: Vec<i64>,
    /// Log channel ID
    pub log_channel_id: Option<i64>,
    /// Alert channel ID
    pub alert_channel_id: Option<i64>,
    /// Maximum message length
    #[serde(default = "default_max_message_length")]
    pub max_message_length: usize,
}

/// API server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Host to bind to
    #[serde(default = "default_host")]
    pub host: IpAddr,
    /// Port to bind to
    #[serde(default = "default_api_port")]
    pub port: u16,
    /// CORS origins (empty means all origins)
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
    /// Request body size limit in bytes
    #[serde(default = "default_body_limit")]
    pub body_limit: usize,
    /// Rate limiting configuration
    pub rate_limit: Option<RateLimitConfig>,
    /// Enable/disable Swagger UI
    #[serde(default = "default_bool_true")]
    pub enable_swagger: bool,
    /// API key for external access (optional)
    pub api_key: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_api_port(),
            cors_origins: Vec::new(),
            request_timeout_secs: default_request_timeout(),
            body_limit: default_body_limit(),
            rate_limit: Some(RateLimitConfig::default()),
            enable_swagger: default_bool_true(),
            api_key: None,
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Requests per minute for normal users
    #[serde(default = "default_rpm_normal")]
    pub requests_per_minute: u32,
    /// Requests per minute for premium users
    #[serde(default = "default_rpm_premium")]
    pub requests_per_minute_premium: u32,
    /// Burst size (maximum requests in a short period)
    #[serde(default = "default_burst_size")]
    pub burst_size: u32,
    /// Rate limit key prefix in Redis
    #[serde(default = "default_rate_limit_prefix")]
    pub redis_prefix: String,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: default_rpm_normal(),
            requests_per_minute_premium: default_rpm_premium(),
            burst_size: default_burst_size(),
            redis_prefix: default_rate_limit_prefix(),
        }
    }
}

/// Trading engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    /// Default slippage tolerance percentage
    #[serde(default = "default_slippage")]
    pub default_slippage_percent: f64,
    /// Maximum slippage tolerance percentage
    #[serde(default = "default_max_slippage")]
    pub max_slippage_percent: f64,
    /// Minimum token liquidity (USD) to trade
    #[serde(default = "default_min_liquidity")]
    pub min_liquidity_usd: f64,
    /// Maximum position size percentage of portfolio
    #[serde(default = "default_max_position_size")]
    pub max_position_size_percent: f64,
    /// Daily loss limit percentage
    #[serde(default = "default_daily_loss_limit")]
    pub daily_loss_limit_percent: f64,
    /// Enable/disable auto-sell on profit target
    #[serde(default = "default_bool_true")]
    pub auto_sell_enabled: bool,
    /// Default profit target percentage for auto-sell
    #[serde(default = "default_profit_target")]
    pub default_profit_target_percent: f64,
    /// Enable/disable MEV protection
    #[serde(default = "default_bool_true")]
    pub mev_protection_enabled: bool,
    /// Flashbots RPC URL (if using Flashbots for MEV protection)
    pub flashbots_rpc_url: Option<String>,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            default_slippage_percent: default_slippage(),
            max_slippage_percent: default_max_slippage(),
            min_liquidity_usd: default_min_liquidity(),
            max_position_size_percent: default_max_position_size(),
            daily_loss_limit_percent: default_daily_loss_limit(),
            auto_sell_enabled: default_bool_true(),
            default_profit_target_percent: default_profit_target(),
            mev_protection_enabled: default_bool_true(),
            flashbots_rpc_url: None,
        }
    }
}

/// Market data configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataConfig {
    /// DexScreener API key (optional)
    pub dexscreener_api_key: Option<String>,
    /// CoinGecko API key (optional)
    pub coingecko_api_key: Option<String>,
    /// Update interval for price feeds in seconds
    #[serde(default = "default_price_update_interval")]
    pub price_update_interval_secs: u64,
    /// Cache TTL for market data in seconds
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
    /// Maximum concurrent API requests
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
    /// Enable/disable WebSocket streaming
    #[serde(default = "default_bool_true")]
    pub websocket_enabled: bool,
    /// Timeout for API requests in seconds
    #[serde(default = "default_api_timeout")]
    pub api_timeout_secs: u64,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self {
            dexscreener_api_key: None,
            coingecko_api_key: None,
            price_update_interval_secs: default_price_update_interval(),
            cache_ttl_secs: default_cache_ttl(),
            max_concurrent_requests: default_max_concurrent_requests(),
            websocket_enabled: default_bool_true(),
            api_timeout_secs: default_api_timeout(),
        }
    }
}

/// Security configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// JWT secret key
    pub jwt_secret: String,
    /// JWT token expiration in hours
    #[serde(default = "default_jwt_expiration")]
    pub jwt_expiration_hours: u64,
    /// Password hash iterations for Argon2
    #[serde(default = "default_password_iterations")]
    pub password_iterations: u32,
    /// Private key encryption key
    pub encryption_key: String,
    /// Enable/disable two-factor authentication
    #[serde(default = "default_bool_false")]
    pub two_factor_enabled: bool,
    /// Session timeout in hours
    #[serde(default = "default_session_timeout")]
    pub session_timeout_hours: u64,
    /// Maximum failed login attempts before lockout
    #[serde(default = "default_max_login_attempts")]
    pub max_login_attempts: u32,
    /// Lockout duration in minutes
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_minutes: u32,
}

/// Monitoring and logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Enable/disable JSON logging
    #[serde(default = "default_bool_false")]
    pub json_logging: bool,
    /// Prometheus metrics endpoint port
    #[serde(default = "default_metrics_port")]
    pub metrics_port: u16,
    /// Enable/disable tracing with Jaeger
    #[serde(default = "default_bool_false")]
    pub tracing_enabled: bool,
    /// Jaeger endpoint URL
    pub jaeger_endpoint: Option<String>,
    /// Sentry DSN (optional)
    pub sentry_dsn: Option<String>,
    /// Enable/disable health checks
    #[serde(default = "default_bool_true")]
    pub health_checks_enabled: bool,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            json_logging: default_bool_false(),
            metrics_port: default_metrics_port(),
            tracing_enabled: default_bool_false(),
            jaeger_endpoint: None,
            sentry_dsn: None,
            health_checks_enabled: default_bool_true(),
        }
    }
}

/// Main application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Application environment
    #[serde(default)]
    pub environment: Environment,
    /// Database configuration
    pub database: DatabaseConfig,
    /// Redis configuration
    pub redis: RedisConfig,
    /// Blockchain configurations
    pub blockchain: BlockchainConfig,
    /// Telegram bot configuration (optional)
    pub telegram: Option<TelegramConfig>,
    /// API server configuration
    pub api: ApiConfig,
    /// Trading engine configuration
    pub trading: TradingConfig,
    /// Market data configuration
    pub market_data: MarketDataConfig,
    /// Security configuration
    pub security: SecurityConfig,
    /// Monitoring and logging configuration
    pub monitoring: MonitoringConfig,
}

impl Config {
    /// Load configuration from various sources
    ///
    /// Sources are loaded in this order (later sources override earlier ones):
    /// 1. Default values
    /// 2. Configuration file (config.toml, config.yaml, or config.json)
    /// 3. Environment variables (with `VELOCIDAD_` prefix)
    /// 4. Profile-specific configuration
    pub fn load() -> Result<Self, figment::Error> {
        let profile = std::env::var("VELOCIDAD_ENV").unwrap_or_else(|_| "development".to_string());

        let config = Figment::new()
            // Default values
            .merge(Figment::from(Self::default()))
            // Configuration files (in order of precedence)
            .merge(Toml::file("config.toml"))
            .merge(Yaml::file("config.yaml"))
            .merge(Json::file("config.json"))
            // Profile-specific files
            .merge(Toml::file(format!("config.{}.toml", profile)))
            .merge(Yaml::file(format!("config.{}.yaml", profile)))
            .merge(Json::file(format!("config.{}.json", profile)))
            // Environment variables
            .merge(Env::prefixed("VELOCIDAD_").split("_"))
            // Select the active profile
            .select(Profile::from(profile.as_str()))
            // Extract the configuration
            .extract()?;

        Ok(config)
    }

    /// Get the socket address for the API server
    pub fn api_addr(&self) -> SocketAddr {
        SocketAddr::new(self.api.host, self.api.port)
    }

    /// Get the socket address for the metrics server
    pub fn metrics_addr(&self) -> SocketAddr {
        SocketAddr::new(self.api.host, self.monitoring.metrics_port)
    }

    /// Check if Telegram bot is enabled
    pub fn telegram_enabled(&self) -> bool {
        self.telegram.is_some()
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        // Validate database URL
        if self.database.url.is_empty() {
            errors.push("Database URL cannot be empty".to_string());
        }

        // Validate Redis URL
        if self.redis.url.is_empty() {
            errors.push("Redis URL cannot be empty".to_string());
        }

        // Validate Ethereum configuration if present
        if let Some(eth) = &self.blockchain.ethereum {
            if eth.rpc_url.is_empty() {
                errors.push("Ethereum RPC URL cannot be empty".to_string());
            }
            if eth.chain_id == 0 {
                errors.push("Ethereum chain ID must be non-zero".to_string());
            }
        }

        // Validate Solana configuration if present
        if let Some(sol) = &self.blockchain.solana {
            if sol.rpc_url.is_empty() {
                errors.push("Solana RPC URL cannot be empty".to_string());
            }
        }

        // Validate security configuration
        if self.security.jwt_secret.is_empty() {
            errors.push("JWT secret cannot be empty".to_string());
        }
        if self.security.encryption_key.is_empty() {
            errors.push("Encryption key cannot be empty".to_string());
        }

        // Validate trading configuration
        if self.trading.default_slippage_percent <= 0.0 {
            errors.push("Default slippage must be positive".to_string());
        }
        if self.trading.max_slippage_percent <= self.trading.default_slippage_percent {
            errors.push("Maximum slippage must be greater than default slippage".to_string());
        }
        if self.trading.min_liquidity_usd <= 0.0 {
            errors.push("Minimum liquidity must be positive".to_string());
        }
        if self.trading.max_position_size_percent <= 0.0
            || self.trading.max_position_size_percent > 100.0
        {
            errors.push("Maximum position size must be between 0 and 100 percent".to_string());
        }
        if self.trading.daily_loss_limit_percent < 0.0
            || self.trading.daily_loss_limit_percent > 100.0
        {
            errors.push("Daily loss limit must be between 0 and 100 percent".to_string());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            environment: Environment::default(),
            database: DatabaseConfig::default(),
            redis: RedisConfig::default(),
            blockchain: BlockchainConfig::default(),
            telegram: None,
            api: ApiConfig::default(),
            trading: TradingConfig::default(),
            market_data: MarketDataConfig::default(),
            security: SecurityConfig {
                jwt_secret: "change-me-in-production".to_string(),
                jwt_expiration_hours: default_jwt_expiration(),
                password_iterations: default_password_iterations(),
                encryption_key: "change-me-in-production".to_string(),
                two_factor_enabled: default_bool_false(),
                session_timeout_hours: default_session_timeout(),
                max_login_attempts: default_max_login_attempts(),
                lockout_duration_minutes: default_lockout_duration(),
            },
            monitoring: MonitoringConfig::default(),
        }
    }
}

// Default value functions

fn default_host() -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))
}

fn default_api_port() -> u16 {
    3000
}

fn default_metrics_port() -> u16 {
    9090
}

fn default_pool_max_size() -> u32 {
    20
}

fn default_pool_min_size() -> u32 {
    5
}

fn default_redis_pool_size() -> u32 {
    10
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_idle_timeout() -> u64 {
    600
}

fn default_max_lifetime() -> u64 {
    1800
}

fn default_read_timeout() -> u64 {
    10
}

fn default_write_timeout() -> u64 {
    10
}

fn default_gas_limit() -> u64 {
    300_000
}

fn default_priority_fee() -> u64 {
    2_000_000_000 // 2 Gwei
}

fn default_max_gas_price() -> u64 {
    500_000_000_000 // 500 Gwei
}

fn default_tx_timeout() -> u64 {
    120
}

fn default_confirmations() -> u32 {
    1
}

fn default_solana_commitment() -> String {
    "confirmed".to_string()
}

fn default_max_retries() -> u32 {
    3
}

fn default_max_message_length() -> usize {
    4096
}

fn default_request_timeout() -> u64 {
    30
}

fn default_body_limit() -> usize {
    10 * 1024 * 1024 // 10 MB
}

fn default_rpm_normal() -> u32 {
    60
}

fn default_rpm_premium() -> u32 {
    300
}

fn default_burst_size() -> u32 {
    10
}

fn default_rate_limit_prefix() -> String {
    "rate_limit".to_string()
}

fn default_slippage() -> f64 {
    0.5 // 0.5%
}

fn default_max_slippage() -> f64 {
    5.0 // 5%
}

fn default_min_liquidity() -> f64 {
    10_000.0 // $10,000
}

fn default_max_position_size() -> f64 {
    10.0 // 10% of portfolio
}

fn default_daily_loss_limit() -> f64 {
    5.0 // 5% daily loss limit
}

fn default_profit_target() -> f64 {
    20.0 // 20% profit target
}

fn default_price_update_interval() -> u64 {
    5 // 5 seconds
}

fn default_cache_ttl() -> u64 {
    60 // 60 seconds
}

fn default_max_concurrent_requests() -> usize {
    10
}

fn default_api_timeout() -> u64 {
    10
}

fn default_jwt_expiration() -> u64 {
    24 // 24 hours
}

fn default_password_iterations() -> u32 {
    3
}

fn default_session_timeout() -> u64 {
    168 // 1 week
}

fn default_max_login_attempts() -> u32 {
    5
}

fn default_lockout_duration() -> u32 {
    15 // 15 minutes
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_bool_true() -> bool {
    true
}

fn default_bool_false() -> bool {
    false
}
