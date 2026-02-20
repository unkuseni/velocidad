//! Trading engine module for the Velocidad trading bot backend.
//!
//! This module provides the core trading functionality including:
//! - Trade execution with risk management
//! - Snipe trading (token launches)
//! - Arbitrage detection and execution
//! - Strategy execution framework
//! - Risk management and position sizing

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use crate::blockchain::{Address, BlockchainClient, Chain, TransactionReceipt};
use crate::database::{Trade, TradeRepository, TradeStatus, UserRepository};
use crate::error::{Error as AppError, Result as AppResult};
use crate::market::{Dex, MarketDataAggregator, PriceData, TokenMetrics};
use crate::prelude::*;

/// Trading-specific errors
#[derive(Error, Debug)]
pub enum TradingError {
    /// Insufficient funds for trade
    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),

    /// Slippage tolerance exceeded
    #[error("Slippage tolerance exceeded: expected {expected}, got {actual}")]
    SlippageExceeded {
        expected: f64,
        actual: f64,
    },

    /// Risk limit exceeded
    #[error("Risk limit exceeded: {0}")]
    RiskLimit(String),

    /// Invalid trade parameters
    #[error("Invalid trade parameters: {0}")]
    InvalidParameters(String),

    /// Trade execution timeout
    #[error("Trade execution timeout after {0:?}")]
    Timeout(Duration),

    /// MEV protection failed
    #[error("MEV protection failed: {0}")]
    MevProtectionFailed(String),

    /// Token validation failed
    #[error("Token validation failed: {0}")]
    TokenValidationFailed(String),

    /// No profitable arbitrage found
    #[error("No profitable arbitrage found")]
    NoProfitableArbitrage,

    /// Strategy execution error
    #[error("Strategy execution error: {0}")]
    StrategyError(String),

    /// Market data unavailable
    #[error("Market data unavailable: {0}")]
    MarketDataUnavailable(String),

    /// Blockchain error
    #[error("Blockchain error: {0}")]
    Blockchain(String),

    /// Internal error
    #[error("Internal trading error: {0}")]
    Internal(String),
}

impl From<TradingError> for AppError {
    fn from(err: TradingError) -> Self {
        AppError::Trading(err.to_string())
    }
}

/// Convenience type alias for trading results
pub type Result<T> = std::result::Result<T, TradingError>;

/// Configuration for snipe trading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnipeConfig {
    /// Token address to snipe
    pub token_address: Address,
    /// Chain to execute on
    pub chain: Chain,
    /// Amount to spend (in native token)
    pub amount: f64,
    /// Maximum slippage percentage
    pub max_slippage_percent: f64,
    /// Enable MEV protection
    pub mev_protection: bool,
    /// Use flashbots (if mev_protection enabled)
    pub use_flashbots: bool,
    /// Gas price multiplier
    pub gas_price_multiplier: f64,
    /// Timeout for execution
    pub timeout_secs: u64,
    /// Validate token before execution
    pub validate_token: bool,
}

impl Default for SnipeConfig {
    fn default() -> Self {
        Self {
            token_address: Address::new("0x0").unwrap(),
            chain: Chain::Ethereum,
            amount: 0.0,
            max_slippage_percent: 5.0,
            mev_protection: true,
            use_flashbots: false,
            gas_price_multiplier: 1.2,
            timeout_secs: 30,
            validate_token: true,
        }
    }
}

/// Result of a trade execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeResult {
    /// Trade ID
    pub trade_id: Uuid,
    /// Transaction hash
    pub tx_hash: Option<String>,
    /// Execution status
    pub status: TradeStatus,
    /// Amount in (native token)
    pub amount_in: f64,
    /// Amount out (target token)
    pub amount_out: f64,
    /// Effective price
    pub effective_price: f64,
    /// Slippage percentage
    pub slippage_percent: f64,
    /// Gas used
    pub gas_used: Option<u64>,
    /// Gas price (in Gwei)
    pub gas_price: Option<f64>,
    /// Profit/loss in USD (if applicable)
    pub profit_loss_usd: Option<f64>,
    /// Execution timestamp
    pub execution_time: chrono::DateTime<chrono::Utc>,
    /// Error message (if failed)
    pub error_message: Option<String>,
}

