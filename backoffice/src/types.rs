//! Shared type definitions for the Velocidad trading bot backend.
//!
//! This module contains common types used across multiple modules in the
//! application, including financial types, identifiers, trading types,
//! and API data structures.

use std::fmt;
use std::num::ParseFloatError;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Error type for type validation and conversion
#[derive(Error, Debug)]
pub enum TypeError {
    /// Invalid value for type
    #[error("Invalid value for {type_name}: {value}")]
    InvalidValue {
        /// Type name
        type_name: &'static str,
        /// Invalid value
        value: String,
        /// Optional reason
        reason: Option<String>,
    },

    /// Parse error
    #[error("Parse error: {0}")]
    Parse(String),

    /// Decimal conversion error
    #[error("Decimal conversion error: {0}")]
    Decimal(#[from] rust_decimal::Error),

    /// Float parse error
    #[error("Float parse error: {0}")]
    FloatParse(#[from] ParseFloatError),

    /// UUID parse error
    #[error("UUID parse error: {0}")]
    UuidParse(#[from] uuid::Error),
}

/// Result type for type operations
pub type TypeResult<T> = std::result::Result<T, TypeError>;

// ============================================================================
// Identifiers (Newtype pattern for type safety)
// ============================================================================

/// User ID wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(Uuid);

impl UserId {
    /// Create a new UserId from a UUID
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Generate a new random UserId
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID
    pub fn inner(&self) -> &Uuid {
        &self.0
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::random()
    }
}

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for UserId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

impl From<UserId> for Uuid {
    fn from(user_id: UserId) -> Self {
        user_id.0
    }
}

impl FromStr for UserId {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(Uuid::parse_str(s)?))
    }
}

/// Trade ID wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TradeId(Uuid);

impl TradeId {
    /// Create a new TradeId from a UUID
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Generate a new random TradeId
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID
    pub fn inner(&self) -> &Uuid {
        &self.0
    }
}

impl Default for TradeId {
    fn default() -> Self {
        Self::random()
    }
}

impl fmt::Display for TradeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for TradeId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

/// Strategy ID wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StrategyId(Uuid);

impl StrategyId {
    /// Create a new StrategyId from a UUID
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Generate a new random StrategyId
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID
    pub fn inner(&self) -> &Uuid {
        &self.0
    }
}

impl Default for StrategyId {
    fn default() -> Self {
        Self::random()
    }
}

impl fmt::Display for StrategyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for StrategyId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

/// Alert ID wrapper
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlertId(Uuid);

impl AlertId {
    /// Create a new AlertId from a UUID
    pub fn new(id: Uuid) -> Self {
        Self(id)
    }

    /// Generate a new random AlertId
    pub fn random() -> Self {
        Self(Uuid::new_v4())
    }

    /// Get the inner UUID
    pub fn inner(&self) -> &Uuid {
        &self.0
    }
}

impl Default for AlertId {
    fn default() -> Self {
        Self::random()
    }
}

impl fmt::Display for AlertId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for AlertId {
    fn from(uuid: Uuid) -> Self {
        Self::new(uuid)
    }
}

// ============================================================================
// Financial Types (Newtype pattern for monetary values)
// ============================================================================

/// Money amount in USD with fixed precision
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Money(Decimal);

impl Money {
    /// Create a new Money amount
    pub fn new(amount: Decimal) -> TypeResult<Self> {
        if amount.is_sign_negative() {
            return Err(TypeError::InvalidValue {
                type_name: "Money",
                value: amount.to_string(),
                reason: Some("Money amount cannot be negative".to_string()),
            });
        }

        // Ensure we have at most 2 decimal places for USD
        let rounded = amount.round_dp(2);
        Ok(Self(rounded))
    }

    /// Create from a floating point USD amount
    pub fn from_usd(usd: f64) -> TypeResult<Self> {
        let decimal = Decimal::from_f64(usd).ok_or_else(|| TypeError::InvalidValue {
            type_name: "Money",
            value: usd.to_string(),
            reason: Some("Failed to convert float to decimal".to_string()),
        })?;
        Self::new(decimal)
    }

