//! Error types for the Velocidad trading bot backend.
//!
//! This module defines a unified error type that covers all error scenarios
//! across the application, including API errors, database errors, blockchain
//! errors, trading errors, and Telegram errors.

use std::num::ParseFloatError;
use std::str::ParseBoolError;
use std::string::FromUtf8Error;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Main error type for the Velocidad backend.
#[derive(Error, Debug)]
pub enum Error {
    /// Configuration errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Database errors
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Database migration errors
    #[error("Database migration error: {0}")]
    Migration(String),

    /// Database connection pool errors
    #[error("Database connection pool error: {0}")]
    Pool(#[from] bb8::RunError<sqlx::Error>),

    /// Redis errors
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),

    /// Redis pool errors
    #[error("Redis pool error: {0}")]
    RedisPool(#[from] bb8::RunError<redis::RedisError>),

    /// Message queue errors (RabbitMQ)
    #[error("Message queue error: {0}")]
    RabbitMq(#[from] lapin::Error),

    /// Message queue errors (Kafka)
    #[error("Kafka error: {0}")]
    Kafka(#[from] rdkafka::error::KafkaError),

    /// Ethereum blockchain errors
    #[error("Ethereum error: {0}")]
    Ethereum(String),

    /// Solana blockchain errors
    #[error("Solana error: {0}")]
    Solana(String),

    /// Transaction execution errors
    #[error("Transaction error: {0}")]
    Transaction(String),

    /// Smart contract interaction errors
    #[error("Contract error: {0}")]
    Contract(String),

    /// Trading errors
    #[error("Trading error: {0}")]
    Trading(String),

    /// Risk management errors
    #[error("Risk limit exceeded: {0}")]
    RiskLimit(String),

    /// Insufficient funds
    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),

    /// Slippage tolerance exceeded
    #[error("Slippage tolerance exceeded: {0}")]
    SlippageExceeded(String),

    /// Market data errors
    #[error("Market data error: {0}")]
    MarketData(String),

    /// Price feed errors
    #[error("Price feed error: {0}")]
    PriceFeed(String),

    /// Telegram bot errors
    #[error("Telegram error: {0}")]
    Telegram(String),

    /// WebSocket errors
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    /// HTTP client errors
    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    /// API validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Authentication errors
    #[error("Authentication error: {0}")]
    Auth(String),

    /// Authorization errors
    #[error("Authorization error: {0}")]
    Authorization(String),

    /// Resource not found errors
    #[error("Resource not found: {0}")]
    NotFound(String),

    /// Rate limiting errors
    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    /// JSON serialization/deserialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// UTF-8 conversion errors
    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] FromUtf8Error),

    /// Parse errors
    #[error("Parse error: {0}")]
    Parse(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic errors wrapped from anyhow
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),

    /// Internal server errors (for unexpected conditions)
    #[error("Internal server error: {0}")]
    Internal(String),
}

/// Convenience type alias for Result<T, Error>
pub type Result<T> = std::result::Result<T, Error>;

