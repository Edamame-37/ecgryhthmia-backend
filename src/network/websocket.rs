use crate::api::{
    authorization::Identity,
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::HeaderMap,
    response::Response,
};
use serde::Deserialize;
use std::sync::Weak;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{broadcast, Mutex, OwnedSemaphorePermit, RwLock, Semaphore};
#[derive(Clone)]
pub struct SocketLimits {
    global: Arc<Semaphore>,
    actors: Arc<Mutex<HashMap<String, Weak<Semaphore>>>>,
}
impl Default for SocketLimits {
    fn default() -> Self {
        Self {
            global: Arc::new(Semaphore::new(128)),
            actors: Default::default(),
        }
    }
}
impl SocketLimits {
    pub async fn acquire(
        &self,
        actor: &str,
    ) -> ApiResult<(OwnedSemaphorePermit, OwnedSemaphorePermit)> {
        let global = self.global.clone().try_acquire_owned().map_err(|_| {
            ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Socket capacity reached".into(),
            )
        })?;
        let mut actors = self.actors.lock().await;
        actors.retain(|_, value| value.strong_count() > 0);
        let limiter = actors
            .get(actor)
            .and_then(Weak::upgrade)
            .unwrap_or_else(|| Arc::new(Semaphore::new(4)));
        actors.insert(actor.to_owned(), Arc::downgrade(&limiter));
        let actor = limiter.try_acquire_owned().map_err(|_| {
            ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Account socket limit reached".into(),
            )
        })?;
        Ok((global, actor))
    }
}
#[derive(Clone, Default)]
pub struct StreamHub(Arc<RwLock<HashMap<String, broadcast::Sender<String>>>>);
impl StreamHub {
    pub async fn subscribe(&self, session: &str) -> ApiResult<broadcast::Receiver<String>> {
        let mut channels = self.0.write().await;
        channels.retain(|_, channel| channel.receiver_count() > 0);
        if channels.len() >= 1024 && !channels.contains_key(session) {
            return Err(ApiError::conflict("Subscriber limit reached"));
        }
        Ok(channels
            .entry(session.into())
            .or_insert_with(|| broadcast::channel(64).0)
            .subscribe())
    }
    pub async fn publish(&self, session: &str, message: String) {
        if let Some(channel) = self.0.read().await.get(session) {
            let _ = channel.send(message);
        }
    }
}
#[derive(Deserialize)]
pub struct SocketQuery {
    ticket: String,
}
pub async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<SocketQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    let origin = headers
        .get("Origin")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(ApiError::forbidden)?;
    if !state
        .config
        .allowed_origins
        .iter()
        .any(|allowed| allowed == origin)
    {
        return Err(ApiError::forbidden());
    }
    let (identity, session) = state.tickets.consume(&query.ticket).await?;
    let identity = revalidate(&state, &identity).await?;
    identity.session(&state.pool, &session).await?;
    let permits = state.sockets.acquire(&identity.actor_id).await?;
    let receiver = state.hub.subscribe(&session).await?;
    Ok(upgrade
        .max_message_size(16384)
        .on_upgrade(move |socket| async move {
            let _permits = permits;
            handle_socket(socket, state, identity, session, receiver).await
        }))
}
async fn handle_socket(
    mut socket: WebSocket,
    state: AppState,
    identity: Identity,
    session: String,
    mut receiver: broadcast::Receiver<String>,
) {
    let mut check = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
         _=check.tick()=>{
          let current=revalidate(&state,&identity).await;
          match current {Ok(ref current) if current.session(&state.pool,&session).await.is_ok()=>{},_=>break}
          if socket.send(Message::Ping(vec![])).await.is_err(){break}
         },
         message=receiver.recv()=>match message {Ok(message)=>{if !matches!(tokio::time::timeout(Duration::from_secs(5),socket.send(Message::Text(message))).await,Ok(Ok(()))){break}},Err(_)=>break},
         incoming=socket.recv()=>match incoming {
          Some(Ok(Message::Text(text)))=>{
           if state.limits.check(&format!("{}:ws-command",identity.actor_id),30).await.is_err(){break}
           let command:serde_json::Value=match serde_json::from_str(&text){Ok(command)=>command,Err(_)=>break};
           if command["command"]=="get_segment" {
            if let Some(index)=command["index"].as_u64() {
             match crate::api::recordings::segment_packet(&state,&identity,&session,index).await {
              Ok(packet)=>if !matches!(tokio::time::timeout(Duration::from_secs(5),socket.send(Message::Text(packet))).await,Ok(Ok(()))){break},
              Err(_)=>break
             }
            }
           }
          },
          Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,
          _=>{}
         }
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

async fn revalidate(state: &AppState, identity: &Identity) -> ApiResult<Identity> {
    let token = identity
        .access_token
        .as_deref()
        .ok_or_else(ApiError::unauthorized)?;
    let claims = state.auth.verify(token).await?;
    let mut current =
        crate::api::auth::authorize(state, &claims, identity.impersonation_id.as_deref()).await?;
    current.access_token = identity.access_token.clone();
    Ok(current)
}