    /// Get the amount as Decimal
    pub fn amount(&self) -> Decimal {
        self.0
    }

    /// Get the amount as USD float (for compatibility)
    pub fn as_usd(&self) -> f64 {
        self.0.try_into().unwrap_or(0.0)
    }

    /// Zero money amount
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }

    /// Check if amount is zero
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Add money amounts
    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(Self(self.0.checked_add(other.0)?))
    }

    /// Subtract money amounts
    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        let result = self.0.checked_sub(other.0)?;
        if result.is_sign_negative() {
            return None;
        }
        Some(Self(result))
    }

    /// Multiply by a factor
    pub fn checked_mul(&self, factor: Decimal) -> Option<Self> {
        let result = self.0.checked_mul(factor)?;
        if result.is_sign_negative() {
            return None;
        }
        Some(Self(result))
    }

    /// Divide by a divisor
    pub fn checked_div(&self, divisor: Decimal) -> Option<Self> {
        if divisor.is_zero() {
            return None;
        }
        let result = self.0.checked_div(divisor)?;
        if result.is_sign_negative() {
            return None;
        }
        Some(Self(result))
    }
}

impl Default for Money {
    fn default() -> Self {
        Self::zero()
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${:.2}", self.0)
    }
}

impl FromStr for Money {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let cleaned = s.trim_start_matches('$').trim();
        let decimal = Decimal::from_str(cleaned)?;
        Self::new(decimal)
    }
}

/// Price with precision for token prices
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Price(Decimal);

impl Price {
    /// Create a new Price
    pub fn new(price: Decimal) -> TypeResult<Self> {
        if price.is_sign_negative() {
            return Err(TypeError::InvalidValue {
                type_name: "Price",
                value: price.to_string(),
                reason: Some("Price cannot be negative".to_string()),
            });
        }

        // Ensure we have reasonable precision (8 decimal places for crypto)
        let rounded = price.round_dp(8);
        Ok(Self(rounded))
    }

    /// Create from a floating point price
    pub fn from_float(price: f64) -> TypeResult<Self> {
        let decimal = Decimal::from_f64(price).ok_or_else(|| TypeError::InvalidValue {
            type_name: "Price",
            value: price.to_string(),
            reason: Some("Failed to convert float to decimal".to_string()),
        })?;
        Self::new(decimal)
    }

    /// Get the price as Decimal
    pub fn value(&self) -> Decimal {
        self.0
    }

    /// Get the price as float (for compatibility)
    pub fn as_float(&self) -> f64 {
        self.0.try_into().unwrap_or(0.0)
    }

    /// Zero price
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }

    /// Check if price is zero
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Calculate percentage change
    pub fn percentage_change(&self, previous: &Self) -> Percentage {
        if previous.is_zero() {
            return Percentage::from_decimal(Decimal::ZERO);
        }

        let change = (self.0 - previous.0) / previous.0 * dec!(100);
        Percentage::from_decimal(change).unwrap_or(Percentage::zero())
    }
}

impl Default for Price {
    fn default() -> Self {
        Self::zero()
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.8}", self.0)
    }
}

impl FromStr for Price {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let decimal = Decimal::from_str(s)?;
        Self::new(decimal)
    }
}

/// Percentage value (0-100)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Percentage(Decimal);

impl Percentage {
    /// Create a new Percentage
    pub fn new(percent: Decimal) -> TypeResult<Self> {
        // Allow negative percentages for losses
        // Cap at reasonable range (-100% to 1000%)
        if percent < dec!(-100) || percent > dec!(1000) {
            return Err(TypeError::InvalidValue {
                type_name: "Percentage",
                value: percent.to_string(),
                reason: Some("Percentage must be between -100 and 1000".to_string()),
            });
        }

        // Round to 2 decimal places
        let rounded = percent.round_dp(2);
        Ok(Self(rounded))
    }

