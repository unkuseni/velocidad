//! Market data module for aggregating and processing market data.
//!
//! This module provides real-time market data aggregation from multiple sources,
//! including DEXs, price oracles, and blockchain data feeds.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use thiserror::Error;

use crate::blockchain::{Address, Chain};
use crate::error::{Error, Result};
use crate::prelude::*;

/// Supported DEXs for market data
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dex {
    /// Uniswap V2
    UniswapV2,
    /// Uniswap V3
    UniswapV3,
    /// Sushiswap
    Sushiswap,
    /// Pancakeswap
    Pancakeswap,
    /// Curve
    Curve,
    /// Balancer
    Balancer,
    /// 1inch
    OneInch,
    /// 0x
    ZeroX,
    /// Jupiter (Solana)
    Jupiter,
    /// Raydium (Solana)
    Raydium,
    /// Orca (Solana)
    Orca,
}

impl Dex {
    /// Get the human-readable name of the DEX
    pub fn name(&self) -> &'static str {
        match self {
            Dex::UniswapV2 => "Uniswap V2",
            Dex::UniswapV3 => "Uniswap V3",
            Dex::Sushiswap => "Sushiswap",
            Dex::Pancakeswap => "Pancakeswap",
            Dex::Curve => "Curve",
            Dex::Balancer => "Balancer",
            Dex::OneInch => "1inch",
            Dex::ZeroX => "0x",
            Dex::Jupiter => "Jupiter",
            Dex::Raydium => "Raydium",
            Dex::Orca => "Orca",
        }
    }

    /// Get the default chain for this DEX
    pub fn default_chain(&self) -> Chain {
        match self {
            Dex::UniswapV2 | Dex::UniswapV3 | Dex::Sushiswap | Dex::Curve | Dex::Balancer | Dex::OneInch | Dex::ZeroX => Chain::Ethereum,
            Dex::Pancakeswap => Chain::Bsc,
            Dex::Jupiter | Dex::Raydium | Dex::Orca => Chain::Solana,
        }
    }

    /// Check if this DEX supports the given chain
    pub fn supports_chain(&self, chain: Chain) -> bool {
        match self {
            Dex::UniswapV2 | Dex::UniswapV3 | Dex::Sushiswap | Dex::Curve | Dex::Balancer | Dex::OneInch | Dex::ZeroX => {
                matches!(chain, Chain::Ethereum | Chain::EthereumGoerli | Chain::EthereumSepolia | Chain::Arbitrum | Chain::Polygon | Chain::Optimism | Chain::Base | Chain::Avalanche)
            }
            Dex::Pancakeswap => chain == Chain::Bsc,
            Dex::Jupiter | Dex::Raydium | Dex::Orca => {
                matches!(chain, Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet)
            }
        }
    }
}

impl fmt::Display for Dex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Price data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceData {
    /// Token address
    pub token: Address,
    /// DEX where the price was observed
    pub dex: Dex,
    /// Chain where the DEX is located
    pub chain: Chain,
    /// Price in USD
    pub price: f64,
    /// Confidence interval (standard deviation) for the price
    pub confidence: Option<f64>,
    /// Liquidity in USD at this price point
    pub liquidity_usd: f64,
    /// 24-hour volume in USD
    pub volume_24h_usd: Option<f64>,
    /// Price update timestamp
    pub timestamp: DateTime<Utc>,
    /// Source of the price data
    pub source: PriceSource,
}

/// Price source
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// Direct from DEX pool
    DexPool,
    /// DEX aggregator (1inch, 0x, etc.)
    DexAggregator,
    /// Price oracle (Chainlink, Pyth, etc.)
    Oracle,
    /// Centralized exchange
    Cex,
    /// Community price feed
    Community,
}