/// Arbitrage opportunity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArbitrageOpportunity {
    /// Source DEX
    pub source_dex: Dex,
    /// Destination DEX
    pub dest_dex: Dex,
    /// Token address
    pub token_address: Address,
    /// Chain
    pub chain: Chain,
    /// Expected profit percentage
    pub profit_percent: f64,
    /// Expected profit in USD
    pub profit_usd: f64,
    /// Required capital in USD
    pub required_capital_usd: f64,
    /// Estimated execution time (seconds)
    pub estimated_execution_time_secs: f64,
    /// Risk score (0-10, lower is better)
    pub risk_score: u8,
    /// Opportunity expiration timestamp
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Trading strategy trait
#[async_trait]
pub trait TradingStrategy: Send + Sync {
    /// Get strategy name
    fn name(&self) -> &str;

    /// Get strategy description
    fn description(&self) -> &str;

    /// Check if strategy is active
    fn is_active(&self) -> bool;

    /// Execute the strategy
    async fn execute(&mut self, context: &StrategyContext) -> Result<Vec<TradeResult>>;

    /// Get performance metrics
    fn performance_metrics(&self) -> HashMap<String, f64>;

    /// Update strategy configuration
    fn update_config(&mut self, config: serde_json::Value) -> Result<()>;
}

/// Strategy execution context
#[derive(Debug, Clone)]
pub struct StrategyContext {
    /// Market data aggregator
    pub market_data: Arc<MarketDataAggregator>,
    /// Blockchain clients
    pub blockchain_clients: HashMap<Chain, Arc<dyn BlockchainClient>>,
    /// User ID
    pub user_id: Uuid,
    /// Available capital by chain
    pub available_capital: HashMap<Chain, f64>,
    /// Current timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Risk manager configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskManagerConfig {
    /// Daily loss limit (percentage of portfolio)
    pub daily_loss_limit_percent: f64,
    /// Maximum position size (percentage of portfolio)
    pub max_position_size_percent: f64,
    /// Maximum slippage percentage
    pub max_slippage_percent: f64,
    /// Blacklisted tokens
    pub blacklisted_tokens: Vec<Address>,
    /// Minimum liquidity USD for trading
    pub min_liquidity_usd: f64,
    /// Maximum gas price (Gwei)
    pub max_gas_price_gwei: f64,
    /// Maximum trade size USD
    pub max_trade_size_usd: f64,
    /// Cooldown period between trades (seconds)
    pub trade_cooldown_secs: u64,
}

impl Default for RiskManagerConfig {
    fn default() -> Self {
        Self {
            daily_loss_limit_percent: 5.0,
            max_position_size_percent: 20.0,
            max_slippage_percent: 10.0,
            blacklisted_tokens: Vec::new(),
            min_liquidity_usd: 10000.0,
            max_gas_price_gwei: 100.0,
            max_trade_size_usd: 10000.0,
            trade_cooldown_secs: 60,
        }
    }
}

/// Risk manager for validating trades
pub struct RiskManager {
    /// Configuration
    config: RiskManagerConfig,
    /// User risk profiles
    user_risk_profiles: RwLock<HashMap<Uuid, UserRiskProfile>>,
    /// Trade history for cooldown tracking
    trade_history: Mutex<HashMap<Uuid, Vec<Instant>>>,
}

/// User risk profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRiskProfile {
    /// User ID
    pub user_id: Uuid,
    /// Daily P&L (USD)
    pub daily_pnl_usd: f64,
    /// Total P&L (USD)
    pub total_pnl_usd: f64,
    /// Risk tolerance (1-10)
    pub risk_tolerance: u8,
    /// Maximum daily loss (USD)
    pub max_daily_loss_usd: f64,
    /// Current positions
    pub current_positions: HashMap<Address, Position>,
}

/// Position information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// Token address
    pub token_address: Address,
    /// Amount held
    pub amount: f64,
    /// Average entry price (USD)
    pub avg_entry_price_usd: f64,
    /// Current price (USD)
    pub current_price_usd: f64,
    /// Unrealized P&L (USD)
    pub unrealized_pnl_usd: f64,
}

impl RiskManager {
    /// Create a new risk manager
    pub fn new(config: RiskManagerConfig) -> Self {
        Self {
            config,
            user_risk_profiles: RwLock::new(HashMap::new()),
            trade_history: Mutex::new(HashMap::new()),
        }
    }