    /// Create from a floating point percentage
    pub fn from_float(percent: f64) -> TypeResult<Self> {
        let decimal = Decimal::from_f64(percent).ok_or_else(|| TypeError::InvalidValue {
            type_name: "Percentage",
            value: percent.to_string(),
            reason: Some("Failed to convert float to decimal".to_string()),
        })?;
        Self::new(decimal)
    }

    /// Create from a decimal value (0-1)
    pub fn from_decimal(decimal: Decimal) -> TypeResult<Self> {
        Self::new(decimal * dec!(100))
    }

    /// Get the percentage as Decimal
    pub fn value(&self) -> Decimal {
        self.0
    }

    /// Get as decimal (0-1)
    pub fn as_decimal(&self) -> Decimal {
        self.0 / dec!(100)
    }

    /// Get as float (for compatibility)
    pub fn as_float(&self) -> f64 {
        self.0.try_into().unwrap_or(0.0)
    }

    /// Zero percentage
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }

    /// 100% percentage
    pub fn hundred() -> Self {
        Self(dec!(100))
    }

    /// Check if percentage is zero
    pub fn is_zero(&self) -> bool {
        self.0.is_zero()
    }

    /// Apply percentage to a money amount
    pub fn apply_to(&self, money: &Money) -> Option<Money> {
        money.checked_mul(self.as_decimal())
    }

    /// Apply percentage to a price
    pub fn apply_to_price(&self, price: &Price) -> Option<Price> {
        let adjustment = price.value() * self.as_decimal();
        let new_price = price.value() + adjustment;
        Price::new(new_price).ok()
    }
}

impl Default for Percentage {
    fn default() -> Self {
        Self::zero()
    }
}

impl fmt::Display for Percentage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.2}%", self.0)
    }
}

impl FromStr for Percentage {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let cleaned = s.trim_end_matches('%').trim();
        let decimal = Decimal::from_str(cleaned)?;
        Self::new(decimal)
    }
}

// ============================================================================
// Trading Types
// ============================================================================

/// Trade side (buy/sell)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Buy order
    #[serde(rename = "buy")]
    Buy,
    /// Sell order
    #[serde(rename = "sell")]
    Sell,
}

impl Side {
    /// Check if this is a buy order
    pub fn is_buy(&self) -> bool {
        matches!(self, Side::Buy)
    }

    /// Check if this is a sell order
    pub fn is_sell(&self) -> bool {
        matches!(self, Side::Sell)
    }

    /// Get the opposite side
    pub fn opposite(&self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

impl Default for Side {
    fn default() -> Self {
        Side::Buy
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "buy"),
            Side::Sell => write!(f, "sell"),
        }
    }
}

impl FromStr for Side {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "buy" => Ok(Side::Buy),
            "sell" => Ok(Side::Sell),
            _ => Err(TypeError::InvalidValue {
                type_name: "Side",
                value: s.to_string(),
                reason: Some("Must be 'buy' or 'sell'".to_string()),
            }),
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    /// Market order (execute immediately at best available price)
    #[serde(rename = "market")]
    Market,
    /// Limit order (execute at specified price or better)
    #[serde(rename = "limit")]
    Limit,
    /// Stop loss order (becomes market order when price reaches stop price)
    #[serde(rename = "stop_loss")]
    StopLoss,
    /// Stop limit order (becomes limit order when price reaches stop price)
    #[serde(rename = "stop_limit")]
    StopLimit,
}

impl OrderType {
    /// Check if this is a market order
    pub fn is_market(&self) -> bool {
        matches!(self, OrderType::Market)
    }

    /// Check if this is a limit order
    pub fn is_limit(&self) -> bool {
        matches!(self, OrderType::Limit)
    }

    /// Check if this is a stop order
    pub fn is_stop(&self) -> bool {
        matches!(self, OrderType::StopLoss | OrderType::StopLimit)
    }
}

