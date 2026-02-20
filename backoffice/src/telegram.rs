//! Telegram bot module for command handling and user interaction.
//!
//! This module provides Telegram bot integration using the teloxide crate,
//! with support for trading commands, portfolio management, alerts, and
//! real-time notifications.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use teloxide::dispatching::UpdateFilterExt;
use teloxide::dptree;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};
use teloxide::utils::command::BotCommand;
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{Error, Result};
use crate::prelude::*;

/// Telegram bot configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot token from @BotFather
    pub token: String,
    /// Webhook URL for production (optional)
    pub webhook_url: Option<String>,
    /// List of allowed Telegram user IDs (empty = allow all)
    pub allowed_users: Vec<i64>,
    /// List of admin Telegram user IDs
    pub admin_users: Vec<i64>,
    /// Channel ID for logging (optional)
    pub log_channel_id: Option<i64>,
    /// Channel ID for alerts (optional)
    pub alert_channel_id: Option<i64>,
    /// Maximum message length (Telegram limit is 4096)
    pub max_message_length: usize,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            webhook_url: None,
            allowed_users: Vec::new(),
            admin_users: Vec::new(),
            log_channel_id: None,
            alert_channel_id: None,
            max_message_length: 4000, // Slightly under Telegram limit
        }
    }
}

/// Telegram bot commands
#[derive(BotCommand, Clone, Debug, PartialEq, Eq)]
#[command(
    rename = "lowercase",
    description = "Velocidad Trading Bot Commands:",
    parse_with = "split"
)]
pub enum Command {
    /// Start the bot and show welcome message
    #[command(description = "Start the bot")]
    Start,
    /// Help - show all commands
    #[command(description = "Show this help message")]
    Help,
    /// Portfolio - show current portfolio
    #[command(description = "Show your portfolio")]
    Portfolio,
    /// Snip - execute a token snipe
    #[command(description = "Execute a token snipe")]
    Snip,
    /// Limit - place a limit order
    #[command(description = "Place a limit order")]
    Limit,
    /// Chart - show price chart for a token
    #[command(description = "Show price chart for a token")]
    Chart,
    /// Arbitrage - find arbitrage opportunities
    #[command(description = "Find arbitrage opportunities")]
    Arbitrage,
    /// Positions - show open positions
    #[command(description = "Show open positions")]
    Positions,
    /// History - show trade history
    #[command(description = "Show trade history")]
    History,
    /// Alerts - manage price alerts
    #[command(description = "Manage price alerts")]
    Alerts,
    /// Settings - manage bot settings
    #[command(description = "Manage bot settings")]
    Settings,
    /// Stats - show trading statistics
    #[command(description = "Show trading statistics")]
    Stats,
    /// Ping - check bot status
    #[command(description = "Check bot status")]
    Ping,
    /// Admin - admin commands (admin only)
    #[command(description = "Admin commands")]
    Admin,
}

/// User state for conversation flow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserState {
    /// Default state
    Idle,
    /// Waiting for token address for snipe
    WaitingForSnipeToken,
    /// Waiting for amount for snipe
    WaitingForSnipeAmount {
        token_address: String,
    },
    /// Waiting for slippage for snipe
    WaitingForSnipeSlippage {
        token_address: String,
        amount: f64,
    },
    /// Waiting for token address for chart
    WaitingForChartToken,
    /// Waiting for timeframe for chart
    WaitingForChartTimeframe {
        token_address: String,
    },
    /// Waiting for alert condition
    WaitingForAlertCondition,
    /// Waiting for alert value
    WaitingForAlertValue {
        condition: String,
        token_address: String,
    },
}

impl Default for UserState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Telegram user session data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSession {
    /// User ID in our database
    pub user_id: Option<Uuid>,
    /// Telegram user ID
    pub telegram_id: i64,
    /// Current user state
    pub state: UserState,
    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
    /// User preferences
    pub preferences: UserPreferences,
    /// Temporary data for multi-step commands
    pub temp_data: serde_json::Value,
}