/// Liquidity pool data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityPool {
    /// Pool address
    pub address: Address,
    /// DEX
    pub dex: Dex,
    /// Chain
    pub chain: Chain,
    /// Token 0 address
    pub token0: Address,
    /// Token 1 address
    pub token1: Address,
    /// Token 0 reserves
    pub reserve0: f64,
    /// Token 1 reserves
    pub reserve1: f64,
    /// Total liquidity in USD
    pub liquidity_usd: f64,
    /// 24-hour trading volume in USD
    pub volume_24h_usd: f64,
    /// Fee tier (percentage, e.g., 0.3 for 0.3%)
    pub fee_tier: f64,
    /// Last update timestamp
    pub last_updated: DateTime<Utc>,
}

/// Token metrics for comprehensive analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenMetrics {
    /// Token address
    pub token: Address,
    /// Chain
    pub chain: Chain,
    /// Current price in USD (weighted average across sources)
    pub price_usd: f64,
    /// Price change percentage (24h)
    pub price_change_24h: Option<f64>,
    /// Price change percentage (7d)
    pub price_change_7d: Option<f64>,
    /// Total liquidity across all DEXs in USD
    pub total_liquidity_usd: f64,
    /// 24-hour trading volume in USD
    pub volume_24h_usd: f64,
    /// Market cap in USD (if available)
    pub market_cap_usd: Option<f64>,
    /// Fully diluted valuation in USD (if available)
    pub fdv_usd: Option<f64>,
    /// Holder count (if available)
    pub holder_count: Option<u64>,
    /// Holder distribution (top 10 holders percentage)
    pub top_10_holders_percent: Option<f64>,
    /// Social sentiment score (-1.0 to 1.0)
    pub sentiment_score: Option<f64>,
    /// Honeypot risk score (0.0 to 1.0, higher = more risky)
    pub honeypot_risk: Option<f64>,
    /// Contract verification status
    pub contract_verified: bool,
    /// Liquidity locked percentage (0.0 to 1.0)
    pub liquidity_locked_percent: Option<f64>,
    /// Last update timestamp
    pub last_updated: DateTime<Utc>,
}

/// Price feed trait for different data sources
#[async_trait]
pub trait PriceFeed: Send + Sync {
    /// Get the name of the price feed
    fn name(&self) -> &str;

    /// Get the supported chains
    fn supported_chains(&self) -> Vec<Chain>;

    /// Get the supported DEXs
    fn supported_dexs(&self) -> Vec<Dex>;

    /// Get price for a token
    async fn get_price(&self, token: &Address, chain: Chain, dex: Option<Dex>) -> Result<PriceData>;

    /// Get prices for multiple tokens
    async fn get_prices(
        &self,
        tokens: &[Address],
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<Vec<PriceData>>;

    /// Subscribe to price updates
    async fn subscribe(
        &self,
        tokens: &[Address],
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<tokio::sync::mpsc::Receiver<PriceData>>;

    /// Get liquidity pools for a token
    async fn get_liquidity_pools(
        &self,
        token: &Address,
        chain: Chain,
    ) -> Result<Vec<LiquidityPool>>;
}

/// Market data aggregator error types
#[derive(Error, Debug)]
pub enum MarketError {
    /// No price data available
    #[error("No price data available for token {token} on {chain}")]
    NoPriceData {
        token: Address,
        chain: Chain,
        dex: Option<Dex>,
    },

    /// Invalid token address
    #[error("Invalid token address: {0}")]
    InvalidToken(String),

    /// Unsupported chain
    #[error("Unsupported chain {0} for DEX {1}")]
    UnsupportedChain(Chain, Dex),

    /// API rate limit exceeded
    #[error("API rate limit exceeded for {0}")]
    RateLimitExceeded(String),

    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Data parsing error
    #[error("Data parsing error: {0}")]
    ParseError(String),

    /// WebSocket error
    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),
}

/// Market data aggregator configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataConfig {
    /// Update interval in seconds
    pub update_interval_secs: u64,
    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
    /// Maximum concurrent requests
    pub max_concurrent_requests: usize,
    /// Enable WebSocket streaming
    pub websocket_enabled: bool,
    /// API timeout in seconds
    pub api_timeout_secs: u64,
    /// List of enabled price feeds
    pub enabled_feeds: Vec<String>,
    /// Minimum liquidity threshold in USD
    pub min_liquidity_usd: f64,
    /// Price confidence threshold (standard deviation)
    pub price_confidence_threshold: f64,
}