impl Default for OrderType {
    fn default() -> Self {
        OrderType::Market
    }
}

impl fmt::Display for OrderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderType::Market => write!(f, "market"),
            OrderType::Limit => write!(f, "limit"),
            OrderType::StopLoss => write!(f, "stop_loss"),
            OrderType::StopLimit => write!(f, "stop_limit"),
        }
    }
}

impl FromStr for OrderType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "market" => Ok(OrderType::Market),
            "limit" => Ok(OrderType::Limit),
            "stop_loss" | "stop-loss" | "stop" => Ok(OrderType::StopLoss),
            "stop_limit" | "stop-limit" => Ok(OrderType::StopLimit),
            _ => Err(TypeError::InvalidValue {
                type_name: "OrderType",
                value: s.to_string(),
                reason: Some("Must be 'market', 'limit', 'stop_loss', or 'stop_limit'".to_string()),
            }),
        }
    }
}

/// Time in force for orders
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeInForce {
    /// Good Till Cancelled (order stays active until cancelled)
    #[serde(rename = "gtc")]
    Gtc,
    /// Immediate or Cancel (fill immediately or cancel)
    #[serde(rename = "ioc")]
    Ioc,
    /// Fill or Kill (fill completely or cancel)
    #[serde(rename = "fok")]
    Fok,
    /// Good Till Date (order active until specified date)
    #[serde(rename = "gtd")]
    Gtd,
}

impl TimeInForce {
    /// Check if this is Good Till Cancelled
    pub fn is_gtc(&self) -> bool {
        matches!(self, TimeInForce::Gtc)
    }

    /// Check if this is Immediate or Cancel
    pub fn is_ioc(&self) -> bool {
        matches!(self, TimeInForce::Ioc)
    }

    /// Check if this is Fill or Kill
    pub fn is_fok(&self) -> bool {
        matches!(self, TimeInForce::Fok)
    }

    /// Check if this is Good Till Date
    pub fn is_gtd(&self) -> bool {
        matches!(self, TimeInForce::Gtd)
    }
}

impl Default for TimeInForce {
    fn default() -> Self {
        TimeInForce::Gtc
    }
}

impl fmt::Display for TimeInForce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TimeInForce::Gtc => write!(f, "gtc"),
            TimeInForce::Ioc => write!(f, "ioc"),
            TimeInForce::Fok => write!(f, "fok"),
            TimeInForce::Gtd => write!(f, "gtd"),
        }
    }
}

impl FromStr for TimeInForce {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "gtc" => Ok(TimeInForce::Gtc),
            "ioc" => Ok(TimeInForce::Ioc),
            "fok" => Ok(TimeInForce::Fok),
            "gtd" => Ok(TimeInForce::Gtd),
            _ => Err(TypeError::InvalidValue {
                type_name: "TimeInForce",
                value: s.to_string(),
                reason: Some("Must be 'gtc', 'ioc', 'fok', or 'gtd'".to_string()),
            }),
        }
    }
}

// ============================================================================
// API Data Structures
// ============================================================================

/// Pagination parameters for list endpoints
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PaginationParams {
    /// Page number (1-indexed)
    #[serde(default = "default_page")]
    pub page: u32,

    /// Page size
    #[serde(default = "default_page_size")]
    pub page_size: u32,

    /// Cursor for cursor-based pagination
    #[serde(default)]
    pub cursor: Option<String>,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    20
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            page: default_page(),
            page_size: default_page_size(),
            cursor: None,
        }
    }
}

impl PaginationParams {
    /// Calculate offset for SQL queries
    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.page_size
    }

    /// Calculate limit for SQL queries
    pub fn limit(&self) -> u32 {
        self.page_size
    }
}