/// Error response for API endpoints
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Error code (machine-readable)
    pub code: String,

    /// Human-readable error message
    pub message: String,

    /// Optional additional error details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl Error {
    /// Convert error to an HTTP status code
    pub fn status_code(&self) -> StatusCode {
        match self {
            Error::Validation(_) => StatusCode::BAD_REQUEST,
            Error::Auth(_) => StatusCode::UNAUTHORIZED,
            Error::Authorization(_) => StatusCode::FORBIDDEN,
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::RateLimit(_) => StatusCode::TOO_MANY_REQUESTS,
            Error::InsufficientFunds(_) => StatusCode::BAD_REQUEST,
            Error::SlippageExceeded(_) => StatusCode::BAD_REQUEST,
            Error::RiskLimit(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Get machine-readable error code
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "CONFIG_ERROR",
            Error::Database(_) => "DATABASE_ERROR",
            Error::Migration(_) => "MIGRATION_ERROR",
            Error::Pool(_) => "POOL_ERROR",
            Error::Redis(_) => "REDIS_ERROR",
            Error::RedisPool(_) => "REDIS_POOL_ERROR",
            Error::RabbitMq(_) => "RABBITMQ_ERROR",
            Error::Kafka(_) => "KAFKA_ERROR",
            Error::Ethereum(_) => "ETHEREUM_ERROR",
            Error::Solana(_) => "SOLANA_ERROR",
            Error::Transaction(_) => "TRANSACTION_ERROR",
            Error::Contract(_) => "CONTRACT_ERROR",
            Error::Trading(_) => "TRADING_ERROR",
            Error::RiskLimit(_) => "RISK_LIMIT_EXCEEDED",
            Error::InsufficientFunds(_) => "INSUFFICIENT_FUNDS",
            Error::SlippageExceeded(_) => "SLIPPAGE_EXCEEDED",
            Error::MarketData(_) => "MARKET_DATA_ERROR",
            Error::PriceFeed(_) => "PRICE_FEED_ERROR",
            Error::Telegram(_) => "TELEGRAM_ERROR",
            Error::WebSocket(_) => "WEBSOCKET_ERROR",
            Error::HttpClient(_) => "HTTP_CLIENT_ERROR",
            Error::Validation(_) => "VALIDATION_ERROR",
            Error::Auth(_) => "AUTHENTICATION_ERROR",
            Error::Authorization(_) => "AUTHORIZATION_ERROR",
            Error::NotFound(_) => "NOT_FOUND",
            Error::RateLimit(_) => "RATE_LIMIT_EXCEEDED",
            Error::Json(_) => "JSON_ERROR",
            Error::Utf8(_) => "UTF8_ERROR",
            Error::Parse(_) => "PARSE_ERROR",
            Error::Io(_) => "IO_ERROR",
            Error::Anyhow(_) => "INTERNAL_ERROR",
            Error::Internal(_) => "INTERNAL_ERROR",
        }
    }

    /// Create a validation error
    pub fn validation(msg: impl Into<String>) -> Self {
        Error::Validation(msg.into())
    }

    /// Create an authentication error
    pub fn auth(msg: impl Into<String>) -> Self {
        Error::Auth(msg.into())
    }

    /// Create a trading error
    pub fn trading(msg: impl Into<String>) -> Self {
        Error::Trading(msg.into())
    }

    /// Create a risk limit error
    pub fn risk_limit(msg: impl Into<String>) -> Self {
        Error::RiskLimit(msg.into())
    }

    /// Create a not found error
    pub fn not_found(msg: impl Into<String>) -> Self {
        Error::NotFound(msg.into())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let response = ErrorResponse {
            code: self.code().to_string(),
            message: self.to_string(),
            details: None,
        };

        (status, axum::Json(response)).into_response()
    }
}

// Implement From for external error types

impl From<ParseFloatError> for Error {
    fn from(err: ParseFloatError) -> Self {
        Error::Parse(format!("Failed to parse float: {}", err))
    }
}

impl From<ParseBoolError> for Error {
    fn from(err: ParseBoolError) -> Self {
        Error::Parse(format!("Failed to parse boolean: {}", err))
    }
}

impl From<hex::FromHexError> for Error {
    fn from(err: hex::FromHexError) -> Self {
        Error::Parse(format!("Failed to parse hex: {}", err))
    }
}

impl From<url::ParseError> for Error {
    fn from(err: url::ParseError) -> Self {
        Error::Parse(format!("Failed to parse URL: {}", err))
    }
}

impl From<jsonwebtoken::errors::Error> for Error {
    fn from(err: jsonwebtoken::errors::Error) -> Self {
        Error::Auth(format!("JWT error: {}", err))
    }
}

impl From<argon2::password_hash::Error> for Error {
    fn from(err: argon2::password_hash::Error) -> Self {
        Error::Auth(format!("Password hash error: {}", err))
    }
}

// Note: We don't implement From for ethers::errors::* because they might not be compiled
// if the "ethereum" feature is disabled. Instead, we'll have conversion methods in the
// blockchain module.

// Similarly for teloxide errors - we'll handle those in the telegram module.

/// Extension trait for adding context to Results
pub trait Context<T, E> {
    /// Add context to an error
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static;

    /// Add context created by a closure (lazy evaluation)
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C;
}

impl<T, E> Context<T, E> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
    {
        self.map_err(|e| anyhow::anyhow!(e).context(context).into())
    }

    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|e| anyhow::anyhow!(e).context(f()).into())
    }
}