impl Default for MarketDataConfig {
    fn default() -> Self {
        Self {
            update_interval_secs: 30,
            cache_ttl_secs: 300,
            max_concurrent_requests: 10,
            websocket_enabled: true,
            api_timeout_secs: 30,
            enabled_feeds: vec!["dex_screener".to_string(), "coingecko".to_string()],
            min_liquidity_usd: 10000.0,
            price_confidence_threshold: 0.05, // 5%
        }
    }
}

/// Market data aggregator
pub struct MarketDataAggregator {
    /// Price feeds
    feeds: Vec<Arc<dyn PriceFeed>>,
    /// Configuration
    config: MarketDataConfig,
    /// Price cache
    price_cache: Arc<RwLock<HashMap<PriceCacheKey, CachedPriceData>>>,
    /// Token metrics cache
    metrics_cache: Arc<RwLock<HashMap<Address, TokenMetrics>>>,
    /// WebSocket connections
    #[allow(dead_code)]
    ws_connections: Arc<RwLock<HashMap<Chain, tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>>>>,
}

/// Price cache key
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PriceCacheKey {
    token: Address,
    chain: Chain,
    dex: Option<Dex>,
}

/// Cached price data
#[derive(Debug, Clone)]
struct CachedPriceData {
    price: PriceData,
    expires_at: DateTime<Utc>,
}