    /// Validate a trade against risk limits
    #[instrument(skip(self, trade_params))]
    pub async fn validate_trade(
        &self,
        user_id: Uuid,
        trade_params: &TradeParameters,
    ) -> Result<()> {
        let user_profile = self.get_user_profile(user_id).await?;

        // Check daily loss limit
        if user_profile.daily_pnl_usd < -user_profile.max_daily_loss_usd {
            return Err(TradingError::RiskLimit(format!(
                "Daily loss limit exceeded: {} USD",
                user_profile.daily_pnl_usd
            )));
        }

        // Check position size limit
        let position_size_percent =
            (trade_params.amount_usd / self.get_portfolio_value(user_id).await?) * 100.0;
        if position_size_percent > self.config.max_position_size_percent {
            return Err(TradingError::RiskLimit(format!(
                "Position size limit exceeded: {}% > {}%",
                position_size_percent, self.config.max_position_size_percent
            )));
        }

        // Check slippage
        if trade_params.max_slippage_percent > self.config.max_slippage_percent {
            return Err(TradingError::RiskLimit(format!(
                "Slippage limit exceeded: {}% > {}%",
                trade_params.max_slippage_percent, self.config.max_slippage_percent
            )));
        }

        // Check trade cooldown
        if let Some(last_trades) = self.trade_history.lock().await.get(&user_id) {
            if let Some(last_trade) = last_trades.last() {
                let cooldown = Duration::from_secs(self.config.trade_cooldown_secs);
                if last_trade.elapsed() < cooldown {
                    return Err(TradingError::RiskLimit(format!(
                        "Trade cooldown active: {} seconds remaining",
                        cooldown.as_secs() - last_trade.elapsed().as_secs()
                    )));
                }
            }
        }

        // Check blacklisted tokens
        if self.config.blacklisted_tokens.contains(&trade_params.token_address) {
            return Err(TradingError::RiskLimit(format!(
                "Token {} is blacklisted",
                trade_params.token_address
            )));
        }

        Ok(())
    }

    /// Update user risk profile
    pub async fn update_user_profile(&self, user_id: Uuid, pnl_usd: f64) -> Result<()> {
        let mut profiles = self.user_risk_profiles.write().await;
        if let Some(profile) = profiles.get_mut(&user_id) {
            profile.daily_pnl_usd += pnl_usd;
            profile.total_pnl_usd += pnl_usd;
        } else {
            profiles.insert(
                user_id,
                UserRiskProfile {
                    user_id,
                    daily_pnl_usd: pnl_usd,
                    total_pnl_usd: pnl_usd,
                    risk_tolerance: 5,
                    max_daily_loss_usd: 1000.0,
                    current_positions: HashMap::new(),
                },
            );
        }
        Ok(())
    }

    /// Record a trade for cooldown tracking
    pub async fn record_trade(&self, user_id: Uuid) {
        let mut history = self.trade_history.lock().await;
        history.entry(user_id).or_insert_with(Vec::new).push(Instant::now());
    }

    /// Get user risk profile
    async fn get_user_profile(&self, user_id: Uuid) -> Result<UserRiskProfile> {
        let profiles = self.user_risk_profiles.read().await;
        profiles
            .get(&user_id)
            .cloned()
            .ok_or_else(|| TradingError::Internal(format!("User {} profile not found", user_id)))
    }

    /// Get portfolio value (simplified)
    async fn get_portfolio_value(&self, user_id: Uuid) -> Result<f64> {
        let profile = self.get_user_profile(user_id).await?;
        let positions_value: f64 = profile
            .current_positions
            .values()
            .map(|p| p.amount * p.current_price_usd)
            .sum();
        Ok(positions_value + 10000.0) // Simplified: assume 10k USD cash
    }
}

/// Trade parameters
#[derive(Debug, Clone)]
pub struct TradeParameters {
    /// Token address
    pub token_address: Address,
    /// Chain
    pub chain: Chain,
    /// Amount in USD
    pub amount_usd: f64,
    /// Maximum slippage percentage
    pub max_slippage_percent: f64,
    /// Use limit order
    pub use_limit_order: bool,
    /// Limit price (if use_limit_order)
    pub limit_price_usd: Option<f64>,
    /// Target profit percentage (for auto-sell)
    pub target_profit_percent: Option<f64>,
    /// Stop loss percentage
    pub stop_loss_percent: Option<f64>,
}

