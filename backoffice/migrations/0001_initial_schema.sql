-- Initial database schema for Velocidad Trading Bot
-- Migration: 0001_initial_schema.sql
-- Created: $(date)
-- Description: Creates core tables for users, trades, strategies, market data, alerts, and portfolio positions

-- Enable required extensions
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";
CREATE EXTENSION IF NOT EXISTS "pg_trgm"; -- For text search
COMMENT ON EXTENSION "pg_trgm" IS 'Trigram indexing for text search';

-- ============================================================================
-- Core Tables
-- ============================================================================

-- Users and authentication
CREATE TABLE users (
    -- Primary identifier
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    -- External identifiers
    telegram_id BIGINT UNIQUE,
    wallet_address VARCHAR(255) UNIQUE,

    -- Security
    encrypted_private_key BYTEA, -- Encrypted with user password
    encryption_salt BYTEA, -- Salt for key derivation

    -- User settings and preferences
    settings JSONB DEFAULT '{}'::jsonb,

    -- User profile
    username VARCHAR(100),
    email VARCHAR(255) UNIQUE,
    email_verified BOOLEAN DEFAULT false,
    phone_number VARCHAR(50),

    -- Security settings
    two_factor_enabled BOOLEAN DEFAULT false,
    two_factor_secret VARCHAR(255),
    failed_login_attempts INTEGER DEFAULT 0,
    lockout_until TIMESTAMPTZ,

    -- Status
    is_active BOOLEAN DEFAULT true,
    is_verified BOOLEAN DEFAULT false,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    deleted_at TIMESTAMPTZ,
    last_login_at TIMESTAMPTZ,

    -- Constraints
    CHECK (telegram_id IS NOT NULL OR wallet_address IS NOT NULL OR email IS NOT NULL)
);

COMMENT ON TABLE users IS 'User accounts and authentication information';
COMMENT ON COLUMN users.encrypted_private_key IS 'Encrypted private key (AES-GCM)';
COMMENT ON COLUMN users.encryption_salt IS 'Salt for PBKDF2 key derivation';
COMMENT ON COLUMN users.settings IS 'User preferences and settings in JSON format';

-- Trading strategies
CREATE TABLE strategies (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- Strategy details
    name VARCHAR(255) NOT NULL,
    description TEXT,
    strategy_type VARCHAR(50) NOT NULL,

    -- Strategy configuration
    config JSONB NOT NULL DEFAULT '{}'::jsonb,

    -- Performance metrics
    performance_metrics JSONB DEFAULT '{}'::jsonb,
    total_pnl DECIMAL(36, 18) DEFAULT 0,
    total_trades INTEGER DEFAULT 0,
    win_rate DECIMAL(5, 4) DEFAULT 0,

    -- Status and settings
    is_active BOOLEAN DEFAULT true,
    is_public BOOLEAN DEFAULT false,

    -- Risk parameters
    max_position_size DECIMAL(36, 18),
    max_daily_loss DECIMAL(36, 18),
    allowed_chains TEXT[] DEFAULT '{}',
    allowed_tokens TEXT[] DEFAULT '{}',

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    last_executed_at TIMESTAMPTZ,

    -- Constraints
    CHECK (strategy_type IN ('sniping', 'arbitrage', 'hft', 'market_making', 'mean_reversion', 'momentum'))
);

COMMENT ON TABLE strategies IS 'Trading strategies configuration and performance';
COMMENT ON COLUMN strategies.config IS 'Strategy-specific configuration parameters';
COMMENT ON COLUMN strategies.performance_metrics IS 'Detailed performance metrics in JSON format';

