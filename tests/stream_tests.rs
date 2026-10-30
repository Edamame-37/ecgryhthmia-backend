mod support;
use futures_util::StreamExt;
use serde_json::json;
use std::time::Duration;
use support::*;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
};
#[tokio::test]
async fn websocket_tickets_origin_scope_reuse_and_revocation() {
    let context = TestContext::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = context.app.clone();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let denied = context
        .request(
            "POST",
            "/api/stream/ticket",
            Some("other"),
            json!({"session_id":"ses_pat_A"}),
        )
        .await;
    assert_eq!(denied.status().as_u16(), 403);
    let ticket = json_response(
        context
            .request(
                "POST",
                "/api/stream/ticket",
                Some("patient"),
                json!({"session_id":"ses_pat_A"}),
            )
            .await,
    )
    .await["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let ticket_b = json_response(
        context
            .request(
                "POST",
                "/api/stream/ticket",
                Some("other"),
                json!({"session_id":"ses_pat_B"}),
            )
            .await,
    )
    .await["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let request = |ticket: &str, origin: &str| {
        let mut request = format!("ws://{address}/ws?ticket={ticket}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_str(origin).unwrap());
        request
    };
    assert!(connect_async(request(&ticket, "https://untrusted.invalid"))
        .await
        .is_err());
    let (mut a, _) = connect_async(request(&ticket, "https://app.ecg.test"))
        .await
        .unwrap();
    let (mut b, _) = connect_async(request(&ticket_b, "https://app.ecg.test"))
        .await
        .unwrap();
    assert!(connect_async(request(&ticket, "https://app.ecg.test"))
        .await
        .is_err());
    let packet = ecg_backend::models::payload::encode_packet(
        &json!({"type":"live_data","session_id":"ses_pat_A"}),
    )
    .unwrap();
    context.state.hub.publish("ses_pat_A", packet.clone()).await;
    let text = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(Ok(Message::Text(text))) = a.next().await {
                break text;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(text, packet);
    assert!(tokio::time::timeout(Duration::from_millis(200), async {
        loop {
            if let Some(Ok(Message::Text(text))) = b.next().await {
                break text;
            }
        }
    })
    .await
    .is_err());
    sqlx::query("UPDATE accounts SET enabled=false WHERE id='acc_patient'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(18), async {
        while let Some(message) = a.next().await {
            if matches!(message, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    })
    .await
    .expect("Revocation must close the subscription within the 15 second check interval");
    server.abort();
    context.cleanup().await;
}
#[tokio::test]
async fn slow_device_does_not_delay_another_devices_first_packet() {
    let context = TestContext::new().await;
    let mut a = context.state.hub.subscribe("ses_pat_A").await.unwrap();
    let mut b = context.state.hub.subscribe("ses_pat_B").await.unwrap();
    let mut long = payload(1);
    long.ecg.samples = vec![vec![0.5, 0.6, 0.1]; 2500];
    long.duration_s = 10.0;
    ecg_backend::services::ingestion::persist(&context.state, "dev_A", long, true)
        .await
        .unwrap();
    let mut short = payload(1);
    short.device_id = "device_pat_B".into();
    short.session_id = "ses_pat_B".into();
    ecg_backend::services::ingestion::persist(&context.state, "dev_B", short, true)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), a.recv())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_millis(500), b.recv())
        .await
        .expect("A 10-second frame must not block another device")
        .unwrap();
    context.cleanup().await;
}

#[tokio::test]
async fn socket_capacity_is_per_actor_and_released_on_disconnect() {
    let limits = ecg_backend::network::websocket::SocketLimits::default();
    let mut permits = Vec::new();
    for _ in 0..4 {
        permits.push(limits.acquire("actor_A").await.unwrap());
    }
    assert_eq!(limits.acquire("actor_A").await.unwrap_err().0.as_u16(), 429);
    let other = limits.acquire("actor_B").await.unwrap();
    drop(other);
    drop(permits.pop());
    assert!(limits.acquire("actor_A").await.is_ok());
}