/// DEX aggregator trait
#[async_trait]
pub trait DexAggregator: Send + Sync {
    /// Get best price for a token swap
    async fn get_best_price(
        &self,
        chain: Chain,
        token_in: Address,
        token_out: Address,
        amount: f64,
    ) -> Result<PriceData>;

    /// Get swap route
    async fn get_swap_route(
        &self,
        chain: Chain,
        token_in: Address,
        token_out: Address,
        amount: f64,
    ) -> Result<SwapRoute>;

    /// Execute swap
    async fn execute_swap(
        &self,
        route: &SwapRoute,
        slippage_percent: f64,
    ) -> Result<TransactionReceipt>;
}

/// Swap route information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapRoute {
    /// Chain
    pub chain: Chain,
    /// Token in address
    pub token_in: Address,
    /// Token out address
    pub token_out: Address,
    /// Amount in
    pub amount_in: f64,
    /// Expected amount out
    pub expected_amount_out: f64,
    /// Best DEX to use
    pub best_dex: Dex,
    /// Alternative DEXes
    pub alternative_dexes: Vec<Dex>,
    /// Route path (for multi-hop swaps)
    pub route_path: Vec<Address>,
    /// Estimated gas
    pub estimated_gas: u64,
    /// Price impact percentage
    pub price_impact_percent: f64,
}

/// Order manager for tracking and managing orders
pub struct OrderManager {
    /// Active orders
    active_orders: RwLock<HashMap<Uuid, ActiveOrder>>,
    /// Order history
    order_history: Mutex<Vec<CompletedOrder>>,
    /// Order event channel
    order_events: mpsc::UnboundedSender<OrderEvent>,
}

/// Active order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveOrder {
    /// Order ID
    pub order_id: Uuid,
    /// User ID
    pub user_id: Uuid,
    /// Trade parameters
    pub params: TradeParameters,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Status
    pub status: OrderStatus,
    /// Last update timestamp
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Completed order
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedOrder {
    /// Order ID
    pub order_id: Uuid,
    /// User ID
    pub user_id: Uuid,
    /// Trade result
    pub result: TradeResult,
    /// Completion timestamp
    pub completed_at: chrono::DateTime<chrono::Utc>,
}

/// Order status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order pending execution
    Pending,
    /// Order executing
    Executing,
    /// Order filled
    Filled,
    /// Order partially filled
    PartiallyFilled,
    /// Order cancelled
    Cancelled,
    /// Order failed
    Failed,
    /// Order expired
    Expired,
}

/// Order event
#[derive(Debug, Clone)]
pub enum OrderEvent {
    /// Order created
    OrderCreated(ActiveOrder),
    /// Order updated
    OrderUpdated(ActiveOrder),
    /// Order completed
    OrderCompleted(CompletedOrder),
    /// Order cancelled
    OrderCancelled(Uuid),
}