-- Trade executions
CREATE TABLE trades (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    strategy_id UUID REFERENCES strategies(id) ON DELETE SET NULL,

    -- Blockchain information
    chain_id INTEGER NOT NULL,
    chain_name VARCHAR(50) NOT NULL,

    -- Token information
    token_in VARCHAR(255) NOT NULL,
    token_out VARCHAR(255) NOT NULL,
    token_in_symbol VARCHAR(50),
    token_out_symbol VARCHAR(50),

    -- Amounts
    amount_in DECIMAL(36, 18) NOT NULL,
    amount_out DECIMAL(36, 18) NOT NULL,
    amount_in_usd DECIMAL(36, 18),
    amount_out_usd DECIMAL(36, 18),

    -- Price and slippage
    price DECIMAL(36, 18),
    slippage_percent DECIMAL(10, 4),

    -- Transaction details
    tx_hash VARCHAR(255) UNIQUE,
    tx_from VARCHAR(255),
    tx_to VARCHAR(255),
    gas_used BIGINT,
    gas_price DECIMAL(36, 18),
    gas_price_gwei DECIMAL(36, 18),
    gas_cost_usd DECIMAL(36, 18),

    -- DEX information
    dex VARCHAR(50),
    dex_version VARCHAR(20),
    router_address VARCHAR(255),

    -- Status
    status VARCHAR(50) NOT NULL,
    error_message TEXT,

    -- Profit/Loss
    profit_loss DECIMAL(36, 18),
    profit_loss_usd DECIMAL(36, 18),
    profit_loss_percent DECIMAL(10, 4),

    -- Metadata
    metadata JSONB DEFAULT '{}'::jsonb,
    notes TEXT,

    -- Timestamps
    execution_time TIMESTAMPTZ,
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- Constraints
    CHECK (status IN ('pending', 'executing', 'executed', 'failed', 'reverted', 'cancelled')),
    CHECK (amount_in > 0),
    CHECK (amount_out > 0)
);

COMMENT ON TABLE trades IS 'Trade execution records';
COMMENT ON COLUMN trades.chain_id IS 'EIP-155 chain ID';
COMMENT ON COLUMN trades.metadata IS 'Additional trade metadata in JSON format';

-- Market data (time-series)
CREATE TABLE market_data (
    -- Composite primary key
    token_address VARCHAR(255) NOT NULL,
    dex VARCHAR(50) NOT NULL,
    chain_id INTEGER NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,

    -- Price data
    price DECIMAL(36, 18) NOT NULL,
    price_usd DECIMAL(36, 18),
    price_change_24h DECIMAL(10, 4),
    price_change_7d DECIMAL(10, 4),

    -- Liquidity data
    liquidity DECIMAL(36, 18) NOT NULL,
    liquidity_usd DECIMAL(36, 18),
    reserve0 DECIMAL(36, 18),
    reserve1 DECIMAL(36, 18),

    -- Volume data
    volume_24h DECIMAL(36, 18),
    volume_24h_usd DECIMAL(36, 18),
    trades_24h INTEGER,

    -- Pool information
    pool_address VARCHAR(255),
    fee_tier DECIMAL(5, 4),

    -- Derived metrics
    market_cap_usd DECIMAL(36, 18),
    fdv_usd DECIMAL(36, 18),

    -- Data quality
    confidence DECIMAL(5, 4) DEFAULT 1.0,
    source VARCHAR(50) NOT NULL,

    -- Primary key and indexes
    PRIMARY KEY (token_address, dex, chain_id, timestamp)
);

COMMENT ON TABLE market_data IS 'Time-series market data for tokens across DEXes';
COMMENT ON COLUMN market_data.confidence IS 'Confidence score for price data (0-1)';
COMMENT ON COLUMN market_data.source IS 'Data source (dex_pool, aggregator, oracle, cex)';

-- Create hypertable for time-series data (for TimescaleDB)
-- Note: Uncomment if using TimescaleDB
-- SELECT create_hypertable('market_data', 'timestamp');

-- Alerts system
CREATE TABLE alerts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- Alert configuration
    alert_type VARCHAR(50) NOT NULL,
    name VARCHAR(255) NOT NULL,
    description TEXT,

    -- Target token
    token_address VARCHAR(255) NOT NULL,
    token_symbol VARCHAR(50),
    chain_id INTEGER,

    -- Condition
    condition_type VARCHAR(50) NOT NULL,
    condition_value DECIMAL(36, 18) NOT NULL,
    condition_operator VARCHAR(10) NOT NULL,

    -- Notification settings
    notification_channels TEXT[] DEFAULT '{"telegram"}',
    is_recurring BOOLEAN DEFAULT false,
    recurrence_interval INTEGER, -- in minutes

    -- Status
    is_active BOOLEAN DEFAULT true,
    last_triggered_at TIMESTAMPTZ,
    trigger_count INTEGER DEFAULT 0,

    -- Metadata
    metadata JSONB DEFAULT '{}'::jsonb,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- Constraints
    CHECK (alert_type IN ('price', 'volume', 'liquidity', 'new_listing', 'whale_movement')),
    CHECK (condition_operator IN ('>', '>=', '<', '<=', '=', '!='))
);