/// User preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Default chain for trading
    pub default_chain: String,
    /// Default slippage percentage
    pub default_slippage: f64,
    /// Receive trade notifications
    pub receive_trade_notifications: bool,
    /// Receive price alerts
    pub receive_price_alerts: bool,
    /// Notification frequency
    pub notification_frequency: NotificationFrequency,
    /// Preferred timezone
    pub timezone: String,
}

/// Notification frequency
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum NotificationFrequency {
    /// Real-time notifications
    Realtime,
    /// Daily summary
    Daily,
    /// Weekly summary
    Weekly,
    /// Only important alerts
    ImportantOnly,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            default_chain: "ethereum".to_string(),
            default_slippage: 1.0, // 1%
            receive_trade_notifications: true,
            receive_price_alerts: true,
            notification_frequency: NotificationFrequency::ImportantOnly,
            timezone: "UTC".to_string(),
        }
    }
}

impl Default for UserSession {
    fn default() -> Self {
        Self {
            user_id: None,
            telegram_id: 0,
            state: UserState::Idle,
            last_activity: Utc::now(),
            preferences: UserPreferences::default(),
            temp_data: serde_json::json!({}),
        }
    }
}

/// Telegram bot error types
#[derive(Error, Debug)]
pub enum TelegramError {
    /// User not authorized
    #[error("User {0} not authorized")]
    UnauthorizedUser(i64),

    /// Invalid command format
    #[error("Invalid command format: {0}")]
    InvalidCommand(String),

    /// Invalid token address
    #[error("Invalid token address: {0}")]
    InvalidTokenAddress(String),

    /// Invalid amount
    #[error("Invalid amount: {0}")]
    InvalidAmount(String),

    /// Invalid slippage
    #[error("Invalid slippage: {0}")]
    InvalidSlippage(String),

    /// Conversation timeout
    #[error("Conversation timeout")]
    ConversationTimeout,

    /// Message too long
    #[error("Message too long: {0} characters (max: {1})")]
    MessageTooLong(usize, usize),

    /// Telegram API error
    #[error("Telegram API error: {0}")]
    ApiError(String),

    /// User session not found
    #[error("User session not found")]
    SessionNotFound,

    /// Admin command required
    #[error("Admin command required")]
    AdminRequired,
}

/// Telegram bot service
pub struct TelegramBot {
    /// Telegram bot
    bot: Bot,
    /// User session storage
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
    /// Configuration
    config: TelegramConfig,
    /// Whether the bot is running
    is_running: Arc<RwLock<bool>>,
}

impl TelegramBot {
    /// Create a new Telegram bot
    pub fn new(config: TelegramConfig) -> Result<Self> {
        let bot = Bot::new(&config.token);

        Ok(Self {
            bot,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            config,
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Start the bot with polling
    pub async fn start_polling(&self) -> Result<()> {
        tracing::info!("Starting Telegram bot with polling...");

        let bot = self.bot.clone();
        let sessions = self.sessions.clone();
        let config = self.config.clone();

        // Update running state
        {
            let mut is_running = self.is_running.write().await;
            *is_running = true;
        }

        // Create command handler
        let handler = Update::filter_message()
            .filter_command::<Command>()
            .endpoint(handle_command);

        // Create message handler for non-command messages
        let message_handler = Update::filter_message()
            .filter(|msg: Message| msg.text().is_some())
            .endpoint(handle_message);

        // Combine handlers
        let handler = dptree::entry()
            .branch(handler)
            .branch(message_handler);

        // Start dispatcher
        Dispatcher::builder(bot, handler)
            .dependencies(dptree::deps![sessions, config])
            .enable_ctrlc_handler()
            .build()
            .dispatch()
            .await;

        Ok(())
    }

    /// Stop the bot
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping Telegram bot...");

        let mut is_running = self.is_running.write().await;
        *is_running = false;

        Ok(())
    }

    /// Check if a user is authorized
    pub fn is_user_authorized(&self, user_id: i64) -> bool {
        if self.config.allowed_users.is_empty() {
            true // Allow all if no restrictions
        } else {
            self.config.allowed_users.contains(&user_id)
                || self.config.admin_users.contains(&user_id)
        }
    }

    /// Check if a user is admin
    pub fn is_user_admin(&self, user_id: i64) -> bool {
        self.config.admin_users.contains(&user_id)
    }

    /// Get or create user session
    pub async fn get_or_create_session(&self, telegram_id: i64) -> UserSession {
        let mut sessions = self.sessions.write().await;

        if let Some(session) = sessions.get_mut(&telegram_id) {
            session.last_activity = Utc::now();
            return session.clone();
        }

        let session = UserSession {
            telegram_id,
            ..Default::default()
        };

        sessions.insert(telegram_id, session.clone());
        session
    }

    /// Update user session
    pub async fn update_session(&self, telegram_id: i64, session: UserSession) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.insert(telegram_id, session);
        Ok(())
    }

    /// Clear user session
    pub async fn clear_session(&self, telegram_id: i64) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(&telegram_id);
        Ok(())
    }

