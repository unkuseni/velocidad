//! Health check handler for the Velocidad trading bot backend.
//!
//! This module provides endpoints for checking the health and status of the API service,
//! including database connectivity, Redis connectivity, and general system status.

use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::{
    api::state::AppState,
    database::Database,
    error::{Error, Result as AppResult},
    prelude::*,
};

/// Health check response structure
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// Service status
    pub status: HealthStatus,

    /// Service name
    pub service: String,

    /// Service version
    pub version: String,

    /// Timestamp of the health check
    pub timestamp: DateTime<Utc>,

    /// Uptime in seconds
    pub uptime_seconds: u64,

    /// Database connectivity status
    pub database: HealthComponentStatus,

    /// Redis connectivity status (if configured)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis: Option<HealthComponentStatus>,

    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Health status enumeration
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Service is healthy and fully operational
    Healthy,

    /// Service is degraded but still functional
    Degraded,

    /// Service is unhealthy and may not be fully functional
    Unhealthy,
}

/// Component health status
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthComponentStatus {
    /// Component status
    pub status: HealthStatus,

    /// Response time in milliseconds (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,

    /// Optional error message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Timestamp of the component check
    pub checked_at: DateTime<Utc>,
}

impl HealthComponentStatus {
    /// Create a healthy component status
    fn healthy(response_time_ms: Option<u64>) -> Self {
        Self {
            status: HealthStatus::Healthy,
            response_time_ms,
            error: None,
            checked_at: Utc::now(),
        }
    }

    /// Create an unhealthy component status
    fn unhealthy(error: impl Into<String>, response_time_ms: Option<u64>) -> Self {
        Self {
            status: HealthStatus::Unhealthy,
            response_time_ms,
            error: Some(error.into()),
            checked_at: Utc::now(),
        }
    }
}

/// Liveness probe endpoint
///
/// This endpoint checks if the service is running and responsive.
/// It does NOT check external dependencies like databases or Redis.
///
/// Returns:
/// - 200 OK with simple status if service is running
/// - 503 Service Unavailable if service is not ready
pub async fn liveness_probe() -> impl IntoResponse {
    debug!("Liveness probe requested");

    let response = serde_json::json!({
        "status": "alive",
        "timestamp": Utc::now(),
        "service": "velocidad-api",
    });

    (StatusCode::OK, Json(response))
}

