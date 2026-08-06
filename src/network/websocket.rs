use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;
use axum::{
    extract::{ws::{WebSocket, WebSocketUpgrade, Message}, State},
    response::IntoResponse,
};
use tracing::info;
use futures_util::{stream::StreamExt, sink::SinkExt};

pub type ClientList = Arc<Mutex<Vec<UnboundedSender<String>>>>;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(clients): State<ClientList>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, clients))
}

async fn handle_socket(socket: WebSocket, clients: ClientList) {
    info!("[WebSocket] Koneksi WebSocket baru sedang dinegosiasikan...");
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    
    {
        let mut clients_lock = clients.lock().unwrap();
        clients_lock.push(tx);
        info!("[WebSocket] Client baru terhubung! Total klien aktif: {}", clients_lock.len());
    }

    // Task to forward messages to the websocket client
    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender.send(Message::Text(msg)).await.is_err() {
                break;
            }
        }
    });

    // Task to receive incoming messages from client (optional, logging only)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            if let Ok(text) = msg.to_text() {
                info!("[WebSocket] Menerima pesan dari client: {}", text);
            }
        }
    });

    // Wait until either task completes, then close the other
    tokio::select! {
        _ = (&mut send_task) => {},
        _ = (&mut recv_task) => {},
    }

    info!("[WebSocket] Koneksi client ditutup.");
}