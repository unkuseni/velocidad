//! API module for the Velocidad trading bot backend.
//!
//! This module defines the HTTP API endpoints using Axum, including:
//! - Trading endpoints (execute trades, get trade history)
//! - Portfolio endpoints (get portfolio, positions)
//! - Market data endpoints (prices, charts, analytics)
//! - User management endpoints (authentication, settings)
//! - WebSocket endpoints for real-time updates

pub mod auth;
pub mod errors;
pub mod extractors;
pub mod handlers;
pub mod middleware;
pub mod routes;
pub mod state;
pub mod websocket;

// Re-exports for convenience
pub use auth::*;
pub use errors::*;
pub use extractors::*;
pub use handlers::*;
pub use middleware::*;
pub use routes::*;
pub use state::*;
pub use websocket::*;

/// API version constants
pub const API_V1_PREFIX: &str = "/api/v1";