/// Readiness probe endpoint
///
/// This endpoint checks if the service is ready to handle requests.
/// It checks critical dependencies like database connectivity.
///
/// Returns:
/// - 200 OK with detailed status if service is ready
/// - 503 Service Unavailable if service is not ready
pub async fn readiness_probe(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, Error> {
    debug!("Readiness probe requested");

    let start_time = Instant::now();
    let mut overall_status = HealthStatus::Healthy;
    let mut details = serde_json::Map::new();

    // Check database connectivity
    let db_status = check_database(&state.database).await;
    if let HealthStatus::Unhealthy = db_status.status {
        overall_status = HealthStatus::Unhealthy;
    }
    details.insert("database".to_string(), serde_json::to_value(&db_status).unwrap());

    // Check Redis connectivity if configured
    if let Some(redis_pool) = &state.redis_pool {
        let redis_status = check_redis(redis_pool).await;
        if let HealthStatus::Unhealthy = redis_status.status {
            overall_status = HealthStatus::Degraded;
        }
        details.insert("redis".to_string(), serde_json::to_value(&redis_status).unwrap());
    }

    let response_time = start_time.elapsed().as_millis() as u64;

    let response = HealthCheckResponse {
        status: overall_status,
        service: "velocidad-api".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
        database: db_status,
        redis: state.redis_pool.as_ref().map(|_| check_redis(&state.redis_pool.as_ref().unwrap()).await),
        details: Some(serde_json::Value::Object(details)),
    };

    info!(
        status = ?overall_status,
        response_time_ms = response_time,
        "Readiness probe completed"
    );

    // Return appropriate status code based on overall health
    let status_code = match overall_status {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Degraded => StatusCode::OK, // Still OK but with degraded status
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    };

    Ok((status_code, Json(response)))
}

/// Comprehensive health check endpoint
///
/// This endpoint provides detailed health information about all system components.
/// It checks database, Redis, and any other configured dependencies.
///
/// Returns:
/// - 200 OK with detailed health information
/// - 503 Service Unavailable if critical components are unhealthy
pub async fn health_check(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, Error> {
    debug!("Health check requested");

    let start_time = Instant::now();
    let mut overall_status = HealthStatus::Healthy;
    let mut details = serde_json::Map::new();

    // Check database connectivity with performance measurement
    let db_start = Instant::now();
    let db_status = check_database(&state.database).await;
    let db_response_time = db_start.elapsed().as_millis() as u64;

    let db_status_with_timing = HealthComponentStatus {
        response_time_ms: Some(db_response_time),
        ..db_status
    };

    if let HealthStatus::Unhealthy = db_status.status {
        overall_status = HealthStatus::Unhealthy;
    }
    details.insert("database".to_string(), serde_json::to_value(&db_status_with_timing).unwrap());

    // Check Redis connectivity if configured
    if let Some(redis_pool) = &state.redis_pool {
        let redis_start = Instant::now();
        let redis_status = check_redis(redis_pool).await;
        let redis_response_time = redis_start.elapsed().as_millis() as u64;

        let redis_status_with_timing = HealthComponentStatus {
            response_time_ms: Some(redis_response_time),
            ..redis_status
        };

        if let HealthStatus::Unhealthy = redis_status.status {
            overall_status = match overall_status {
                HealthStatus::Healthy => HealthStatus::Degraded,
                current => current,
            };
        }
        details.insert("redis".to_string(), serde_json::to_value(&redis_status_with_timing).unwrap());
    }

    // Check message queue if configured
    #[cfg(feature = "rabbitmq")]
    if let Some(mq_connection) = &state.mq_connection {
        let mq_status = check_message_queue(mq_connection).await;
        details.insert("message_queue".to_string(), serde_json::to_value(&mq_status).unwrap());

        if let HealthStatus::Unhealthy = mq_status.status {
            overall_status = HealthStatus::Degraded;
        }
    }

    // Add system information
    details.insert("system".to_string(), serde_json::json!({
        "concurrent_requests": state.request_counter.load(std::sync::atomic::Ordering::Relaxed),
        "environment": state.config.environment.as_str(),
        "log_level": state.config.monitoring.log_level,
    }));

    let total_response_time = start_time.elapsed().as_millis() as u64;

    let response = HealthCheckResponse {
        status: overall_status,
        service: "velocidad-api".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
        uptime_seconds: state.start_time.elapsed().as_secs(),
        database: db_status_with_timing,
        redis: state.redis_pool.as_ref().map(|pool| {
            let status = check_redis(pool).await;
            HealthComponentStatus {
                response_time_ms: None, // Already included in details
                ..status
            }
        }),
        details: Some(serde_json::Value::Object(details)),
    };

    info!(
        status = ?overall_status,
        response_time_ms = total_response_time,
        "Health check completed"
    );

    // Return appropriate status code based on overall health
    let status_code = match overall_status {
        HealthStatus::Healthy => StatusCode::OK,
        HealthStatus::Degraded => StatusCode::OK, // Still OK but with degraded status
        HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
    };

    Ok((status_code, Json(response)))
}

/// Check database connectivity
async fn check_database(database: &Database) -> HealthComponentStatus {
    let start_time = Instant::now();

    match database.pool().acquire().await {
        Ok(conn) => {
            let response_time = start_time.elapsed().as_millis() as u64;

            // Try to execute a simple query
            match sqlx::query("SELECT 1").execute(&conn).await {
                Ok(_) => HealthComponentStatus::healthy(Some(response_time)),
                Err(e) => {
                    warn!(error = %e, "Database query failed");
                    HealthComponentStatus::unhealthy(format!("Query failed: {}", e), Some(response_time))
                }
            }
        }
        Err(e) => {
            let response_time = start_time.elapsed().as_millis() as u64;
            warn!(error = %e, "Database connection failed");
            HealthComponentStatus::unhealthy(format!("Connection failed: {}", e), Some(response_time))
        }
    }
}

/// Check Redis connectivity
async fn check_redis(redis_pool: &bb8::Pool<redis::aio::ConnectionManager>) -> HealthComponentStatus {
    let start_time = Instant::now();

    match redis_pool.get().await {
        Ok(mut conn) => {
            // Try to execute a simple PING command
            match redis::cmd("PING").query_async::<_, String>(&mut *conn).await {
                Ok(pong) if pong == "PONG" => {
                    let response_time = start_time.elapsed().as_millis() as u64;
                    HealthComponentStatus::healthy(Some(response_time))
                }
                Ok(pong) => {
                    let response_time = start_time.elapsed().as_millis() as u64;
                    warn!(pong = %pong, "Redis returned unexpected PONG response");
                    HealthComponentStatus::unhealthy(
                        format!("Unexpected response: {}", pong),
                        Some(response_time),
                    )
                }
                Err(e) => {
                    let response_time = start_time.elapsed().as_millis() as u64;
                    warn!(error = %e, "Redis PING failed");
                    HealthComponentStatus::unhealthy(format!("PING failed: {}", e), Some(response_time))
                }
            }
        }
        Err(e) => {
            let response_time = start_time.elapsed().as_millis() as u64;
            warn!(error = %e, "Redis connection failed");
            HealthComponentStatus::unhealthy(format!("Connection failed: {}", e), Some(response_time))
        }
    }
}

/// Check message queue connectivity (RabbitMQ)
#[cfg(feature = "rabbitmq")]
async fn check_message_queue(connection: &lapin::Connection) -> HealthComponentStatus {
    let start_time = Instant::now();

    match connection.status().state() {
        lapin::ConnectionState::Connected => {
            let response_time = start_time.elapsed().as_millis() as u64;
            HealthComponentStatus::healthy(Some(response_time))
        }
        state => {
            let response_time = start_time.elapsed().as_millis() as u64;
            warn!(state = ?state, "Message queue not connected");
            HealthComponentStatus::unhealthy(
                format!("Connection state: {:?}", state),
                Some(response_time),
            )
        }
    }
}

/// Simple status endpoint for load balancers and monitors
pub async fn status() -> impl IntoResponse {
    let response = serde_json::json!({
        "status": "ok",
        "service": "velocidad-api",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": Utc::now(),
    });

    (StatusCode::OK, Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    // Note: These tests would require proper test setup with mock dependencies
    // For now, they serve as documentation of expected behavior

    #[tokio::test]
    async fn test_liveness_probe_always_returns_ok() {
        let response = liveness_probe().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "alive");
        assert_eq!(json["service"], "velocidad-api");
        assert!(json.get("timestamp").is_some());
    }

    #[tokio::test]
    async fn test_status_endpoint_returns_basic_info() {
        let response = status().await.into_response();
        assert_eq!(response.status(), StatusCode::OK);

        let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "velocidad-api");
        assert!(json.get("version").is_some());
        assert!(json.get("timestamp").is_some());
    }

    // Integration tests for health check endpoints would require:
    // 1. Mock AppState with database and Redis pools
    // 2. Test containers for database and Redis
    // 3. Proper test setup and teardown
}