impl MarketDataAggregator {
    /// Create a new market data aggregator
    pub fn new(config: MarketDataConfig) -> Result<Self> {
        let mut feeds = Vec::new();

        // Initialize enabled feeds based on configuration
        for feed_name in &config.enabled_feeds {
            match feed_name.as_str() {
                "dex_screener" => {
                    #[cfg(feature = "dex_screener")]
                    {
                        let feed = DexScreenerFeed::new()?;
                        feeds.push(Arc::new(feed) as Arc<dyn PriceFeed>);
                    }
                    #[cfg(not(feature = "dex_screener"))]
                    {
                        tracing::warn!("DEX Screener feed not compiled in");
                    }
                }
                "coingecko" => {
                    #[cfg(feature = "coingecko")]
                    {
                        let feed = CoinGeckoFeed::new()?;
                        feeds.push(Arc::new(feed) as Arc<dyn PriceFeed>);
                    }
                    #[cfg(not(feature = "coingecko"))]
                    {
                        tracing::warn!("CoinGecko feed not compiled in");
                    }
                }
                "chainlink" => {
                    #[cfg(feature = "chainlink")]
                    {
                        let feed = ChainlinkFeed::new()?;
                        feeds.push(Arc::new(feed) as Arc<dyn PriceFeed>);
                    }
                    #[cfg(not(feature = "chainlink"))]
                    {
                        tracing::warn!("Chainlink feed not compiled in");
                    }
                }
                _ => {
                    tracing::warn!("Unknown price feed: {}", feed_name);
                }
            }
        }

        Ok(Self {
            feeds,
            config,
            price_cache: Arc::new(RwLock::new(HashMap::new())),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
            ws_connections: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get price for a token
    pub async fn get_price(
        &self,
        token: &Address,
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<PriceData> {
        // Check cache first
        let cache_key = PriceCacheKey {
            token: token.clone(),
            chain,
            dex: dex.clone(),
        };

        {
            let cache = self.price_cache.read().await;
            if let Some(cached) = cache.get(&cache_key) {
                if cached.expires_at > Utc::now() {
                    tracing::debug!("Cache hit for price: {:?}", cache_key);
                    return Ok(cached.price.clone());
                }
            }
        }

        // Query all enabled feeds
        let mut prices = Vec::new();
        let mut errors = Vec::new();

        for feed in &self.feeds {
            if feed.supported_chains().contains(&chain) {
                match feed.get_price(token, chain, dex.clone()).await {
                    Ok(price) => {
                        // Filter by minimum liquidity
                        if price.liquidity_usd >= self.config.min_liquidity_usd {
                            prices.push(price);
                        }
                    }
                    Err(e) => {
                        errors.push(format!("{}: {}", feed.name(), e));
                    }
                }
            }
        }

        if prices.is_empty() {
            let error_msg = if errors.is_empty() {
                format!("No price feeds support chain {}", chain)
            } else {
                errors.join(", ")
            };
            return Err(Error::MarketData(format!(
                "Failed to get price for {} on {}: {}",
                token, chain, error_msg
            )));
        }

        // Calculate weighted average price based on liquidity
        let total_liquidity: f64 = prices.iter().map(|p| p.liquidity_usd).sum();
        let mut weighted_price = 0.0;
        let mut weighted_confidence = 0.0;
        let mut total_volume = 0.0;

        for price in &prices {
            let weight = price.liquidity_usd / total_liquidity;
            weighted_price += price.price * weight;
            if let Some(confidence) = price.confidence {
                weighted_confidence += confidence * weight;
            }
            if let Some(volume) = price.volume_24h_usd {
                total_volume += volume;
            }
        }

        // Use the price with highest liquidity as the base
        let best_price = prices
            .iter()
            .max_by(|a, b| a.liquidity_usd.partial_cmp(&b.liquidity_usd).unwrap())
            .unwrap();

        let result = PriceData {
            token: token.clone(),
            dex: best_price.dex,
            chain: best_price.chain,
            price: weighted_price,
            confidence: if weighted_confidence > 0.0 {
                Some(weighted_confidence)
            } else {
                None
            },
            liquidity_usd: total_liquidity,
            volume_24h_usd: if total_volume > 0.0 {
                Some(total_volume)
            } else {
                None
            },
            timestamp: Utc::now(),
            source: best_price.source,
        };

        // Update cache
        let cached_data = CachedPriceData {
            price: result.clone(),
            expires_at: Utc::now() + chrono::Duration::seconds(self.config.cache_ttl_secs as i64),
        };

        let mut cache = self.price_cache.write().await;
        cache.insert(cache_key, cached_data);

        Ok(result)
    }

    /// Get token metrics
    pub async fn get_token_metrics(&self, token: &Address, chain: Chain) -> Result<TokenMetrics> {
        // Check cache first
        {
            let cache = self.metrics_cache.read().await;
            if let Some(metrics) = cache.get(token) {
                if metrics.last_updated + chrono::Duration::seconds(self.config.cache_ttl_secs as i64) > Utc::now() {
                    tracing::debug!("Cache hit for token metrics: {}", token);
                    return Ok(metrics.clone());
                }
            }
        }

        // Get price data from all DEXs
        let mut all_prices = Vec::new();
        let mut all_pools = Vec::new();

        for feed in &self.feeds {
            if feed.supported_chains().contains(&chain) {
                // Get price
                if let Ok(price) = feed.get_price(token, chain, None).await {
                    if price.liquidity_usd >= self.config.min_liquidity_usd {
                        all_prices.push(price);
                    }
                }

                // Get liquidity pools
                if let Ok(pools) = feed.get_liquidity_pools(token, chain).await {
                    for pool in pools {
                        if pool.liquidity_usd >= self.config.min_liquidity_usd {
                            all_pools.push(pool);
                        }
                    }
                }
            }
        }

        if all_prices.is_empty() {
            return Err(Error::MarketData(format!(
                "No price data available for token {} on {}",
                token, chain
            )));
        }

        // Calculate metrics
        let total_liquidity: f64 = all_pools.iter().map(|p| p.liquidity_usd).sum();
        let total_volume: f64 = all_pools.iter().map(|p| p.volume_24h_usd).sum();

        // Calculate weighted average price
        let mut weighted_price = 0.0;
        for price in &all_prices {
            let weight = price.liquidity_usd / total_liquidity;
            weighted_price += price.price * weight;
        }

        // TODO: Fetch additional metrics from blockchain and social sources
        // For now, return basic metrics

        let metrics = TokenMetrics {
            token: token.clone(),
            chain,
            price_usd: weighted_price,
            price_change_24h: None, // Would need historical data
            price_change_7d: None,  // Would need historical data
            total_liquidity_usd: total_liquidity,
            volume_24h_usd: total_volume,
            market_cap_usd: None,
            fdv_usd: None,
            holder_count: None,
            top_10_holders_percent: None,
            sentiment_score: None,
            honeypot_risk: None,
            contract_verified: false, // Would need contract verification
            liquidity_locked_percent: None,
            last_updated: Utc::now(),
        };

        // Update cache
        let mut cache = self.metrics_cache.write().await;
        cache.insert(token.clone(), metrics.clone());

        Ok(metrics)
    }

    /// Subscribe to price updates
    pub async fn subscribe_to_price_updates(
        &self,
        token: &Address,
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<tokio::sync::mpsc::Receiver<PriceData>> {
        // Find a feed that supports WebSocket subscription
        for feed in &self.feeds {
            if feed.supported_chains().contains(&chain) {
                match feed.subscribe(&[token.clone()], chain, dex.clone()).await {
                    Ok(receiver) => return Ok(receiver),
                    Err(_) => continue,
                }
            }
        }

        Err(Error::MarketData(
            "No price feed supports WebSocket subscription for the given parameters".to_string(),
        ))
    }

    /// Start background update task
    pub async fn start_background_updates(&self) -> Result<()> {
        let config = self.config.clone();
        let feeds = self.feeds.clone();
        let price_cache = self.price_cache.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(config.update_interval_secs));

            loop {
                interval.tick().await;

                // In a real implementation, we would update subscriptions and refresh cache
                tracing::debug!("Background market data update tick");
            }
        });

        Ok(())
    }
}

/// DEX Screener feed implementation
#[cfg(feature = "dex_screener")]
pub struct DexScreenerFeed {
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

#[cfg(feature = "dex_screener")]
impl DexScreenerFeed {
    /// Create a new DEX Screener feed
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("DEXSCREENER_API_KEY").ok();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::MarketData(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            api_key,
            base_url: "https://api.dexscreener.com".to_string(),
            client,
        })
    }
}

#[cfg(feature = "dex_screener")]
#[async_trait]
impl PriceFeed for DexScreenerFeed {
    fn name(&self) -> &str {
        "dex_screener"
    }

