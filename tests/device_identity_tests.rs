mod support;
use axum::http::StatusCode;
use serde_json::json;
use support::*;

fn device_input(hardware_id: Option<&str>, name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "hardware_id": hardware_id,
        "mqtt_broker": "test.invalid",
        "mqtt_port": 8883,
        "mqtt_topic": "ecg/device-new",
        "mqtt_username": "fixture",
        "mqtt_password": "fixture-password"
    })
}

#[tokio::test]
async fn registration_keeps_internal_display_and_hardware_identities_separate() {
    let context = TestContext::new().await;
    let response = context
        .request(
            "POST",
            "/api/admin/devices",
            Some("admin"),
            device_input(Some("physical_001"), "Examination room monitor"),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved = json_response(response).await;
    assert_eq!(saved["saved"], true);
    assert_eq!(saved["connection_status"], "connecting");
    let internal_id = saved["id"].as_str().unwrap();
    assert!(internal_id.starts_with("dev_"));
    assert_ne!(internal_id, "physical_001");

    let listed = json_response(
        context
            .request("GET", "/api/admin/devices", Some("admin"), json!({}))
            .await,
    )
    .await;
    let registered = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|device| device["id"] == internal_id)
        .unwrap();
    assert_eq!(registered["name"], "Examination room monitor");
    assert_eq!(registered["hardware_id"], "physical_001");
    assert_eq!(registered["mapping_status"], "mapped");

    let edit = json!({
        "name": "Renamed monitor",
        "mqtt_broker": "test.invalid",
        "mqtt_port": 8883,
        "mqtt_topic": "ecg/device-new",
        "mqtt_username": "fixture",
        "mqtt_password": ""
    });
    let response = context
        .request(
            "PUT",
            &format!("/api/admin/devices/{internal_id}"),
            Some("admin"),
            edit,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mapping: String = sqlx::query_scalar("SELECT hardware_id FROM devices WHERE id=$1")
        .bind(internal_id)
        .fetch_one(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(mapping, "physical_001");

    let duplicate = context
        .request(
            "POST",
            "/api/admin/devices",
            Some("admin"),
            device_input(Some("physical_001"), "Duplicate mapping"),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    context.cleanup().await;
}

#[tokio::test]
async fn unmapped_and_ambiguous_hardware_identities_fail_closed() {
    let context = TestContext::new().await;
    sqlx::query("UPDATE devices SET hardware_id=NULL WHERE id='dev_A'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert!(
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .is_err()
    );

    sqlx::query("UPDATE devices SET hardware_id='dev_A' WHERE id='dev_B'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        ecg_backend::api::authorization::resolve_device_id(&context.state.pool, "dev_A")
            .await
            .unwrap_err()
            .0,
        StatusCode::CONFLICT
    );
    context.cleanup().await;
}