impl OrderManager {
    /// Create a new order manager
    pub fn new() -> (Self, mpsc::UnboundedReceiver<OrderEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                active_orders: RwLock::new(HashMap::new()),
                order_history: Mutex::new(Vec::new()),
                order_events: tx,
            },
            rx,
        )
    }

    /// Create a new order
    pub async fn create_order(&self, user_id: Uuid, params: TradeParameters) -> Result<Uuid> {
        let order_id = Uuid::new_v4();
        let order = ActiveOrder {
            order_id,
            user_id,
            params,
            created_at: chrono::Utc::now(),
            status: OrderStatus::Pending,
            updated_at: chrono::Utc::now(),
        };

        {
            let mut orders = self.active_orders.write().await;
            orders.insert(order_id, order.clone());
        }

        let _ = self.order_events.send(OrderEvent::OrderCreated(order));
        Ok(order_id)
    }

    /// Update order status
    pub async fn update_order_status(&self, order_id: Uuid, status: OrderStatus) -> Result<()> {
        let mut orders = self.active_orders.write().await;
        if let Some(order) = orders.get_mut(&order_id) {
            order.status = status;
            order.updated_at = chrono::Utc::now();

            let order_clone = order.clone();
            drop(orders); // Release lock before sending event

            let _ = self.order_events.send(OrderEvent::OrderUpdated(order_clone));
            Ok(())
        } else {
            Err(TradingError::Internal(format!(
                "Order {} not found",
                order_id
            )))
        }
    }

    /// Complete an order
    pub async fn complete_order(&self, order_id: Uuid, result: TradeResult) -> Result<()> {
        let order = {
            let mut orders = self.active_orders.write().await;
            orders.remove(&order_id)
        };

        if let Some(order) = order {
            let completed_order = CompletedOrder {
                order_id,
                user_id: order.user_id,
                result,
                completed_at: chrono::Utc::now(),
            };

            {
                let mut history = self.order_history.lock().await;
                history.push(completed_order.clone());
            }

            let _ = self
                .order_events
                .send(OrderEvent::OrderCompleted(completed_order));
            Ok(())
        } else {
            Err(TradingError::Internal(format!(
                "Order {} not found",
                order_id
            )))
        }
    }

    /// Cancel an order
    pub async fn cancel_order(&self, order_id: Uuid) -> Result<()> {
        let existed = {
            let mut orders = self.active_orders.write().await;
            orders.remove(&order_id).is_some()
        };

        if existed {
            let _ = self.order_events.send(OrderEvent::OrderCancelled(order_id));
            Ok(())
        } else {
            Err(TradingError::Internal(format!(
                "Order {} not found",
                order_id
            )))
        }
    }

    /// Get active orders for user
    pub async fn get_user_orders(&self, user_id: Uuid) -> Vec<ActiveOrder> {
        let orders = self.active_orders.read().await;
        orders
            .values()
            .filter(|order| order.user_id == user_id)
            .cloned()
            .collect()
    }
}

/// Main trading engine
pub struct TradingEngine {
    /// Risk manager
    pub risk_manager: Arc<RiskManager>,
    /// Order manager
    pub order_manager: Arc<OrderManager>,
    /// Market data aggregator
    pub market_data: Arc<MarketDataAggregator>,
    /// Blockchain clients by chain
    pub blockchain_clients: HashMap<Chain, Arc<dyn BlockchainClient>>,
    /// DEX aggregator
    pub dex_aggregator: Arc<dyn DexAggregator>,
    /// Active strategies
    pub strategies: RwLock<HashMap<String, Box<dyn TradingStrategy>>>,
    /// Database repository for trades
    pub trade_repository: Arc<dyn TradeRepository>,
    /// User repository
    pub user_repository: Arc<dyn UserRepository>,
    /// Is engine running
    pub is_running: Mutex<bool>,
}

impl TradingEngine {
    /// Create a new trading engine
    pub fn new(
        risk_manager: Arc<RiskManager>,
        order_manager: Arc<OrderManager>,
        market_data: Arc<MarketDataAggregator>,
        blockchain_clients: HashMap<Chain, Arc<dyn BlockchainClient>>,
        dex_aggregator: Arc<dyn DexAggregator>,
        trade_repository: Arc<dyn TradeRepository>,
        user_repository: Arc<dyn UserRepository>,
    ) -> Self {
        Self {
            risk_manager,
            order_manager,
            market_data,
            blockchain_clients,
            dex_aggregator,
            strategies: RwLock::new(HashMap::new()),
            trade_repository,
            user_repository,
            is_running: Mutex::new(false),
        }
    }

