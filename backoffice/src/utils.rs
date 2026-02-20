//! Utility functions for the Velocidad trading bot backend.
//!
//! This module provides common utility functions used across the application,
//! including cryptographic helpers, validation functions, serialization helpers,
//! time utilities, and mathematical functions for trading operations.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use hex::{FromHex, ToHex};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::prelude::*;

/// Utility errors
#[derive(Error, Debug)]
pub enum UtilError {
    /// Invalid input data
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Encoding/decoding error
    #[error("Encoding error: {0}")]
    Encoding(String),

    /// Decoding error
    #[error("Decoding error: {0}")]
    Decoding(String),

    /// Parse error
    #[error("Parse error: {0}")]
    Parse(String),

    /// Cryptographic error
    #[error("Cryptographic error: {0}")]
    Crypto(String),

    /// Time error
    #[error("Time error: {0}")]
    Time(String),

    /// Validation error
    #[error("Validation error: {0}")]
    Validation(String),
}

/// Result type for utility operations
pub type UtilResult<T> = std::result::Result<T, UtilError>;

// ============================================================================
// Cryptographic Utilities
// ============================================================================

/// Compute SHA256 hash of data
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Compute SHA256 hash and return as hex string
pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).encode_hex()
}

/// Compute double SHA256 (SHA256 of SHA256)
pub fn double_sha256(data: &[u8]) -> [u8; 32] {
    let first = sha256(data);
    sha256(&first)
}

/// Compute Keccak256 hash (Ethereum)
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    use tiny_keccak::{Hasher, Keccak};
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut output = [0u8; 32];
    hasher.finalize(&mut output);
    output
}

/// Compute Keccak256 hash and return as hex string
pub fn keccak256_hex(data: &[u8]) -> String {
    keccak256(data).encode_hex()
}

/// Generate a cryptographically secure random bytes
pub fn generate_random_bytes(size: usize) -> Vec<u8> {
    let mut rng = ChaCha20Rng::from_entropy();
    let mut bytes = vec![0u8; size];
    rng.fill_bytes(&mut bytes);
    bytes
}

/// Generate a random UUID
pub fn generate_uuid() -> Uuid {
    Uuid::new_v4()
}

/// Generate a random string of specified length
pub fn generate_random_string(length: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = ChaCha20Rng::from_entropy();
    (0..length)
        .map(|_| {
            let idx = rng.next_u32() as usize % CHARSET.len();
            CHARSET[idx] as char
        })
        .collect()
}

/// Encrypt data using AES-GCM
pub fn aes_gcm_encrypt(key: &[u8], data: &[u8]) -> UtilResult<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit, OsRng},
        Aes256Gcm, Nonce,
    };

    if key.len() != 32 {
        return Err(UtilError::Crypto("AES-256-GCM requires 32-byte key".to_string()));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| UtilError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, data)
        .map_err(|e| UtilError::Crypto(format!("Encryption failed: {}", e)))?;

    let mut result = Vec::with_capacity(nonce.len() + ciphertext.len());
    result.extend_from_slice(nonce.as_slice());
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Decrypt data using AES-GCM
pub fn aes_gcm_decrypt(key: &[u8], encrypted_data: &[u8]) -> UtilResult<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };

    if key.len() != 32 {
        return Err(UtilError::Crypto("AES-256-GCM requires 32-byte key".to_string()));
    }

    if encrypted_data.len() < 12 {
        return Err(UtilError::Crypto("Encrypted data too short".to_string()));
    }

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| UtilError::Crypto(format!("Failed to create cipher: {}", e)))?;

    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| UtilError::Crypto(format!("Decryption failed: {}", e)))
}

// ============================================================================
// Validation Utilities
// ============================================================================

