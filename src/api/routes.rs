use super::{
    auth::JwtVerifier,
    error::{ApiError, ApiResult},
    limits::RateLimits,
    stream::TicketStore,
};
use crate::{
    config::AppConfig,
    network::{pacer::Pacer, websocket::StreamHub},
};
use axum::{
    extract::State,
    http::{header, HeaderValue, Method},
    middleware,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::{cors::CorsLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    pub heavy_requests: Arc<tokio::sync::Semaphore>,
    pub pool: PgPool,
    pub config: Arc<AppConfig>,
    pub auth: JwtVerifier,
    pub limits: RateLimits,
    pub hub: StreamHub,
    pub sockets: crate::network::websocket::SocketLimits,
    pub pacer: Pacer,
    pub tickets: TicketStore,
    pub mqtt_tasks:
        Arc<tokio::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>>,
    pub mqtt_clients: Arc<
        tokio::sync::RwLock<
            std::collections::HashMap<String, crate::network::mqtt_listener::DeviceConnection>,
        >,
    >,
}
impl AppState {
    pub fn new(pool: PgPool, config: AppConfig) -> Self {
        let config = Arc::new(config);
        let hub = StreamHub::default();
        Self {
            pool,
            heavy_requests: Arc::new(tokio::sync::Semaphore::new(2)),
            auth: JwtVerifier::new(config.clone()),
            config,
            limits: RateLimits::default(),
            pacer: Pacer::new(hub.clone()),
            hub,
            sockets: Default::default(),
            tickets: TicketStore::default(),
            mqtt_clients: Default::default(),
            mqtt_tasks: Default::default(),
        }
    }
}
async fn live() -> Json<serde_json::Value> {
    Json(json!({"status":"alive","release":option_env!("BUILD_SHA").unwrap_or("development")}))
}
async fn ready(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let current = crate::db::postgres::schema_ready(&state.pool).await?;
    if !current {
        return Err(ApiError(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Schema not ready".into(),
        ));
    }
    Ok(Json(
        json!({"status":"ready","release":option_env!("BUILD_SHA").unwrap_or("development")}),
    ))
}
pub fn create_router(state: AppState) -> Router {
    let origins: Vec<HeaderValue> = state
        .config
        .allowed_origins
        .iter()
        .filter_map(|v| v.parse().ok())
        .collect();
    let private = Router::new()
        .merge(super::accounts::router())
        .merge(super::patients::router())
        .merge(super::devices::router())
        .merge(super::recordings::router())
        .merge(super::impersonation::router())
        .merge(super::stream::router())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            super::auth::authenticate,
        ));
    Router::new()
        .merge(private)
        .route(
            "/api/auth/register_profile",
            post(super::accounts::register_profile),
        )
        .route("/api/health", get(live))
        .route("/api/ready", get(ready))
        .route("/ws", get(crate::network::websocket::ws_handler))
        .route("/", get(crate::network::websocket::ws_handler))
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024))
        .layer(tower::limit::ConcurrencyLimitLayer::new(64))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    header::ACCEPT,
                    axum::http::HeaderName::from_static("x-impersonation-id"),
                    axum::http::HeaderName::from_static("idempotency-key"),
                ]),
        )
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