COMMENT ON TABLE alerts IS 'Price and market condition alerts';
COMMENT ON COLUMN alerts.notification_channels IS 'Array of notification channels: telegram, email, webhook';

-- Alert triggers history
CREATE TABLE alert_triggers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    alert_id UUID NOT NULL REFERENCES alerts(id) ON DELETE CASCADE,

    -- Trigger data
    triggered_value DECIMAL(36, 18) NOT NULL,
    threshold_value DECIMAL(36, 18) NOT NULL,

    -- Market context
    token_price DECIMAL(36, 18),
    token_price_usd DECIMAL(36, 18),
    market_conditions JSONB DEFAULT '{}'::jsonb,

    -- Notification status
    notified_channels TEXT[] DEFAULT '{}',
    notification_status VARCHAR(50) DEFAULT 'pending',
    notification_error TEXT,

    -- Timestamps
    triggered_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- Indexes will be created separately
    CHECK (notification_status IN ('pending', 'sent', 'failed', 'skipped'))
);

COMMENT ON TABLE alert_triggers IS 'History of alert triggers and notifications';

-- Portfolio positions
CREATE TABLE portfolio_positions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- Token information
    chain_id INTEGER NOT NULL,
    token_address VARCHAR(255) NOT NULL,
    token_symbol VARCHAR(50),
    token_name VARCHAR(255),

    -- Position data
    amount DECIMAL(36, 18) NOT NULL,
    amount_usd DECIMAL(36, 18),

    -- Price data
    avg_entry_price DECIMAL(36, 18) NOT NULL,
    avg_entry_price_usd DECIMAL(36, 18),
    current_price DECIMAL(36, 18),
    current_price_usd DECIMAL(36, 18),

    -- P&L
    unrealized_pnl DECIMAL(36, 18),
    unrealized_pnl_usd DECIMAL(36, 18),
    unrealized_pnl_percent DECIMAL(10, 4),
    realized_pnl DECIMAL(36, 18) DEFAULT 0,
    realized_pnl_usd DECIMAL(36, 18) DEFAULT 0,

    -- Position tracking
    position_type VARCHAR(20) DEFAULT 'spot',
    is_open BOOLEAN DEFAULT true,

    -- Metadata
    metadata JSONB DEFAULT '{}'::jsonb,

    -- Timestamps
    opened_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    closed_at TIMESTAMPTZ,

    -- Constraints
    CHECK (amount > 0),
    CHECK (avg_entry_price > 0),
    CHECK (position_type IN ('spot', 'margin', 'perpetual'))
);

COMMENT ON TABLE portfolio_positions IS 'Current and historical portfolio positions';
COMMENT ON COLUMN portfolio_positions.position_type IS 'Type of position: spot, margin, perpetual';

-- ============================================================================
-- Support Tables
-- ============================================================================

-- API keys for external integrations
CREATE TABLE api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,

    -- Key information
    name VARCHAR(255) NOT NULL,
    key_hash VARCHAR(255) NOT NULL UNIQUE,
    key_prefix VARCHAR(10) NOT NULL,

    -- Permissions
    permissions TEXT[] DEFAULT '{}',
    allowed_ips CIDR[] DEFAULT '{}',
    rate_limit_per_minute INTEGER DEFAULT 60,

    -- Status
    is_active BOOLEAN DEFAULT true,
    last_used_at TIMESTAMPTZ,
    usage_count INTEGER DEFAULT 0,

    -- Expiration
    expires_at TIMESTAMPTZ,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP
);

COMMENT ON TABLE api_keys IS 'API keys for external integrations and services';
COMMENT ON COLUMN api_keys.key_hash IS 'Hashed API key (SHA256)';
COMMENT ON COLUMN api_keys.key_prefix IS 'First 10 chars of original key for identification';