/// Check if a string is a valid Ethereum address
pub fn is_valid_ethereum_address(address: &str) -> bool {
    // Remove 0x prefix if present
    let address = address.trim_start_matches("0x");

    // Check length (40 hex characters = 20 bytes)
    if address.len() != 40 {
        return false;
    }

    // Check hex characters
    address.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validate Ethereum address checksum (EIP-55)
pub fn validate_ethereum_checksum(address: &str) -> bool {
    // Address must start with 0x
    if !address.starts_with("0x") {
        return false;
    }

    let address = &address[2..];
    if address.len() != 40 {
        return false;
    }

    // Convert to lowercase for hashing
    let address_lower = address.to_lowercase();

    // Compute keccak256 hash of lowercase address
    let hash = keccak256(address_lower.as_bytes());
    let hash_hex = hash.encode_hex::<String>();

    // Check each character
    for (i, (addr_char, hash_char)) in address.chars().zip(hash_hex.chars()).enumerate() {
        let addr_char = addr_char as char;
        let hash_char = hash_char as char;

        // If hash character is 8-f (hex), then address character should be uppercase
        if hash_char >= '8' && hash_char <= 'f' {
            if !addr_char.is_ascii_uppercase() {
                return false;
            }
        } else {
            // Otherwise, address character should be lowercase
            if !addr_char.is_ascii_lowercase() {
                return false;
            }
        }
    }

    true
}

/// Check if a string is a valid hexadecimal string
pub fn is_valid_hex_string(s: &str, require_prefix: bool) -> bool {
    let s = if require_prefix && s.starts_with("0x") {
        &s[2..]
    } else if require_prefix {
        return false;
    } else if s.starts_with("0x") {
        &s[2..]
    } else {
        s
    };

    // Check even length (hex bytes are pairs)
    if s.len() % 2 != 0 {
        return false;
    }

    // Check all characters are hex digits
    s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Validate amount is positive and within reasonable bounds
pub fn validate_amount(amount: f64) -> UtilResult<()> {
    if amount <= 0.0 {
        return Err(UtilError::Validation("Amount must be positive".to_string()));
    }

    if amount > 1_000_000_000.0 {
        return Err(UtilError::Validation("Amount too large".to_string()));
    }

    if amount.is_nan() || amount.is_infinite() {
        return Err(UtilError::Validation("Amount must be a finite number".to_string()));
    }

    Ok(())
}

/// Validate percentage is within 0-100 range
pub fn validate_percentage(percent: f64) -> UtilResult<()> {
    if percent < 0.0 || percent > 100.0 {
        return Err(UtilError::Validation(
            "Percentage must be between 0 and 100".to_string(),
        ));
    }

    if percent.is_nan() || percent.is_infinite() {
        return Err(UtilError::Validation(
            "Percentage must be a finite number".to_string(),
        ));
    }

    Ok(())
}

/// Validate slippage percentage is reasonable
pub fn validate_slippage(slippage: f64) -> UtilResult<()> {
    if slippage < 0.0 || slippage > 50.0 {
        return Err(UtilError::Validation(
            "Slippage must be between 0 and 50 percent".to_string(),
        ));
    }

    if slippage.is_nan() || slippage.is_infinite() {
        return Err(UtilError::Validation(
            "Slippage must be a finite number".to_string(),
        ));
    }

    Ok(())
}

// ============================================================================
// Encoding/Decoding Utilities
// ============================================================================

/// Encode bytes to hex string with 0x prefix
pub fn encode_hex_with_prefix(bytes: &[u8]) -> String {
    format!("0x{}", bytes.encode_hex::<String>())
}

/// Decode hex string (with or without 0x prefix) to bytes
pub fn decode_hex(hex_str: &str) -> UtilResult<Vec<u8>> {
    let hex_str = hex_str.trim_start_matches("0x");
    Vec::from_hex(hex_str).map_err(|e| UtilError::Decoding(format!("Invalid hex: {}", e)))
}

/// Encode bytes to Base64
pub fn encode_base64(bytes: &[u8]) -> String {
    BASE64_STANDARD.encode(bytes)
}

/// Decode Base64 string to bytes
pub fn decode_base64(base64_str: &str) -> UtilResult<Vec<u8>> {
    BASE64_STANDARD
        .decode(base64_str)
        .map_err(|e| UtilError::Decoding(format!("Invalid Base64: {}", e)))
}

/// Serialize to JSON with pretty printing
pub fn to_json_pretty<T: Serialize>(value: &T) -> UtilResult<String> {
    serde_json::to_string_pretty(value).map_err(|e| UtilError::Encoding(format!("JSON serialization failed: {}", e)))
}

/// Serialize to JSON
pub fn to_json<T: Serialize>(value: &T) -> UtilResult<String> {
    serde_json::to_string(value).map_err(|e| UtilError::Encoding(format!("JSON serialization failed: {}", e)))
}

/// Deserialize from JSON
pub fn from_json<'a, T: Deserialize<'a>>(json_str: &'a str) -> UtilResult<T> {
    serde_json::from_str(json_str).map_err(|e| UtilError::Decoding(format!("JSON deserialization failed: {}", e)))
}

// ============================================================================
// Time Utilities
// ============================================================================

/// Get current timestamp in seconds since Unix epoch
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Get current timestamp in milliseconds since Unix epoch
pub fn current_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Convert timestamp (seconds) to DateTime<Utc>
pub fn timestamp_to_datetime(timestamp: i64) -> UtilResult<DateTime<Utc>> {
    NaiveDateTime::from_timestamp_opt(timestamp, 0)
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .ok_or_else(|| UtilError::Time(format!("Invalid timestamp: {}", timestamp)))
}

/// Convert DateTime<Utc> to timestamp (seconds)
pub fn datetime_to_timestamp(datetime: &DateTime<Utc>) -> i64 {
    datetime.timestamp()
}

/// Format duration as human-readable string
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else if seconds < 86400 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        format!("{}h {}m", hours, minutes)
    } else {
        let days = seconds / 86400;
        let hours = (seconds % 86400) / 3600;
        format!("{}d {}h", days, hours)
    }
}