    /// Execute a snipe trade
    #[instrument(skip(self, config))]
    pub async fn execute_snipe(&self, user_id: Uuid, config: SnipeConfig) -> Result<TradeResult> {
        info!(user_id = %user_id, token_address = %config.token_address, "Starting snipe execution");

        // 1. Token validation
        if config.validate_token {
            self.validate_token(&config.token_address, config.chain)
                .await
                .context("Token validation failed")?;
        }

        // 2. Get token metrics
        let metrics = self
            .market_data
            .get_token_metrics(config.token_address.clone(), config.chain)
            .await
            .map_err(|e| TradingError::MarketDataUnavailable(e.to_string()))?;

        // 3. Validate liquidity
        if metrics.total_liquidity_usd < 10000.0 {
            return Err(TradingError::TokenValidationFailed(format!(
                "Insufficient liquidity: {} USD",
                metrics.total_liquidity_usd
            )));
        }

        // 4. Get best price
        let native_token = match config.chain {
            Chain::Ethereum | Chain::EthereumGoerli | Chain::EthereumSepolia => {
                Address::new("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap()
            }
            Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet => {
                Address::new("So11111111111111111111111111111111111111112").unwrap()
            }
            _ => Address::new("0x0").unwrap(),
        };

        let price_data = self
            .dex_aggregator
            .get_best_price(
                config.chain,
                native_token.clone(),
                config.token_address.clone(),
                config.amount,
            )
            .await?;

        // 5. Validate slippage
        let expected_amount = config.amount / price_data.price;
        if (expected_amount - price_data.confidence).abs() / expected_amount * 100.0
            > config.max_slippage_percent
        {
            return Err(TradingError::SlippageExceeded {
                expected: expected_amount,
                actual: price_data.confidence,
            });
        }

        // 6. Execute trade
        let trade_result = self
            .execute_trade(
                user_id,
                TradeParameters {
                    token_address: config.token_address.clone(),
                    chain: config.chain,
                    amount_usd: config.amount * 2000.0, // Approximate ETH price
                    max_slippage_percent: config.max_slippage_percent,
                    use_limit_order: false,
                    limit_price_usd: None,
                    target_profit_percent: Some(20.0),
                    stop_loss_percent: Some(10.0),
                },
            )
            .await?;

        info!(user_id = %user_id, trade_id = %trade_result.trade_id, "Snipe execution completed");
        Ok(trade_result)
    }

    /// Find arbitrage opportunities
    #[instrument(skip(self))]
    pub async fn find_arbitrage(&self) -> Result<Vec<ArbitrageOpportunity>> {
        debug!("Searching for arbitrage opportunities");

        // This is a simplified implementation
        // In production, this would:
        // 1. Monitor multiple DEXes for price differences
        // 2. Calculate profitable routes
        // 3. Consider gas costs and execution time
        // 4. Filter by risk score

        let opportunities = vec![ArbitrageOpportunity {
            source_dex: Dex::UniswapV2,
            dest_dex: Dex::Sushiswap,
            token_address: Address::new("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(), // USDC
            chain: Chain::Ethereum,
            profit_percent: 0.5,
            profit_usd: 50.0,
            required_capital_usd: 10000.0,
            estimated_execution_time_secs: 5.0,
            risk_score: 3,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(30),
        }];

        if opportunities.is_empty() {
            Err(TradingError::NoProfitableArbitrage)
        } else {
            Ok(opportunities)
        }
    }

    /// Execute arbitrage opportunity
    #[instrument(skip(self, opportunity))]
    pub async fn execute_arbitrage(
        &self,
        user_id: Uuid,
        opportunity: &ArbitrageOpportunity,
    ) -> Result<TradeResult> {
        info!(user_id = %user_id, token_address = %opportunity.token_address, "Executing arbitrage");

        // Simplified implementation
        // In production, this would:
        // 1. Execute buy on source DEX
        // 2. Execute sell on destination DEX
        // 3. Handle atomic execution (or use flash loans)
        // 4. Monitor for MEV

        // For now, simulate a successful trade
        let trade_result = TradeResult {
            trade_id: Uuid::new_v4(),
            tx_hash: Some(format!("0x{:064x}", rand::random::<u128>())),
            status: TradeStatus::Executed,
            amount_in: opportunity.required_capital_usd / 2000.0, // Approx ETH
            amount_out: (opportunity.required_capital_usd + opportunity.profit_usd) / 2000.0,
            effective_price: 2000.0,
            slippage_percent: 0.1,
            gas_used: Some(150000),
            gas_price: Some(50.0),
            profit_loss_usd: Some(opportunity.profit_usd),
            execution_time: chrono::Utc::now(),
            error_message: None,
        };

        Ok(trade_result)
    }

    /// Run a trading strategy
    #[instrument(skip(self, strategy_name))]
    pub async fn run_strategy(&self, user_id: Uuid, strategy_name: &str) -> Result<Vec<TradeResult>> {
        info!(user_id = %user_id, strategy_name, "Running trading strategy");

        let strategies = self.strategies.read().await;
        let strategy = strategies
            .get(strategy_name)
            .ok_or_else(|| TradingError::StrategyError(format!("Strategy {} not found", strategy_name)))?;

        if !strategy.is_active() {
            return Err(TradingError::StrategyError(format!(
                "Strategy {} is not active",
                strategy_name
            )));
        }

        // Create strategy context
        let context = StrategyContext {
            market_data: self.market_data.clone(),
            blockchain_clients: self.blockchain_clients.clone(),
            user_id,
            available_capital: self.get_user_capital(user_id).await?,
            timestamp: chrono::Utc::now(),
        };

        // Execute strategy
        let results = strategy.execute(&context).await?;

        info!(user_id = %user_id, strategy_name, num_trades = results.len(), "Strategy execution completed");
        Ok(results)
    }

    /// Register a trading strategy
    pub async fn register_strategy(
        &self,
        name: String,
        strategy: Box<dyn TradingStrategy>,
    ) -> Result<()> {
        let mut strategies = self.strategies.write().await;
        if strategies.contains_key(&name) {
            return Err(TradingError::StrategyError(format!(
                "Strategy {} already exists",
                name
            )));
        }
        strategies.insert(name, strategy);
        Ok(())
    }

    /// Remove a trading strategy
    pub async fn remove_strategy(&self, name: &str) -> Result<()> {
        let mut strategies = self.strategies.write().await;
        strategies
            .remove(name)
            .ok_or_else(|| TradingError::StrategyError(format!("Strategy {} not found", name)))?;
        Ok(())
    }

    /// Start the trading engine
    pub async fn start(&self) -> Result<()> {
        let mut is_running = self.is_running.lock().await;
        if *is_running {
            return Err(TradingError::Internal("Engine already running".to_string()));
        }

        *is_running = true;
        info!("Trading engine started");

        // Start background tasks here
        // e.g., market monitoring, order processing, etc.

        Ok(())
    }

    /// Stop the trading engine
    pub async fn stop(&self) -> Result<()> {
        let mut is_running = self.is_running.lock().await;
        if !*is_running {
            return Err(TradingError::Internal("Engine not running".to_string()));
        }

        *is_running = false;
        info!("Trading engine stopped");

        // Stop background tasks here

        Ok(())
    }

    /// Execute a trade with full validation
    async fn execute_trade(&self, user_id: Uuid, params: TradeParameters) -> Result<TradeResult> {
        // 1. Risk validation
        self.risk_manager
            .validate_trade(user_id, &params)
            .await
            .context("Risk validation failed")?;

        // 2. Create order
        let order_id = self
            .order_manager
            .create_order(user_id, params.clone())
            .await?;

        // 3. Get swap route
        let native_token = self.get_native_token(params.chain);
        let route = self
            .dex_aggregator
            .get_swap_route(
                params.chain,
                native_token,
                params.token_address.clone(),
                params.amount_usd / 2000.0, // Convert USD to ETH approx
            )
            .await?;

        // 4. Execute swap
        let receipt = self
            .dex_aggregator
            .execute_swap(&route, params.max_slippage_percent)
            .await?;

        // 5. Create trade result
        let trade_result = TradeResult {
            trade_id: Uuid::new_v4(),
            tx_hash: Some(receipt.hash.clone()),
            status: TradeStatus::Executed,
            amount_in: route.amount_in,
            amount_out: route.expected_amount_out,
            effective_price: route.amount_in / route.expected_amount_out,
            slippage_percent: route.price_impact_percent,
            gas_used: Some(receipt.gas_used),
            gas_price: Some(receipt.effective_gas_price as f64 / 1e9), // Convert to Gwei
            profit_loss_usd: None, // Would need price data to calculate
            execution_time: chrono::Utc::now(),
            error_message: None,
        };

        // 6. Update order
        self.order_manager
            .complete_order(order_id, trade_result.clone())
            .await?;

        // 7. Record trade for cooldown
        self.risk_manager.record_trade(user_id).await;

        Ok(trade_result)
    }

    /// Validate token for trading
    async fn validate_token(&self, token_address: &Address, chain: Chain) -> Result<()> {
        // Check if contract is deployed
        let client = self
            .blockchain_clients
            .get(&chain)
            .ok_or_else(|| TradingError::Blockchain(format!("No client for chain {:?}", chain)))?;

        let is_deployed = client
            .is_contract_deployed(token_address.clone())
            .await
            .map_err(|e| TradingError::Blockchain(e.to_string()))?;

        if !is_deployed {
            return Err(TradingError::TokenValidationFailed(
                "Contract not deployed".to_string(),
            ));
        }

        // Check for honeypot patterns (simplified)
        let metrics = self
            .market_data
            .get_token_metrics(token_address.clone(), chain)
            .await
            .map_err(|e| TradingError::MarketDataUnavailable(e.to_string()))?;

        if metrics.honeypot_risk > 0.7 {
            return Err(TradingError::TokenValidationFailed(format!(
                "High honeypot risk: {}",
                metrics.honeypot_risk
            )));
        }

        Ok(())
    }

    /// Get user's available capital by chain
    async fn get_user_capital(&self, user_id: Uuid) -> Result<HashMap<Chain, f64>> {
        // Simplified implementation
        // In production, this would query the user's wallet balances
        let mut capital = HashMap::new();
        capital.insert(Chain::Ethereum, 10000.0); // 10 ETH approx
        capital.insert(Chain::Solana, 5000.0); // 5000 USD worth of SOL
        Ok(capital)
    }

    /// Get native token address for chain
    fn get_native_token(&self, chain: Chain) -> Address {
        match chain {
            Chain::Ethereum | Chain::EthereumGoerli | Chain::EthereumSepolia => {
                Address::new("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2").unwrap()
            }
            Chain::Solana | Chain::SolanaDevnet | Chain::SolanaTestnet => {
                Address::new("So11111111111111111111111111111111111111112").unwrap()
            }
            Chain::Arbitrum => Address::new("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(),
            Chain::Polygon => Address::new("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270").unwrap(),
            Chain::Optimism => Address::new("0x4200000000000000000000000000000000000006").unwrap(),
            Chain::Base => Address::new("0x4200000000000000000000000000000000000006").unwrap(),
            Chain::Avalanche => Address::new("0xB31f66AA3C1e785363F0875A1B74E27b85FD66c7").unwrap(),
            Chain::Bsc => Address::new("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c").unwrap(),
        }
    }
}

/// Extension trait for adding context to trading results
pub trait Context<T> {
    /// Add context to an error
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static;
}

impl<T> Context<T> for Result<T> {
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
    {
        self.map_err(|e| TradingError::Internal(format!("{}: {}", context, e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockall::predicate::*;
    use mockall::*;

    #[tokio::test]
    async fn test_snipe_config_default() {
        let config = SnipeConfig::default();
        assert_eq!(config.max_slippage_percent, 5.0);
        assert_eq!(config.mev_protection, true);
        assert_eq!(config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_risk_manager_validation() {
        let config = RiskManagerConfig::default();
        let risk_manager = RiskManager::new(config);

        let user_id = Uuid::new_v4();
        let params = TradeParameters {
            token_address: Address::new("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            chain: Chain::Ethereum,
            amount_usd: 1000.0,
            max_slippage_percent: 5.0,
            use_limit_order: false,
            limit_price_usd: None,
            target_profit_percent: None,
            stop_loss_percent: None,
        };

        // Should fail because user profile doesn't exist yet
        let result = risk_manager.validate_trade(user_id, &params).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_order_manager_flow() {
        let (order_manager, mut rx) = OrderManager::new();

        let user_id = Uuid::new_v4();
        let params = TradeParameters {
            token_address: Address::new("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48").unwrap(),
            chain: Chain::Ethereum,
            amount_usd: 1000.0,
            max_slippage_percent: 5.0,
            use_limit_order: false,
            limit_price_usd: None,
            target_profit_percent: None,
            stop_loss_percent: None,
        };

        // Create order
        let order_id = order_manager
            .create_order(user_id, params.clone())
            .await
            .unwrap();

        // Check event
        match rx.try_recv() {
            Ok(OrderEvent::OrderCreated(order)) => {
                assert_eq!(order.order_id, order_id);
                assert_eq!(order.user_id, user_id);
            }
            _ => panic!("Expected OrderCreated event"),
        }

        // Get user orders
        let orders = order_manager.get_user_orders(user_id).await;
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].order_id, order_id);

        // Cancel order
        order_manager.cancel_order(order_id).await.unwrap();

        // Check event
        match rx.try_recv() {
            Ok(OrderEvent::OrderCancelled(id)) => {
                assert_eq!(id, order_id);
            }
            _ => panic!("Expected OrderCancelled event"),
        }

        // Order should be removed
        let orders = order_manager.get_user_orders(user_id).await;
        assert!(orders.is_empty());
    }
}