    fn supported_chains(&self) -> Vec<Chain> {
        vec![
            Chain::Ethereum,
            Chain::Bsc,
            Chain::Polygon,
            Chain::Arbitrum,
            Chain::Optimism,
            Chain::Avalanche,
            Chain::Solana,
        ]
    }

    fn supported_dexs(&self) -> Vec<Dex> {
        vec![
            Dex::UniswapV2,
            Dex::UniswapV3,
            Dex::Sushiswap,
            Dex::Pancakeswap,
            Dex::Curve,
            Dex::Balancer,
            Dex::Raydium,
            Dex::Orca,
            Dex::Jupiter,
        ]
    }

    async fn get_price(&self, token: &Address, chain: Chain, dex: Option<Dex>) -> Result<PriceData> {
        // Map chain to DEX Screener chain ID
        let chain_id = match chain {
            Chain::Ethereum => "ethereum",
            Chain::Bsc => "bsc",
            Chain::Polygon => "polygon",
            Chain::Arbitrum => "arbitrum",
            Chain::Optimism => "optimism",
            Chain::Avalanche => "avalanche",
            Chain::Solana => "solana",
            _ => {
                return Err(Error::MarketData(format!(
                    "Unsupported chain for DEX Screener: {}",
                    chain
                )))
            }
        };

        let url = format!("{}/latest/dex/tokens/{}", self.base_url, token.as_str());
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::MarketData(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(Error::MarketData(format!(
                "DEX Screener API error: {}",
                response.status()
            )));
        }

        let data: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::MarketData(format!("Failed to parse response: {}", e)))?;