/// Calculate time elapsed since given timestamp
pub fn time_elapsed_since(timestamp: u64) -> Duration {
    let now = current_timestamp();
    Duration::from_secs(now.saturating_sub(timestamp))
}

/// Check if a timestamp is expired (older than max_age seconds)
pub fn is_timestamp_expired(timestamp: u64, max_age_seconds: u64) -> bool {
    time_elapsed_since(timestamp).as_secs() > max_age_seconds
}

// ============================================================================
// Mathematical Utilities
// ============================================================================

/// Calculate percentage of a value
pub fn calculate_percentage(value: f64, percentage: f64) -> f64 {
    value * (percentage / 100.0)
}

/// Calculate percentage change
pub fn calculate_percentage_change(old_value: f64, new_value: f64) -> f64 {
    if old_value == 0.0 {
        return 0.0;
    }
    ((new_value - old_value) / old_value) * 100.0
}

/// Calculate profit/loss percentage
pub fn calculate_profit_loss_percentage(entry_price: f64, exit_price: f64) -> f64 {
    calculate_percentage_change(entry_price, exit_price)
}

/// Calculate profit/loss amount
pub fn calculate_profit_loss_amount(entry_price: f64, exit_price: f64, quantity: f64) -> f64 {
    (exit_price - entry_price) * quantity
}

/// Calculate simple moving average
pub fn calculate_sma(prices: &[f64], period: usize) -> Option<f64> {
    if prices.len() < period {
        return None;
    }

    let sum: f64 = prices.iter().rev().take(period).sum();
    Some(sum / period as f64)
}

/// Calculate exponential moving average
pub fn calculate_ema(prices: &[f64], period: usize) -> Option<f64> {
    if prices.len() < period {
        return None;
    }

    let multiplier = 2.0 / (period as f64 + 1.0);
    let mut ema = prices[0];

    for &price in prices.iter().skip(1) {
        ema = (price - ema) * multiplier + ema;
    }

    Some(ema)
}