-- Rate limiting
CREATE TABLE rate_limits (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    api_key_id UUID REFERENCES api_keys(id) ON DELETE CASCADE,

    -- Rate limit data
    endpoint VARCHAR(255) NOT NULL,
    ip_address INET,
    request_count INTEGER NOT NULL DEFAULT 1,

    -- Window tracking
    window_start TIMESTAMPTZ NOT NULL,
    window_end TIMESTAMPTZ NOT NULL,

    -- Timestamps
    created_at TIMESTAMPTZ DEFAULT CURRENT_TIMESTAMP,

    -- Check constraint
    CHECK (user_id IS NOT NULL OR api_key_id IS NOT NULL OR ip_address IS NOT NULL)
);

COMMENT ON TABLE rate_limits IS 'Rate limiting tracking for API endpoints';

-- ============================================================================
-- Indexes
-- ============================================================================

-- Users indexes
CREATE INDEX idx_users_telegram_id ON users(telegram_id) WHERE telegram_id IS NOT NULL;
CREATE INDEX idx_users_wallet_address ON users(wallet_address) WHERE wallet_address IS NOT NULL;
CREATE INDEX idx_users_email ON users(email) WHERE email IS NOT NULL;
CREATE INDEX idx_users_created_at ON users(created_at);
CREATE INDEX idx_users_is_active ON users(is_active) WHERE is_active = true;

-- Strategies indexes
CREATE INDEX idx_strategies_user_id ON strategies(user_id);
CREATE INDEX idx_strategies_is_active ON strategies(is_active) WHERE is_active = true;
CREATE INDEX idx_strategies_strategy_type ON strategies(strategy_type);
CREATE INDEX idx_strategies_created_at ON strategies(created_at);

-- Trades indexes
CREATE INDEX idx_trades_user_id ON trades(user_id);
CREATE INDEX idx_trades_strategy_id ON trades(strategy_id) WHERE strategy_id IS NOT NULL;
CREATE INDEX idx_trades_chain_id ON trades(chain_id);
CREATE INDEX idx_trades_status ON trades(status);
CREATE INDEX idx_trades_execution_time ON trades(execution_time);
CREATE INDEX idx_trades_created_at ON trades(created_at);
CREATE INDEX idx_trades_tx_hash ON trades(tx_hash) WHERE tx_hash IS NOT NULL;
CREATE INDEX idx_trades_token_in ON trades(token_in);
CREATE INDEX idx_trades_token_out ON trades(token_out);
CREATE INDEX idx_trades_profit_loss ON trades(profit_loss) WHERE profit_loss IS NOT NULL;

-- Market data indexes
CREATE INDEX idx_market_data_token_address ON market_data(token_address);
CREATE INDEX idx_market_data_dex ON market_data(dex);
CREATE INDEX idx_market_data_chain_id ON market_data(chain_id);
CREATE INDEX idx_market_data_timestamp ON market_data(timestamp);
CREATE INDEX idx_market_data_price ON market_data(price);
CREATE INDEX idx_market_data_liquidity ON market_data(liquidity);
CREATE INDEX idx_market_data_token_dex_chain ON market_data(token_address, dex, chain_id);

-- Alerts indexes
CREATE INDEX idx_alerts_user_id ON alerts(user_id);
CREATE INDEX idx_alerts_is_active ON alerts(is_active) WHERE is_active = true;
CREATE INDEX idx_alerts_alert_type ON alerts(alert_type);
CREATE INDEX idx_alerts_token_address ON alerts(token_address);
CREATE INDEX idx_alerts_created_at ON alerts(created_at);
CREATE INDEX idx_alerts_last_triggered ON alerts(last_triggered_at) WHERE last_triggered_at IS NOT NULL;

-- Alert triggers indexes
CREATE INDEX idx_alert_triggers_alert_id ON alert_triggers(alert_id);
CREATE INDEX idx_alert_triggers_triggered_at ON alert_triggers(triggered_at);
CREATE INDEX idx_alert_triggers_notification_status ON alert_triggers(notification_status);

-- Portfolio positions indexes
CREATE INDEX idx_portfolio_positions_user_id ON portfolio_positions(user_id);
CREATE INDEX idx_portfolio_positions_is_open ON portfolio_positions(is_open) WHERE is_open = true;
CREATE INDEX idx_portfolio_positions_chain_id ON portfolio_positions(chain_id);
CREATE INDEX idx_portfolio_positions_token_address ON portfolio_positions(token_address);
CREATE INDEX idx_portfolio_positions_updated_at ON portfolio_positions(updated_at);

