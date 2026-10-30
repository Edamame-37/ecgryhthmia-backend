use super::{
    authorization::Identity,
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{extract::State, routing::post, Extension, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
type PendingTickets = HashMap<String, (Instant, Identity, String)>;
#[derive(Clone, Default)]
pub struct TicketStore(Arc<Mutex<PendingTickets>>);
impl TicketStore {
    pub async fn issue(&self, identity: Identity, session: String) -> ApiResult<String> {
        let mut tickets = self.0.lock().await;
        tickets.retain(|_, (at, _, _)| at.elapsed() < Duration::from_secs(30));
        if tickets.len() >= 1024 {
            return Err(ApiError::conflict("Too many pending connections"));
        }
        let ticket = uuid::Uuid::new_v4().to_string();
        tickets.insert(ticket.clone(), (Instant::now(), identity, session));
        Ok(ticket)
    }
    pub async fn consume(&self, ticket: &str) -> ApiResult<(Identity, String)> {
        let (at, identity, session) = self
            .0
            .lock()
            .await
            .remove(ticket)
            .ok_or_else(ApiError::unauthorized)?;
        if at.elapsed() >= Duration::from_secs(30) {
            return Err(ApiError::unauthorized());
        }
        Ok((identity, session))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketRequest {
    session_id: String,
}
async fn issue(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<TicketRequest>,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &body.session_id).await?;
    let ticket = state.tickets.issue(identity, body.session_id).await?;
    Ok(Json(json!({"ticket":ticket,"expires_in":30})))
}
pub fn router() -> Router<AppState> {
    Router::new().route("/api/stream/ticket", post(issue))
}