    /// Send message to user
    pub async fn send_message(
        &self,
        chat_id: ChatId,
        text: &str,
        parse_mode: Option<ParseMode>,
        reply_markup: Option<InlineKeyboardMarkup>,
    ) -> Result<Message> {
        // Check message length
        if text.len() > self.config.max_message_length {
            return Err(Error::Telegram(format!(
                "Message too long: {} characters (max: {})",
                text.len(),
                self.config.max_message_length
            )));
        }

        let mut message = self.bot.send_message(chat_id, text);

        if let Some(mode) = parse_mode {
            message = message.parse_mode(mode);
        }

        if let Some(markup) = reply_markup {
            message = message.reply_markup(markup);
        }

        message
            .await
            .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))
    }

    /// Send alert to alert channel
    pub async fn send_alert(&self, text: &str) -> Result<Message> {
        if let Some(channel_id) = self.config.alert_channel_id {
            self.send_message(ChatId(channel_id), text, Some(ParseMode::Html), None)
                .await
        } else {
            Err(Error::Telegram("Alert channel not configured".to_string()))
        }
    }

    /// Send log to log channel
    pub async fn send_log(&self, text: &str) -> Result<Message> {
        if let Some(channel_id) = self.config.log_channel_id {
            self.send_message(ChatId(channel_id), text, Some(ParseMode::Html), None)
                .await
        } else {
            // Log locally if channel not configured
            tracing::info!("Telegram log: {}", text);
            Err(Error::Telegram("Log channel not configured".to_string()))
        }
    }

    /// Format portfolio for display
    pub fn format_portfolio(portfolio: &PortfolioData) -> String {
        let mut output = String::new();
        output.push_str("<b>📊 Portfolio Summary</b>\n\n");

        output.push_str(&format!(
            "<b>Total Value:</b> ${:.2}\n",
            portfolio.total_value_usd
        ));
        output.push_str(&format!(
            "<b>24h Change:</b> {:.2}%\n",
            portfolio.change_24h_percent
        ));
        output.push_str(&format!(
            "<b>Total P&L:</b> ${:.2}\n\n",
            portfolio.total_pnl_usd
        ));

        output.push_str("<b>Positions:</b>\n");
        for position in &portfolio.positions {
            output.push_str(&format!(
                "• {}: ${:.2} ({:.2}%)\n",
                position.token_symbol, position.value_usd, position.allocation_percent
            ));
        }

        output
    }

    /// Format trade for display
    pub fn format_trade(trade: &TradeData) -> String {
        let status_emoji = match trade.status.as_str() {
            "executed" => "✅",
            "pending" => "⏳",
            "failed" => "❌",
            "cancelled" => "🚫",
            _ => "📝",
        };

        let mut output = String::new();
        output.push_str(&format!("{} <b>Trade Executed</b>\n\n", status_emoji));

        output.push_str(&format!("<b>Type:</b> {}\n", trade.trade_type));
        output.push_str(&format!("<b>Pair:</b> {}/{}\n", trade.token_in, trade.token_out));
        output.push_str(&format!("<b>Amount:</b> {:.4} {}\n", trade.amount_in, trade.token_in));
        output.push_str(&format!("<b>Price:</b> ${:.6}\n", trade.price_usd));
        output.push_str(&format!("<b>Slippage:</b> {:.2}%\n", trade.slippage_percent));

        if let Some(profit_loss) = trade.profit_loss_usd {
            let pl_emoji = if profit_loss >= 0.0 { "🟢" } else { "🔴" };
            output.push_str(&format!("<b>P&L:</b> {} ${:.2}\n", pl_emoji, profit_loss));
        }

        if let Some(tx_hash) = &trade.transaction_hash {
            output.push_str(&format!("\n<b>TX Hash:</b> <code>{}</code>", tx_hash));
        }

        output
    }

    /// Format price alert
    pub fn format_price_alert(alert: &PriceAlertData) -> String {
        let mut output = String::new();
        output.push_str("🚨 <b>Price Alert Triggered!</b>\n\n");

        output.push_str(&format!("<b>Token:</b> {}\n", alert.token_symbol));
        output.push_str(&format!("<b>Current Price:</b> ${:.6}\n", alert.current_price));
        output.push_str(&format!("<b>Target Price:</b> ${:.6}\n", alert.target_price));
        output.push_str(&format!("<b>Condition:</b> {}\n", alert.condition));

        if let Some(change_percent) = alert.change_percent {
            output.push_str(&format!("<b>Change:</b> {:.2}%\n", change_percent));
        }

        output
    }

    /// Create inline keyboard for commands
    pub fn create_main_keyboard() -> InlineKeyboardMarkup {
        let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();

        // Row 1: Trading commands
        keyboard.push(vec![
            InlineKeyboardButton::callback("📊 Portfolio", "portfolio"),
            InlineKeyboardButton::callback("🎯 Snip", "snip"),
        ]);

        // Row 2: More trading commands
        keyboard.push(vec![
            InlineKeyboardButton::callback("📈 Chart", "chart"),
            InlineKeyboardButton::callback("🔄 Arbitrage", "arbitrage"),
        ]);

        // Row 3: Management commands
        keyboard.push(vec![
            InlineKeyboardButton::callback("🔔 Alerts", "alerts"),
            InlineKeyboardButton::callback("⚙️ Settings", "settings"),
        ]);

        InlineKeyboardMarkup::new(keyboard)
    }

    /// Create inline keyboard for token selection
    pub fn create_token_keyboard(tokens: &[TokenData]) -> InlineKeyboardMarkup {
        let mut keyboard: Vec<Vec<InlineKeyboardButton>> = Vec::new();

        // Add tokens in rows of 2
        for chunk in tokens.chunks(2) {
            let row: Vec<InlineKeyboardButton> = chunk
                .iter()
                .map(|token| {
                    InlineKeyboardButton::callback(
                        format!("{} ({})", token.symbol, token.price_usd),
                        format!("token_{}", token.address),
                    )
                })
                .collect();
            keyboard.push(row);
        }

        // Add cancel button
        keyboard.push(vec![InlineKeyboardButton::callback("❌ Cancel", "cancel")]);

        InlineKeyboardMarkup::new(keyboard)
    }
}