-- API keys indexes
CREATE INDEX idx_api_keys_user_id ON api_keys(user_id);
CREATE INDEX idx_api_keys_key_hash ON api_keys(key_hash);
CREATE INDEX idx_api_keys_is_active ON api_keys(is_active) WHERE is_active = true;
CREATE INDEX idx_api_keys_expires_at ON api_keys(expires_at) WHERE expires_at IS NOT NULL;

-- Rate limits indexes
CREATE INDEX idx_rate_limits_user_id ON rate_limits(user_id) WHERE user_id IS NOT NULL;
CREATE INDEX idx_rate_limits_api_key_id ON rate_limits(api_key_id) WHERE api_key_id IS NOT NULL;
CREATE INDEX idx_rate_limits_ip_address ON rate_limits(ip_address) WHERE ip_address IS NOT NULL;
CREATE INDEX idx_rate_limits_endpoint ON rate_limits(endpoint);
CREATE INDEX idx_rate_limits_window_end ON rate_limits(window_end);

-- ============================================================================
-- Functions and Triggers
-- ============================================================================

-- Function to update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = CURRENT_TIMESTAMP;
    RETURN NEW;
END;
$$ language 'plpgsql';

-- Apply updated_at triggers to all tables with updated_at column
CREATE TRIGGER update_users_updated_at BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_strategies_updated_at BEFORE UPDATE ON strategies
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_trades_updated_at BEFORE UPDATE ON trades
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_alerts_updated_at BEFORE UPDATE ON alerts
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_portfolio_positions_updated_at BEFORE UPDATE ON portfolio_positions
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_api_keys_updated_at BEFORE UPDATE ON api_keys
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Function to calculate age of data
CREATE OR REPLACE FUNCTION data_age(timestamp_column TIMESTAMPTZ)
RETURNS INTERVAL AS $$
BEGIN
    RETURN CURRENT_TIMESTAMP - timestamp_column;
END;
$$ LANGUAGE plpgsql IMMUTABLE;

-- ============================================================================
-- Views
-- ============================================================================

-- View for active user positions
CREATE VIEW active_positions AS
SELECT
    pp.*,
    u.telegram_id,
    u.wallet_address
FROM portfolio_positions pp
JOIN users u ON pp.user_id = u.id
WHERE pp.is_open = true
ORDER BY pp.updated_at DESC;

-- View for daily trading volume
CREATE VIEW daily_trading_volume AS
SELECT
    DATE(execution_time) as trade_date,
    chain_id,
    COUNT(*) as trade_count,
    SUM(amount_in_usd) as volume_usd,
    AVG(profit_loss_percent) as avg_profit_percent
FROM trades
WHERE execution_time IS NOT NULL
    AND amount_in_usd IS NOT NULL
    AND status = 'executed'
GROUP BY DATE(execution_time), chain_id
ORDER BY trade_date DESC;

-- View for strategy performance
CREATE VIEW strategy_performance AS
SELECT
    s.id as strategy_id,
    s.name as strategy_name,
    s.strategy_type,
    u.telegram_id,
    COUNT(t.id) as total_trades,
    SUM(CASE WHEN t.status = 'executed' THEN 1 ELSE 0 END) as executed_trades,
    SUM(CASE WHEN t.profit_loss_usd > 0 THEN 1 ELSE 0 END) as winning_trades,
    SUM(t.profit_loss_usd) as total_pnl_usd,
    AVG(t.profit_loss_percent) as avg_profit_percent,
    MIN(t.execution_time) as first_trade,
    MAX(t.execution_time) as last_trade
FROM strategies s
LEFT JOIN trades t ON s.id = t.strategy_id
JOIN users u ON s.user_id = u.id
WHERE s.is_active = true
GROUP BY s.id, s.name, s.strategy_type, u.telegram_id
ORDER BY total_pnl_usd DESC NULLS LAST;

-- ============================================================================
-- Comments
-- ============================================================================

COMMENT ON VIEW active_positions IS 'Current open positions across all users';
COMMENT ON VIEW daily_trading_volume IS 'Daily trading volume and statistics';
COMMENT ON VIEW strategy_performance IS 'Performance metrics for all active strategies';

-- ============================================================================
-- Migration Completed
-- ============================================================================
