-- Down migration for initial database schema
-- Migration: 0001_initial_schema_down.sql
-- Description: Drops all tables, views, functions, triggers, and extensions created in the up migration
-- WARNING: This will permanently delete all data in the database!

-- ============================================================================
-- Drop Views (depend on tables)
-- ============================================================================

DROP VIEW IF EXISTS strategy_performance;
DROP VIEW IF EXISTS daily_trading_volume;
DROP VIEW IF EXISTS active_positions;

-- ============================================================================
-- Drop Triggers (depend on functions and tables)
-- Must be dropped before dropping the functions they reference
-- ============================================================================

DROP TRIGGER IF EXISTS update_api_keys_updated_at ON api_keys;
DROP TRIGGER IF EXISTS update_portfolio_positions_updated_at ON portfolio_positions;
DROP TRIGGER IF EXISTS update_alerts_updated_at ON alerts;
DROP TRIGGER IF EXISTS update_trades_updated_at ON trades;
DROP TRIGGER IF EXISTS update_strategies_updated_at ON strategies;
DROP TRIGGER IF EXISTS update_users_updated_at ON users;

-- ============================================================================
-- Drop Functions
-- ============================================================================

DROP FUNCTION IF EXISTS data_age(timestamp_column TIMESTAMPTZ);
DROP FUNCTION IF EXISTS update_updated_at_column();

-- ============================================================================
-- Drop Tables
-- Dropped in reverse order of creation to respect foreign key dependencies
-- ============================================================================

-- Rate limits depend on api_keys and users
DROP TABLE IF EXISTS rate_limits;

-- API keys depend on users
DROP TABLE IF EXISTS api_keys;

-- Alert triggers depend on alerts
DROP TABLE IF EXISTS alert_triggers;

-- Alerts depend on users
DROP TABLE IF EXISTS alerts;

-- Portfolio positions depend on users
DROP TABLE IF EXISTS portfolio_positions;

-- Trades depend on users and strategies
DROP TABLE IF EXISTS trades;

-- Strategies depend on users
DROP TABLE IF EXISTS strategies;

-- Market data has no foreign key dependencies
DROP TABLE IF EXISTS market_data;

-- Users is the base table
DROP TABLE IF EXISTS users;

-- ============================================================================
-- Drop Extensions
-- ============================================================================

DROP EXTENSION IF EXISTS pg_trgm;
DROP EXTENSION IF EXISTS pgcrypto;
DROP EXTENSION IF EXISTS "uuid-ossp";

-- ============================================================================
-- Migration Completed
-- ============================================================================