/// Handler for commands
async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
    config: TelegramConfig,
) -> Result<()> {
    let chat_id = msg.chat.id;
    let user_id = msg.from.unwrap().id.0;

    // Check if user is authorized
    if !config.allowed_users.is_empty() && !config.allowed_users.contains(&user_id) {
        bot.send_message(chat_id, "🚫 You are not authorized to use this bot.")
            .await
            .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;
        return Ok(());
    }

    // Get or create session
    let session = {
        let mut sessions = sessions.write().await;
        let session = sessions
            .entry(user_id)
            .or_insert_with(|| UserSession {
                telegram_id: user_id,
                ..Default::default()
            });
        session.last_activity = Utc::now();
        session.clone()
    };

    // Handle command
    match cmd {
        Command::Start => handle_start(bot, msg, session, sessions).await,
        Command::Help => handle_help(bot, msg).await,
        Command::Portfolio => handle_portfolio(bot, msg, session).await,
        Command::Snip => handle_snipe(bot, msg, session, sessions).await,
        Command::Limit => handle_limit(bot, msg, session, sessions).await,
        Command::Chart => handle_chart(bot, msg, session, sessions).await,
        Command::Arbitrage => handle_arbitrage(bot, msg, session).await,
        Command::Positions => handle_positions(bot, msg, session).await,
        Command::History => handle_history(bot, msg, session).await,
        Command::Alerts => handle_alerts(bot, msg, session, sessions).await,
        Command::Settings => handle_settings(bot, msg, session, sessions).await,
        Command::Stats => handle_stats(bot, msg, session).await,
        Command::Ping => handle_ping(bot, msg).await,
        Command::Admin => handle_admin(bot, msg, session, config).await,
    }
}

