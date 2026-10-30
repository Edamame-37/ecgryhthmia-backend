mod support;
use ecg_backend::{
    api::authorization::Identity,
    services::recording::{self, Command, CommandInput, DeviceAck},
};
use serde_json::json;
use std::process::Command as ProcessCommand;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inventory_reports_legacy_sources_without_changing_database_or_files() {
    let context = TestContext::new().await;
    let source = context.state.config.records_root.join("inventory.jsonl");
    std::fs::write(&source, serde_json::to_vec(&payload(1)).unwrap()).unwrap();
    sqlx::query(
        "UPDATE sessions SET state='ended',ended_at=now(),file_path=$1 WHERE id='ses_pat_A'",
    )
    .bind(source.to_string_lossy().as_ref())
    .execute(&context.state.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO frame_records(id,session_id,start_time,end_time,time_interval,label) VALUES('fra_inventory','ses_pat_A',0,0.2,'0 - 0.2','UNKNOWN')")
        .execute(&context.state.pool)
        .await
        .unwrap();
    let before: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('session',to_jsonb(s),'frames',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM frame_records f WHERE f.session_id=s.id)) FROM sessions s WHERE s.id='ses_pat_A'",
    )
    .fetch_one(&context.state.pool)
    .await
    .unwrap();
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let output = tokio::task::block_in_place(|| {
        ProcessCommand::new(env!("CARGO_BIN_EXE_reconcile"))
            .env(
                "DATABASE_URL",
                format!("{url}{separator}options=-csearch_path%3D{}", context.schema),
            )
            .env("SUPABASE_URL", "https://auth.ecg.test")
            .env("SUPABASE_ANON_KEY", "synthetic-public")
            .env("RECORDS_ROOT", &context.state.config.records_root)
            .env("UPLOADS_ROOT", &context.state.config.uploads_root)
            .arg("--inventory")
            .output()
            .unwrap()
    });
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["mode"], "read_only_inventory");
    assert_eq!(report["readable_sources"], 1);
    let after: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('session',to_jsonb(s),'frames',(SELECT jsonb_agg(to_jsonb(f) ORDER BY f.id) FROM frame_records f WHERE f.session_id=s.id)) FROM sessions s WHERE s.id='ses_pat_A'",
    )
    .fetch_one(&context.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert!(source.exists());
    context.cleanup().await;
}
