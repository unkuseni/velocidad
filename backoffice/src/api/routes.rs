//! API routes for the Velocidad trading bot backend.
//!
//! This module defines the HTTP routes for the API server using Axum.

use axum::{
    middleware,
    routing::{get, post, put, delete},
    Router,
};

use crate::api::{
    handlers::{
        auth::{login, logout, refresh_token, register},
        health::health_check,
        market::{get_market_data, get_price_history, search_tokens},
        portfolio::{get_portfolio, get_positions, get_trade_history},
        trades::{cancel_trade, execute_trade, get_trade, list_trades},
        alerts::{create_alert, delete_alert, get_alert, list_alerts, update_alert},
        user::{get_user_profile, update_user_settings},
    },
    middleware::{
        auth::require_auth,
        rate_limit::rate_limiter,
    },
    websocket::handle_websocket,
    API_V1_PREFIX,
};

/// Build the main API router with all routes and middleware.
pub fn build_api_router() -> Router {
    Router::new()
        // Public routes (no authentication required)
        .route("/health", get(health_check))
        .nest("/auth", auth_routes())
        // API v1 routes (with authentication)
        .nest(API_V1_PREFIX, api_v1_routes())
        // WebSocket endpoint
        .route("/ws", get(handle_websocket))
}

/// Authentication routes (public)
fn auth_routes() -> Router {
    Router::new()
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/refresh", post(refresh_token))
        .route("/logout", post(logout))
        // Apply rate limiting to auth endpoints
        .layer(middleware::from_fn(rate_limiter))
}

/// API v1 routes (protected by authentication)
fn api_v1_routes() -> Router {
    Router::new()
        // Trading routes
        .nest("/trades", trade_routes())
        // Portfolio routes
        .nest("/portfolio", portfolio_routes())
        // Market data routes
        .nest("/market", market_routes())
        // Alert routes
        .nest("/alerts", alert_routes())
        // User routes
        .nest("/user", user_routes())
        // Apply authentication middleware to all v1 routes
        .layer(middleware::from_fn(require_auth))
}

/// Trading routes
fn trade_routes() -> Router {
    Router::new()
        .route("/", get(list_trades).post(execute_trade))
        .route("/{id}", get(get_trade).delete(cancel_trade))
        // Additional trading endpoints
        .route("/{id}/status", get(get_trade)) // TODO: Separate status endpoint
        .route("/{id}/cancel", post(cancel_trade))
}

/// Portfolio routes
fn portfolio_routes() -> Router {
    Router::new()
        .route("/", get(get_portfolio))
        .route("/positions", get(get_positions))
        .route("/history", get(get_trade_history))
        // Additional portfolio endpoints
        .route("/performance", get(get_portfolio)) // TODO: Add performance endpoint
        .route("/allocations", get(get_portfolio)) // TODO: Add allocations endpoint
}

/// Market data routes
fn market_routes() -> Router {
    Router::new()
        .route("/prices", get(get_market_data))
        .route("/prices/{token}", get(get_market_data))
        .route("/history/{token}", get(get_price_history))
        .route("/tokens/search", get(search_tokens))
        // Additional market endpoints
        .route("/tokens/{address}", get(get_market_data)) // TODO: Add token details endpoint
        .route("/charts/{token}/{timeframe}", get(get_price_history)) // TODO: Add chart endpoint
}

/// Alert routes
fn alert_routes() -> Router {
    Router::new()
        .route("/", get(list_alerts).post(create_alert))
        .route("/{id}", get(get_alert).put(update_alert).delete(delete_alert))
        // Additional alert endpoints
        .route("/{id}/enable", put(update_alert)) // TODO: Add enable/disable endpoints
        .route("/{id}/disable", put(update_alert))
}

/// User routes
fn user_routes() -> Router {
    Router::new()
        .route("/profile", get(get_user_profile))
        .route("/settings", put(update_user_settings))
        // Additional user endpoints
        .route("/wallets", get(get_user_profile)) // TODO: Add wallet management
        .route("/api-keys", get(get_user_profile)) // TODO: Add API key management
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_check() {
        let app = build_api_router();

        let response = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_auth_routes_exist() {
        let app = build_api_router();

        let endpoints = vec!["/auth/register", "/auth/login", "/auth/refresh", "/auth/logout"];

        for endpoint in endpoints {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(endpoint)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // These should return 400 (bad request) rather than 404 (not found)
            // because the handlers exist but expect request bodies
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "Endpoint {} should exist",
                endpoint
            );
        }
    }

    #[tokio::test]
    async fn test_protected_routes_require_auth() {
        let app = build_api_router();

        let protected_endpoints = vec![
            "/api/v1/trades",
            "/api/v1/portfolio",
            "/api/v1/market/prices",
            "/api/v1/alerts",
            "/api/v1/user/profile",
        ];

        for endpoint in protected_endpoints {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(endpoint)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // Should return 401 Unauthorized without auth token
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "Endpoint {} should require authentication",
                endpoint
            );
        }
    }

    #[tokio::test]
    async fn test_websocket_endpoint_exists() {
        let app = build_api_router();

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // WebSocket endpoint should accept GET requests
        // It might return 400 Bad Request (not a WebSocket request) rather than 404
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "WebSocket endpoint should exist"
        );
    }
}