        // Parse response and extract relevant data
        // Note: This is a simplified implementation
        // In production, we would properly parse the DEX Screener response

        Ok(PriceData {
            token: token.clone(),
            dex: dex.unwrap_or(Dex::UniswapV2),
            chain,
            price: 0.0, // Would be extracted from response
            confidence: None,
            liquidity_usd: 0.0,
            volume_24h_usd: None,
            timestamp: Utc::now(),
            source: PriceSource::DexPool,
        })
    }

    async fn get_prices(
        &self,
        tokens: &[Address],
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<Vec<PriceData>> {
        let mut prices = Vec::new();
        for token in tokens {
            match self.get_price(token, chain, dex.clone()).await {
                Ok(price) => prices.push(price),
                Err(e) => tracing::warn!("Failed to get price for {}: {}", token, e),
            }
        }
        Ok(prices)
    }

    async fn subscribe(
        &self,
        tokens: &[Address],
        chain: Chain,
        dex: Option<Dex>,
    ) -> Result<tokio::sync::mpsc::Receiver<PriceData>> {
        // DEX Screener doesn't have WebSocket API in free tier
        // Return a channel that never receives anything
        let (tx, rx) = tokio::sync::mpsc::channel(100);
        drop(tx); // Immediately drop sender to close channel
        Ok(rx)
    }

    async fn get_liquidity_pools(
        &self,
        token: &Address,
        chain: Chain,
    ) -> Result<Vec<LiquidityPool>> {
        // Similar implementation to get_price but for pools
        Ok(vec![])
    }
}

/// CoinGecko feed implementation
#[cfg(feature = "coingecko")]
pub struct CoinGeckoFeed {
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

#[cfg(feature = "coingecko")]
impl CoinGeckoFeed {
    /// Create a new CoinGecko feed
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("COINGECKO_API_KEY").ok();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::MarketData(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            api_key,
            base_url: "https://api.coingecko.com".to_string(),
            client,
        })
    }
}

/// Chainlink feed implementation
#[cfg(feature = "chainlink")]
pub struct ChainlinkFeed {
    // Chainlink oracle addresses by chain
    oracles: HashMap<Chain, Address>,
}

#[cfg(feature = "chainlink")]
impl ChainlinkFeed {
    /// Create a new Chainlink feed
    pub fn new() -> Result<Self> {
        let mut oracles = HashMap::new();

        // Ethereum mainnet ETH/USD oracle
        oracles.insert(
            Chain::Ethereum,
            Address::new("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419").unwrap(),
        );

        // Add more oracles as needed

        Ok(Self { oracles })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dex_properties() {
        assert_eq!(Dex::UniswapV2.name(), "Uniswap V2");
        assert_eq!(Dex::UniswapV2.default_chain(), Chain::Ethereum);
        assert!(Dex::UniswapV2.supports_chain(Chain::Ethereum));
        assert!(!Dex::UniswapV2.supports_chain(Chain::Solana));
    }

    #[test]
    fn test_market_data_config_default() {
        let config = MarketDataConfig::default();
        assert_eq!(config.update_interval_secs, 30);
        assert_eq!(config.cache_ttl_secs, 300);
        assert!(config.websocket_enabled);
        assert_eq!(config.min_liquidity_usd, 10000.0);
    }

    #[tokio::test]
    async fn test_market_data_aggregator_new() {
        let config = MarketDataConfig::default();
        let result = MarketDataAggregator::new(config);

        // Should succeed even without any feeds enabled (in test configuration)
        assert!(result.is_ok());
    }
}