/// Calculate standard deviation
pub fn calculate_std_dev(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }

    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|&x| (x - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;

    Some(variance.sqrt())
}

/// Calculate volatility (annualized standard deviation)
pub fn calculate_volatility(returns: &[f64], periods_per_year: f64) -> Option<f64> {
    let std_dev = calculate_std_dev(returns)?;
    Some(std_dev * periods_per_year.sqrt())
}

/// Round to specified number of decimal places
pub fn round_decimal(value: f64, decimals: u32) -> f64 {
    let factor = 10_f64.powi(decimals as i32);
    (value * factor).round() / factor
}

/// Truncate to specified number of decimal places
pub fn truncate_decimal(value: f64, decimals: u32) -> f64 {
    let factor = 10_f64.powi(decimals as i32);
    (value * factor).trunc() / factor
}

// ============================================================================
// Formatting Utilities
// ============================================================================

/// Format large number with suffix (K, M, B, T)
pub fn format_large_number(number: f64) -> String {
    let abs_number = number.abs();

    if abs_number >= 1_000_000_000_000.0 {
        format!("{:.2}T", number / 1_000_000_000_000.0)
    } else if abs_number >= 1_000_000_000.0 {
        format!("{:.2}B", number / 1_000_000_000.0)
    } else if abs_number >= 1_000_000.0 {
        format!("{:.2}M", number / 1_000_000.0)
    } else if abs_number >= 1_000.0 {
        format!("{:.2}K", number / 1_000.0)
    } else {
        format!("{:.2}", number)
    }
}

/// Format currency amount
pub fn format_currency(amount: f64) -> String {
    if amount >= 1_000_000.0 {
        format!("${:.2}M", amount / 1_000_000.0)
    } else if amount >= 1_000.0 {
        format!("${:.2}K", amount / 1_000.0)
    } else {
        format!("${:.2}", amount)
    }
}

/// Format percentage with sign
pub fn format_percentage(percent: f64) -> String {
    let sign = if percent >= 0.0 { "+" } else { "" };
    format!("{}{:.2}%", sign, percent)
}

/// Format byte size (KB, MB, GB, TB)
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Truncate string with ellipsis
pub fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

// ============================================================================
// Miscellaneous Utilities
// ============================================================================

/// Retry an async operation with exponential backoff
pub async fn retry_with_backoff<F, Fut, T, E>(
    mut operation: F,
    max_retries: u32,
    initial_delay_ms: u64,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut retries = 0;
    let mut delay_ms = initial_delay_ms;

    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                if retries >= max_retries {
                    return Err(err);
                }

                debug!("Operation failed (attempt {}): {:?}", retries + 1, err);
                retries += 1;

                // Exponential backoff with jitter
                let jitter = rand::random::<u64>() % (delay_ms / 4);
                let sleep_time = delay_ms + jitter;

                tokio::time::sleep(tokio::time::Duration::from_millis(sleep_time)).await;
                delay_ms *= 2; // Double the delay for next retry
            }
        }
    }
}

/// Parse environment variable with default value
pub fn parse_env_var<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Create a trace ID for distributed tracing
pub fn create_trace_id() -> String {
    format!("trace_{}", generate_uuid())
}

/// Create a request ID for API requests
pub fn create_request_id() -> String {
    format!("req_{}", generate_random_string(16))
}

/// Mask sensitive data for logging
pub fn mask_sensitive(s: &str, visible_chars: usize) -> String {
    if s.len() <= visible_chars * 2 {
        "*".repeat(s.len())
    } else {
        let start = &s[..visible_chars];
        let end = &s[s.len() - visible_chars..];
        format!("{}...{}", start, end)
    }
}

/// Mask API key for logging
pub fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        "*".repeat(key.len())
    } else {
        format!("{}...{}", &key[..4], &key[key.len() - 4..])
    }
}