/// Handler for non-command messages
async fn handle_message(
    bot: Bot,
    msg: Message,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
    config: TelegramConfig,
) -> Result<()> {
    let chat_id = msg.chat.id;
    let user_id = msg.from.unwrap().id.0;
    let text = msg.text().unwrap_or("");

    // Check if user is authorized
    if !config.allowed_users.is_empty() && !config.allowed_users.contains(&user_id) {
        return Ok(());
    }

    // Get session
    let mut sessions_lock = sessions.write().await;
    let session = sessions_lock
        .get_mut(&user_id)
        .ok_or_else(|| Error::Telegram("Session not found".to_string()))?;

    session.last_activity = Utc::now();

    // Handle based on current state
    match &session.state {
        UserState::WaitingForSnipeToken => {
            handle_snipe_token_input(bot, msg, session, text).await
        }
        UserState::WaitingForSnipeAmount { token_address } => {
            handle_snipe_amount_input(bot, msg, session, token_address, text).await
        }
        UserState::WaitingForSnipeSlippage {
            token_address,
            amount,
        } => handle_snipe_slippage_input(bot, msg, session, token_address, *amount, text).await,
        UserState::WaitingForChartToken => {
            handle_chart_token_input(bot, msg, session, text).await
        }
        UserState::WaitingForChartTimeframe { token_address } => {
            handle_chart_timeframe_input(bot, msg, session, token_address, text).await
        }
        UserState::WaitingForAlertCondition => {
            handle_alert_condition_input(bot, msg, session, text).await
        }
        UserState::WaitingForAlertValue {
            condition,
            token_address,
        } => handle_alert_value_input(bot, msg, session, condition, token_address, text).await,
        UserState::Idle => {
            // Ignore non-command messages in idle state
            Ok(())
        }
    }
}

