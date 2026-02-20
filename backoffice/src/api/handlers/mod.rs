//! API handlers module for the Velocidad trading bot backend.
//!
//! This module contains all HTTP request handlers for the API endpoints,
//! organized by resource type (auth, trades, portfolio, market, etc.).
//!
//! Each handler should:
//! - Validate input parameters
//! - Execute business logic
//! - Handle errors appropriately
//! - Return standardized responses

pub mod alerts;
pub mod auth;
pub mod health;
pub mod market;
pub mod portfolio;
pub mod trades;
pub mod user;

// Re-exports for convenience
pub use alerts::*;
pub use auth::*;
pub use health::*;
pub use market::*;
pub use portfolio::*;
pub use trades::*;
pub use user::*;

/// Handler prelude for commonly used imports
pub mod prelude {
    pub use crate::api::{
        errors::ApiError,
        extractors::{AuthUser, Pagination, TimeRange},
        state::AppState,
    };
    pub use crate::error::{Error, Result as AppResult};
    pub use crate::prelude::*;

    // Axum utilities
    pub use axum::{
        extract::{Path, Query, State},
        http::StatusCode,
        response::{IntoResponse, Json},
    };
    pub use serde::{Deserialize, Serialize};
    pub use tracing::{debug, error, info, instrument, warn};
}