/// Paginated response metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationMetadata {
    /// Current page number
    pub page: u32,

    /// Page size
    pub page_size: u32,

    /// Total number of items
    pub total: u64,

    /// Total number of pages
    pub total_pages: u32,

    /// Whether there's a next page
    pub has_next: bool,

    /// Whether there's a previous page
    pub has_previous: bool,

    /// Next cursor for cursor-based pagination
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl PaginationMetadata {
    /// Create pagination metadata from total count
    pub fn from_total(page: u32, page_size: u32, total: u64) -> Self {
        let total_pages = ((total as f64) / (page_size as f64)).ceil() as u32;

        Self {
            page,
            page_size,
            total,
            total_pages,
            has_next: page < total_pages,
            has_previous: page > 1,
            next_cursor: None,
        }
    }
}

/// Sorting parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortParams {
    /// Field to sort by
    pub field: String,

    /// Sort direction
    #[serde(default)]
    pub direction: SortDirection,
}

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    /// Ascending order
    #[serde(rename = "asc")]
    Asc,

    /// Descending order
    #[serde(rename = "desc")]
    Desc,
}

impl Default for SortDirection {
    fn default() -> Self {
        SortDirection::Desc
    }
}

impl fmt::Display for SortDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SortDirection::Asc => write!(f, "asc"),
            SortDirection::Desc => write!(f, "desc"),
        }
    }
}

impl FromStr for SortDirection {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "asc" | "ascending" => Ok(SortDirection::Asc),
            "desc" | "descending" => Ok(SortDirection::Desc),
            _ => Err(TypeError::InvalidValue {
                type_name: "SortDirection",
                value: s.to_string(),
                reason: Some("Must be 'asc' or 'desc'".to_string()),
            }),
        }
    }
}

// ============================================================================
// Time Range
// ============================================================================

/// Time range for queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    /// Start timestamp (inclusive)
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,

    /// End timestamp (exclusive)
    #[serde(default)]
    pub end: Option<DateTime<Utc>>,
}

impl TimeRange {
    /// Create a time range from start to end
    pub fn new(start: Option<DateTime<Utc>>, end: Option<DateTime<Utc>>) -> Self {
        Self { start, end }
    }

    /// Create a time range for the last N days
    pub fn last_days(days: i64) -> Self {
        let end = Utc::now();
        let start = end - chrono::Duration::days(days);
        Self {
            start: Some(start),
            end: Some(end),
        }
    }

    /// Create a time range for the last N hours
    pub fn last_hours(hours: i64) -> Self {
        let end = Utc::now();
        let start = end - chrono::Duration::hours(hours);
        Self {
            start: Some(start),
            end: Some(end),
        }
    }

    /// Check if time range is valid (start < end if both present)
    pub fn is_valid(&self) -> bool {
        match (self.start, self.end) {
            (Some(start), Some(end)) => start < end,
            _ => true,
        }
    }
}

// ============================================================================
// API Response Wrappers
// ============================================================================

/// Standard API response wrapper
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// Response data
    pub data: T,

    /// Optional metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ApiMetadata>,
}

impl<T> ApiResponse<T> {
    /// Create a new API response
    pub fn new(data: T) -> Self {
        Self { data, meta: None }
    }

    /// Create an API response with metadata
    pub fn with_meta(data: T, meta: ApiMetadata) -> Self {
        Self {
            data,
            meta: Some(meta),
        }
    }
}

/// API response metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiMetadata {
    /// Request ID for tracing
    pub request_id: String,

    /// Server timestamp
    pub timestamp: DateTime<Utc>,

    /// Optional pagination metadata
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationMetadata>,
}

impl ApiMetadata {
    /// Create new API metadata
    pub fn new(request_id: String) -> Self {
        Self {
            request_id,
            timestamp: Utc::now(),
            pagination: None,
        }
    }

    /// Create API metadata with pagination
    pub fn with_pagination(request_id: String, pagination: PaginationMetadata) -> Self {
        Self {
            request_id,
            timestamp: Utc::now(),
            pagination: Some(pagination),
        }
    }
}