/// Start command handler
async fn handle_start(
    bot: Bot,
    msg: Message,
    session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    let welcome_text = r#"
🤖 <b>Velocidad Trading Bot</b>

Welcome to the high-performance multi-chain trading bot!

<b>Features:</b>
• Token sniping with MEV protection
• Cross-chain arbitrage detection
• Real-time price alerts
• Portfolio tracking
• Advanced charting

<b>Quick Start:</b>
1. Connect your wallet
2. Set up trading strategies
3. Start trading!

Use /help to see all commands.
"#;

    let keyboard = TelegramBot::create_main_keyboard();

    bot.send_message(msg.chat.id, welcome_text)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Help command handler
async fn handle_help(bot: Bot, msg: Message) -> Result<()> {
    let help_text = Command::descriptions().to_string();

    bot.send_message(msg.chat.id, help_text)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Portfolio command handler
async fn handle_portfolio(bot: Bot, msg: Message, session: UserSession) -> Result<()> {
    // TODO: Fetch actual portfolio data
    let portfolio_data = PortfolioData {
        total_value_usd: 15000.0,
        change_24h_percent: 5.2,
        total_pnl_usd: 1200.0,
        positions: vec![
            PositionData {
                token_symbol: "ETH".to_string(),
                value_usd: 8000.0,
                allocation_percent: 53.3,
            },
            PositionData {
                token_symbol: "SOL".to_string(),
                value_usd: 5000.0,
                allocation_percent: 33.3,
            },
            PositionData {
                token_symbol: "USDC".to_string(),
                value_usd: 2000.0,
                allocation_percent: 13.3,
            },
        ],
    };

    let formatted = TelegramBot::format_portfolio(&portfolio_data);

    bot.send_message(msg.chat.id, formatted)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Snipe command handler
async fn handle_snipe(
    bot: Bot,
    msg: Message,
    mut session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    session.state = UserState::WaitingForSnipeToken;

    // Update session
    {
        let mut sessions_lock = sessions.write().await;
        sessions_lock.insert(session.telegram_id, session.clone());
    }

    let text = "🎯 <b>Token Sniping</b>\n\nPlease enter the token address:";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Limit order command handler
async fn handle_limit(
    bot: Bot,
    msg: Message,
    session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    // Similar to snipe but for limit orders
    let text = "⚠️ Limit orders are coming soon! Use /snip for market orders.";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Chart command handler
async fn handle_chart(
    bot: Bot,
    msg: Message,
    mut session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    session.state = UserState::WaitingForChartToken;

    // Update session
    {
        let mut sessions_lock = sessions.write().await;
        sessions_lock.insert(session.telegram_id, session.clone());
    }

    let text = "📈 <b>Price Chart</b>\n\nPlease enter the token address or symbol:";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Arbitrage command handler
async fn handle_arbitrage(bot: Bot, msg: Message, session: UserSession) -> Result<()> {
    // TODO: Fetch arbitrage opportunities
    let text = "🔄 <b>Arbitrage Opportunities</b>\n\nScanning for opportunities...\n\n(Feature coming soon!)";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Positions command handler
async fn handle_positions(bot: Bot, msg: Message, session: UserSession) -> Result<()> {
    // TODO: Fetch open positions
    let text = "📊 <b>Open Positions</b>\n\nNo open positions found.";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// History command handler
async fn handle_history(bot: Bot, msg: Message, session: UserSession) -> Result<()> {
    // TODO: Fetch trade history
    let text = "📜 <b>Trade History</b>\n\nNo trades yet.";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Alerts command handler
async fn handle_alerts(
    bot: Bot,
    msg: Message,
    mut session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    session.state = UserState::WaitingForAlertCondition;

    // Update session
    {
        let mut sessions_lock = sessions.write().await;
        sessions_lock.insert(session.telegram_id, session.clone());
    }

    let text = "🔔 <b>Price Alerts</b>\n\nEnter alert condition:\n• price > [value]\n• price < [value]\n• volume > [value]\n• change > [percent]%";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Settings command handler
async fn handle_settings(
    bot: Bot,
    msg: Message,
    session: UserSession,
    sessions: Arc<RwLock<HashMap<i64, UserSession>>>,
) -> Result<()> {
    let text = "⚙️ <b>Settings</b>\n\nAvailable settings:\n• /settings chain - Set default chain\n• /settings slippage - Set default slippage\n• /settings notifications - Configure notifications";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Stats command handler
async fn handle_stats(bot: Bot, msg: Message, session: UserSession) -> Result<()> {
    // TODO: Fetch trading statistics
    let text = "📊 <b>Trading Statistics</b>\n\nTotal Trades: 0\nWin Rate: 0%\nTotal P&L: $0.00\nBest Trade: N/A\nWorst Trade: N/A";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Ping command handler
async fn handle_ping(bot: Bot, msg: Message) -> Result<()> {
    let start_time = Utc::now();
    let ping_msg = bot.send_message(msg.chat.id, "🏓 Pong!").await?;
    let end_time = Utc::now();
    let latency = (end_time - start_time).num_milliseconds();

    let text = format!("🏓 <b>Pong!</b>\n\nLatency: {}ms\nBot Status: ✅ Online", latency);

    bot.edit_message_text(msg.chat.id, ping_msg.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to edit message: {}", e)))?;

    Ok(())
}

/// Admin command handler
async fn handle_admin(
    bot: Bot,
    msg: Message,
    session: UserSession,
    config: TelegramConfig,
) -> Result<()> {
    let user_id = msg.from.unwrap().id.0;

    if !config.admin_users.contains(&user_id) {
        bot.send_message(msg.chat.id, "🚫 Admin access required.")
            .await
            .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;
        return Ok(());
    }

    let text = "👑 <b>Admin Panel</b>\n\nAvailable commands:\n• /admin users - List all users\n• /admin stats - System statistics\n• /admin restart - Restart bot\n• /admin broadcast - Send broadcast message";

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for snipe token input
async fn handle_snipe_token_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    token_address: &str,
) -> Result<()> {
    // Validate token address (basic validation)
    if token_address.is_empty() || token_address.len() < 20 {
        bot.send_message(msg.chat.id, "❌ Invalid token address. Please try again.")
            .await
            .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;
        return Ok(());
    }

    session.state = UserState::WaitingForSnipeAmount {
        token_address: token_address.to_string(),
    };

    let text = format!(
        "🎯 <b>Token Sniping</b>\n\nToken: <code>{}</code>\n\nEnter amount to snipe (in ETH):",
        token_address
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for snipe amount input
async fn handle_snipe_amount_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    token_address: &str,
    amount_str: &str,
) -> Result<()> {
    let amount: f64 = match amount_str.parse() {
        Ok(amount) if amount > 0.0 => amount,
        _ => {
            bot.send_message(msg.chat.id, "❌ Invalid amount. Please enter a positive number.")
                .await
                .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;
            return Ok(());
        }
    };

    session.state = UserState::WaitingForSnipeSlippage {
        token_address: token_address.to_string(),
        amount,
    };

    let text = format!(
        "🎯 <b>Token Sniping</b>\n\nToken: <code>{}</code>\nAmount: {} ETH\n\nEnter slippage tolerance (default: 1.0%):",
        token_address, amount
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for snipe slippage input
async fn handle_snipe_slippage_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    token_address: &str,
    amount: f64,
    slippage_str: &str,
) -> Result<()> {
    let slippage: f64 = if slippage_str.trim().is_empty() {
        1.0 // Default slippage
    } else {
        match slippage_str.parse() {
            Ok(slippage) if slippage > 0.0 && slippage <= 50.0 => slippage,
            _ => {
                bot.send_message(msg.chat.id, "❌ Invalid slippage. Please enter a number between 0.1 and 50.")
                    .await
                    .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;
                return Ok(());
            }
        }
    };

    // Reset session state
    session.state = UserState::Idle;

    // TODO: Execute actual trade
    let text = format!(
        "✅ <b>Trade Submitted</b>\n\nToken: <code>{}</code>\nAmount: {} ETH\nSlippage: {}%\n\nTrade is being executed...",
        token_address, amount, slippage
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for chart token input
async fn handle_chart_token_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    token_input: &str,
) -> Result<()> {
    session.state = UserState::WaitingForChartTimeframe {
        token_address: token_input.to_string(),
    };

    let text = format!(
        "📈 <b>Price Chart</b>\n\nToken: {}\n\nSelect timeframe:\n• 1h\n• 4h\n• 1d\n• 1w\n• 1m",
        token_input
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for chart timeframe input
async fn handle_chart_timeframe_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    token_address: &str,
    timeframe: &str,
) -> Result<()> {
    // Reset session state
    session.state = UserState::Idle;

    let text = format!(
        "📈 <b>Price Chart</b>\n\nToken: {}\nTimeframe: {}\n\nChart generation coming soon!",
        token_address, timeframe
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for alert condition input
async fn handle_alert_condition_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    condition: &str,
) -> Result<()> {
    session.state = UserState::WaitingForAlertValue {
        condition: condition.to_string(),
        token_address: String::new(), // Will be set in next step
    };

    let text = format!(
        "🔔 <b>Price Alert</b>\n\nCondition: {}\n\nEnter token address:",
        condition
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Handler for alert value input
async fn handle_alert_value_input(
    bot: Bot,
    msg: Message,
    session: &mut UserSession,
    condition: &str,
    token_address: &str,
    value_str: &str,
) -> Result<()> {
    // Reset session state
    session.state = UserState::Idle;

    let text = format!(
        "✅ <b>Alert Created</b>\n\nToken: {}\nCondition: {}\nValue: {}\n\nYou will be notified when the condition is met.",
        token_address, condition, value_str
    );

    bot.send_message(msg.chat.id, text)
        .parse_mode(ParseMode::Html)
        .await
        .map_err(|e| Error::Telegram(format!("Failed to send message: {}", e)))?;

    Ok(())
}

/// Data structures for responses

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioData {
    pub total_value_usd: f64,
    pub change_24h_percent: f64,
    pub total_pnl_usd: f64,
    pub positions: Vec<PositionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionData {
    pub token_symbol: String,
    pub value_usd: f64,
    pub allocation_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeData {
    pub trade_type: String,
    pub token_in: String,
    pub token_out: String,
    pub amount_in: f64,
    pub price_usd: f64,
    pub slippage_percent: f64,
    pub status: String,
    pub profit_loss_usd: Option<f64>,
    pub transaction_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceAlertData {
    pub token_symbol: String,
    pub current_price: f64,
    pub target_price: f64,
    pub condition: String,
    pub change_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub address: String,
    pub symbol: String,
    pub price_usd: f64,
    pub change_24h: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_parsing() {
        assert_eq!(Command::parse("/start", "bot_name").unwrap(), Command::Start);
        assert_eq!(Command::parse("/help", "bot_name").unwrap(), Command::Help);
        assert_eq!(
            Command::parse("/portfolio", "bot_name").unwrap(),
            Command::Portfolio
        );
    }

    #[test]
    fn test_user_session_default() {
        let session = UserSession::default();
        assert!(session.user_id.is_none());
        assert_eq!(session.telegram_id, 0);
        assert!(matches!(session.state, UserState::Idle));
    }

    #[test]
    fn test_telegram_config_default() {
        let config = TelegramConfig::default();
        assert!(config.token.is_empty());
        assert!(config.webhook_url.is_none());
        assert!(config.allowed_users.is_empty());
        assert_eq!(config.max_message_length, 4000);
    }

    #[test]
    fn test_format_portfolio() {
        let portfolio = PortfolioData {
            total_value_usd: 10000.0,
            change_24h_percent: 5.0,
            total_pnl_usd: 500.0,
            positions: vec![PositionData {
                token_symbol: "ETH".to_string(),
                value_usd: 6000.0,
                allocation_percent: 60.0,
            }],
        };

        let formatted = TelegramBot::format_portfolio(&portfolio);
        assert!(formatted.contains("Portfolio Summary"));
        assert!(formatted.contains("$10000.00"));
        assert!(formatted.contains("5.00%"));
    }

    #[test]
    fn test_format_trade() {
        let trade = TradeData {
            trade_type: "BUY".to_string(),
            token_in: "ETH".to_string(),
            token_out: "USDC".to_string(),
            amount_in: 1.0,
            price_usd: 3000.0,
            slippage_percent: 0.5,
            status: "executed".to_string(),
            profit_loss_usd: Some(50.0),
            transaction_hash: Some("0x1234".to_string()),
        };

        let formatted = TelegramBot::format_trade(&trade);
        assert!(formatted.contains("Trade Executed"));
        assert!(formatted.contains("ETH/USDC"));
        assert!(formatted.contains("$3000.00"));
    }
}
