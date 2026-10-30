mod support;
use ecg_backend::{
    api::authorization::Identity,
    services::recording::{self, Command, CommandInput, DeviceAck},
};
use serde_json::json;
use support::*;
#[tokio::test]
async fn commands_are_idempotent_and_only_acknowledgments_change_recording_state() {
    let context = TestContext::new().await;
    sqlx::query("UPDATE sessions SET state='ended',ended_at=now()")
        .execute(&context.state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO device_credentials(device_id,mqtt_broker,mqtt_port,mqtt_topic,mqtt_username,mqtt_password) VALUES('dev_A','test.invalid',8883,'ecg/test','test','test')").execute(&context.state.pool).await.unwrap();
    let options = rumqttc::MqttOptions::new("local-test", "127.0.0.1", 1883);
    let (client, _event_loop) = rumqttc::AsyncClient::new(options, 10);
    context.state.mqtt_clients.write().await.insert(
        "dev_A".into(),
        ecg_backend::network::mqtt_listener::DeviceConnection {
            client,
            revision: 1,
        },
    );
    let identity = Identity::load(&context.state.pool, &claims("patient"))
        .await
        .unwrap();
    let start = || CommandInput {
        command: Command::Start,
        patient_id: Some("pat_A".into()),
        session_id: None,
    };
    let (first, retry) = tokio::join!(
        recording::command(&context.state, &identity, "dev_A", start(), "start_key"),
        recording::command(&context.state, &identity, "dev_A", start(), "start_key")
    );
    let first = first.unwrap();
    assert_eq!(retry.unwrap(), first);
    assert_eq!(first["state"], "requested");
    assert_eq!(
        recording::command(&context.state, &identity, "dev_A", start(), "start_key")
            .await
            .unwrap(),
        first
    );
    let id = first["session_id"].as_str().unwrap().to_owned();
    assert!(recording::command(
        &context.state,
        &identity,
        "dev_A",
        CommandInput {
            command: Command::Stop,
            patient_id: Some("pat_A".into()),
            session_id: Some(id.clone())
        },
        "start_key"
    )
    .await
    .is_err());
    recording::acknowledge(
        &context.state,
        "dev_A",
        DeviceAck {
            command: Command::Start,
            session_id: id.clone(),
            last_frame_number: None,
        },
    )
    .await
    .unwrap();
    let mut frame = payload(1);
    frame.session_id = id.clone();
    ecg_backend::services::ingestion::persist(&context.state, "dev_A", frame, true)
        .await
        .unwrap();
    let stop = || CommandInput {
        command: Command::Stop,
        patient_id: Some("pat_A".into()),
        session_id: Some(id.clone()),
    };
    assert_eq!(
        recording::command(&context.state, &identity, "dev_A", stop(), "stop_key")
            .await
            .unwrap()["state"],
        "stopping"
    );
    recording::acknowledge(
        &context.state,
        "dev_A",
        DeviceAck {
            command: Command::Stop,
            session_id: id.clone(),
            last_frame_number: Some(1),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        recording::command(&context.state, &identity, "dev_A", stop(), "stop_key")
            .await
            .unwrap()["state"],
        "ended"
    );
    assert_eq!(
        json_response(
            context
                .request(
                    "GET",
                    "/api/patients/pat_A/recording-status",
                    Some("patient"),
                    json!({})
                )
                .await
        )
        .await["is_recording"],
        false
    );
    context.cleanup().await;
}
#[tokio::test]
async fn orphan_cleanup_preserves_referenced_data_and_never_leaves_the_storage_root() {
    let context = TestContext::new().await;
    let frame =
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .unwrap();
    let orphan = context.state.config.records_root.join("orphan.pending");
    tokio::fs::write(&orphan, b"test").await.unwrap();
    ecg_backend::services::storage::reserve(&context.state, "records", &orphan)
        .await
        .unwrap();
    let outside = context
        .state
        .config
        .records_root
        .parent()
        .unwrap()
        .join("outside.pending");
    tokio::fs::write(&outside, b"test").await.unwrap();
    ecg_backend::services::storage::reserve(&context.state, "records", &outside)
        .await
        .unwrap();
    sqlx::query("UPDATE file_cleanup SET not_before=now()")
        .execute(&context.state.pool)
        .await
        .unwrap();
    ecg_backend::services::storage::process(&context.state)
        .await
        .unwrap();
    assert!(!orphan.exists());
    assert!(outside.exists());
    assert!(context
        .state
        .config
        .records_root
        .join("ses_pat_A")
        .join(format!("{}.jsonl", frame.id))
        .exists());
    let retries: i32 = sqlx::query_scalar("SELECT attempts FROM file_cleanup WHERE path=$1")
        .bind(outside.to_string_lossy().as_ref())
        .fetch_one(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(retries, 1);
    context.cleanup().await;
}