/// Standard API error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    /// Error code
    pub code: String,

    /// Error message
    pub message: String,

    /// Optional error details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,

    /// Request ID for tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl ApiError {
    /// Create a new API error
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            request_id: None,
        }
    }

    /// Create an API error with details
    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(details),
            request_id: None,
        }
    }

    /// Add request ID to error
    pub fn with_request_id(self, request_id: impl Into<String>) -> Self {
        Self {
            request_id: Some(request_id.into()),
            ..self
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_money_creation() {
        // Valid money amounts
        assert!(Money::from_usd(100.50).is_ok());
        assert!(Money::from_usd(0.0).is_ok());

        // Invalid money amounts
        assert!(Money::from_usd(-100.0).is_err());
    }

    #[test]
    fn test_money_operations() {
        let m1 = Money::from_usd(100.0).unwrap();
        let m2 = Money::from_usd(50.0).unwrap();

        // Addition
        let sum = m1.checked_add(&m2).unwrap();
        assert_eq!(sum.as_usd(), 150.0);

        // Subtraction
        let diff = m1.checked_sub(&m2).unwrap();
        assert_eq!(diff.as_usd(), 50.0);

        // Invalid subtraction
        assert!(m2.checked_sub(&m1).is_none());
    }

    #[test]
    fn test_percentage_creation() {
        // Valid percentages
        assert!(Percentage::from_float(50.0).is_ok());
        assert!(Percentage::from_float(-10.0).is_ok());
        assert!(Percentage::from_float(0.0).is_ok());

        // Invalid percentages
        assert!(Percentage::from_float(-101.0).is_err());
        assert!(Percentage::from_float(1001.0).is_err());
    }

    #[test]
    fn test_percentage_operations() {
        let money = Money::from_usd(100.0).unwrap();
        let percent = Percentage::from_float(10.0).unwrap();

        // Apply percentage to money
        let result = percent.apply_to(&money).unwrap();
        assert_eq!(result.as_usd(), 10.0);

        // Percentage as decimal
        assert_eq!(percent.as_decimal(), dec!(0.1));
    }

    #[test]
    fn test_price_percentage_change() {
        let price1 = Price::from_float(100.0).unwrap();
        let price2 = Price::from_float(120.0).unwrap();

        let change = price2.percentage_change(&price1);
        assert_eq!(change.as_float(), 20.0);
    }

    #[test]
    fn test_side_parsing() {
        assert_eq!(Side::from_str("buy").unwrap(), Side::Buy);
        assert_eq!(Side::from_str("BUY").unwrap(), Side::Buy);
        assert_eq!(Side::from_str("sell").unwrap(), Side::Sell);
        assert!(Side::from_str("invalid").is_err());
    }

    #[test]
    fn test_pagination_params() {
        let params = PaginationParams {
            page: 2,
            page_size: 50,
            cursor: None,
        };

        assert_eq!(params.offset(), 50);
        assert_eq!(params.limit(), 50);
    }

    #[test]
    fn test_pagination_metadata() {
        let meta = PaginationMetadata::from_total(2, 20, 105);

        assert_eq!(meta.page, 2);
        assert_eq!(meta.page_size, 20);
        assert_eq!(meta.total, 105);
        assert_eq!(meta.total_pages, 6); // 105/20 = 5.25 -> ceil to 6
        assert!(meta.has_next);
        assert!(meta.has_previous);
    }

    #[test]
    fn test_time_range_validity() {
        let now = Utc::now();
        let past = now - chrono::Duration::hours(1);

        // Valid range
        let valid = TimeRange::new(Some(past), Some(now));
        assert!(valid.is_valid());

        // Invalid range
        let invalid = TimeRange::new(Some(now), Some(past));
        assert!(!invalid.is_valid());

        // Open ranges are always valid
        let open_start = TimeRange::new(None, Some(now));
        assert!(open_start.is_valid());

        let open_end = TimeRange::new(Some(past), None);
        assert!(open_end.is_valid());

        let open_both = TimeRange::new(None, None);
        assert!(open_both.is_valid());
    }
}