/// Mask wallet address for logging
pub fn mask_wallet_address(address: &str) -> String {
    if address.len() <= 12 {
        "*".repeat(address.len())
    } else if address.starts_with("0x") && address.len() > 10 {
        format!("0x{}...{}", &address[2..6], &address[address.len() - 4..])
    } else if address.len() > 10 {
        format!("{}...{}", &address[..6], &address[address.len() - 4..])
    } else {
        "*".repeat(address.len())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256() {
        let data = b"hello world";
        let hash = sha256(data);
        let hash_hex = sha256_hex(data);

        assert_eq!(hash.len(), 32);
        assert_eq!(hash_hex.len(), 64);
        assert_eq!(
            hash_hex,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_keccak256() {
        let data = b"hello world";
        let hash = keccak256(data);
        let hash_hex = keccak256_hex(data);

        assert_eq!(hash.len(), 32);
        assert_eq!(hash_hex.len(), 64);
    }

    #[test]
    fn test_validate_ethereum_address() {
        // Valid addresses
        assert!(is_valid_ethereum_address("0x742d35Cc6634C0532925a3b844Bc454e4438f44e"));
        assert!(is_valid_ethereum_address("742d35Cc6634C0532925a3b844Bc454e4438f44e")); // Without 0x

        // Invalid addresses
        assert!(!is_valid_ethereum_address("0x123")); // Too short
        assert!(!is_valid_ethereum_address("0x742d35Cc6634C0532925a3b844Bc454e4438f44g")); // Invalid hex char
    }

    #[test]
    fn test_validate_ethereum_checksum() {
        // Test with a known checksum address
        let address = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
        assert!(validate_ethereum_checksum(address));

        // All lowercase should fail checksum
        let lowercase = "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed";
        assert!(!validate_ethereum_checksum(lowercase));

        // All uppercase should fail checksum
        let uppercase = "0x5AAEB6053F3E94C9B9A09F33669435E7EF1BEAED";
        assert!(!validate_ethereum_checksum(uppercase));
    }

    #[test]
    fn test_hex_encoding_decoding() {
        let data = b"hello";
        let hex_with_prefix = encode_hex_with_prefix(data);
        assert!(hex_with_prefix.starts_with("0x"));

        let decoded = decode_hex(&hex_with_prefix).unwrap();
        assert_eq!(decoded, data);

        // Test without prefix
        let decoded2 = decode_hex(&hex_with_prefix[2..]).unwrap();
        assert_eq!(decoded2, data);
    }

    #[test]
    fn test_base64_encoding_decoding() {
        let data = b"hello world";
        let encoded = encode_base64(data);
        let decoded = decode_base64(&encoded).unwrap();

        assert_eq!(decoded, data);
        assert_eq!(encoded, "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn test_validate_amount() {
        assert!(validate_amount(100.0).is_ok());
        assert!(validate_amount(0.001).is_ok());

        assert!(validate_amount(0.0).is_err());
        assert!(validate_amount(-100.0).is_err());
        assert!(validate_amount(f64::INFINITY).is_err());
        assert!(validate_amount(f64::NAN).is_err());
    }

    #[test]
    fn test_validate_percentage() {
        assert!(validate_percentage(50.0).is_ok());
        assert!(validate_percentage(0.0).is_ok());
        assert!(validate_percentage(100.0).is_ok());

        assert!(validate_percentage(-1.0).is_err());
        assert!(validate_percentage(101.0).is_err());
        assert!(validate_percentage(f64::INFINITY).is_err());
        assert!(validate_percentage(f64::NAN).is_err());
    }

    #[test]
    fn test_validate_slippage() {
        assert!(validate_slippage(1.0).is_ok());
        assert!(validate_slippage(0.0).is_ok());
        assert!(validate_slippage(50.0).is_ok());

        assert!(validate_slippage(-1.0).is_err());
        assert!(validate_slippage(51.0).is_err());
        assert!(validate_slippage(f64::INFINITY).is_err());
        assert!(validate_slippage(f64::NAN).is_err());
    }

    #[test]
    fn test_calculate_percentage() {
        assert_eq!(calculate_percentage(100.0, 10.0), 10.0);
        assert_eq!(calculate_percentage(200.0, 25.0), 50.0);
        assert_eq!(calculate_percentage(0.0, 50.0), 0.0);
    }

    #[test]
    fn test_calculate_percentage_change() {
        assert_eq!(calculate_percentage_change(100.0, 120.0), 20.0);
        assert_eq!(calculate_percentage_change(100.0, 80.0), -20.0);
        assert_eq!(calculate_percentage_change(0.0, 100.0), 0.0);
    }

    #[test]
    fn test_format_large_number() {
        assert_eq!(format_large_number(1_500_000_000_000.0), "1.50T");
        assert_eq!(format_large_number(2_500_000_000.0), "2.50B");
        assert_eq!(format_large_number(3_500_000.0), "3.50M");
        assert_eq!(format_large_number(4_500.0), "4.50K");
        assert_eq!(format_large_number(500.0), "500.00");
    }

    #[test]
    fn test_format_currency() {
        assert_eq!(format_currency(2_500_000.0), "$2500.00K");
        assert_eq!(format_currency(3_500.0), "$3.50K");
        assert_eq!(format_currency(500.0), "$500.00");
        assert_eq!(format_currency(0.50), "$0.50");
    }

    #[test]
    fn test_format_percentage() {
        assert_eq!(format_percentage(10.5), "+10.50%");
        assert_eq!(format_percentage(-5.25), "-5.25%");
        assert_eq!(format_percentage(0.0), "+0.00%");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(1_500_000_000_000), "1.36 TB");
        assert_eq!(format_bytes(2_500_000_000), "2.33 GB");
        assert_eq!(format_bytes(3_500_000), "3.34 MB");
        assert_eq!(format_bytes(4_500), "4.39 KB");
        assert_eq!(format_bytes(500), "500 B");
    }

    #[test]
    fn test_truncate_with_ellipsis() {
        assert_eq!(truncate_with_ellipsis("hello world", 5), "he...");
        assert_eq!(truncate_with_ellipsis("hello world", 11), "hello world");
        assert_eq!(truncate_with_ellipsis("hello world", 3), "...");
        assert_eq!(truncate_with_ellipsis("hello world", 0), "...");
    }

    #[test]
    fn test_mask_sensitive() {
        assert_eq!(mask_sensitive("secret", 2), "se...et");
        assert_eq!(mask_sensitive("short", 3), "***");
        assert_eq!(mask_sensitive("a", 1), "*");
    }

    #[test]
    fn test_mask_api_key() {
        assert_eq!(mask_api_key("sk_live_1234567890abcdef"), "sk_l...cdef");
        assert_eq!(mask_api_key("short"), "*****");
    }

    #[test]
    fn test_mask_wallet_address() {
        assert_eq!(
            mask_wallet_address("0x742d35Cc6634C0532925a3b844Bc454e4438f44e"),
            "0x742d...f44e"
        );
        assert_eq!(mask_wallet_address("short"), "*****");
        assert_eq!(mask_wallet_address("1234567890abcdef"), "123456...cdef");
    }

    #[test]
    fn test_time_utilities() {
        let timestamp = current_timestamp();
        let datetime = timestamp_to_datetime(timestamp as i64).unwrap();
        let converted_timestamp = datetime_to_timestamp(&datetime);

        assert_eq!(timestamp as i64, converted_timestamp);

        let duration = Duration::from_secs(3665); // 1 hour, 1 minute, 5 seconds
        assert_eq!(format_duration(duration), "1h 1m");

        assert!(!is_timestamp_expired(timestamp, 3600));
    }
}
