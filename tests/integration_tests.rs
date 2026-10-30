mod support;
use axum::http::StatusCode;
use serde_json::json;
use support::*;

#[tokio::test]
async fn identity_and_resource_matrix() {
    let context = TestContext::new().await;
    let me = context
        .request("GET", "/api/auth/me", Some("patient"), json!({}))
        .await;
    assert_eq!(me.status(), StatusCode::OK);
    let me = json_response(me).await;
    assert_eq!(me["auth_id"], "auth-patient");
    assert_eq!(me["account_id"], "acc_patient");
    assert_eq!(me["profile_id"], "pat_A");
    for (user, expected) in [
        (None, 401),
        (Some("other"), 403),
        (Some("other-doc"), 403),
        (Some("patient"), 200),
        (Some("doctor"), 200),
        (Some("admin"), 200),
    ] {
        let response = context
            .request("GET", "/api/patients/pat_A", user, json!({}))
            .await;
        assert_eq!(response.status().as_u16(), expected, "user={user:?}");
    }
    let response = context
        .request("GET", "/api/admin/users", Some("patient"), json!({}))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let sessions = json_response(
        context
            .request("GET", "/api/sessions", Some("patient"), json!({}))
            .await,
    )
    .await;
    assert_eq!(sessions["pagination"]["total"], 1);
    assert_eq!(sessions["data"][0]["patient_id"], "pat_A");
    context.cleanup().await;
}
#[tokio::test]
async fn worker_persists_sequence_and_rejects_replay() {
    let context = TestContext::new().await;
    let frame =
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .unwrap();
    assert_eq!(frame.frame_number, 1);
    assert_eq!(frame.start_time, 0.0);
    assert_eq!(frame.end_time, 0.2);
    assert!(
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .is_err()
    );
    let restarted = ecg_backend::api::routes::AppState::new(
        context.state.pool.clone(),
        (*context.state.config).clone(),
    );
    let second = ecg_backend::services::ingestion::persist(&restarted, "dev_A", payload(2), true)
        .await
        .unwrap();
    assert_eq!(second.frame_number, 2);
    assert_eq!(second.start_time, 0.2);
    let records = json_response(
        context
            .request("GET", "/api/records/ses_pat_A", Some("patient"), json!({}))
            .await,
    )
    .await;
    assert_eq!(records.as_array().unwrap().len(), 2);
    assert_eq!(records[1]["start_time"], 0.2);
    context.cleanup().await;
}
#[tokio::test]
async fn unknown_roles_and_revoked_accounts_fail_closed() {
    let context = TestContext::new().await;
    sqlx::query("UPDATE accounts SET role='unknown' WHERE id='acc_patient'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        context
            .request("GET", "/api/sessions", Some("patient"), json!({}))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE accounts SET role='pasien',enabled=false WHERE id='acc_patient'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        context
            .request("GET", "/api/auth/me", Some("patient"), json!({}))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    context.cleanup().await;
}
#[test]
fn database_guard_rejects_unknown_targets_before_connecting() {
    assert!(guard_database("postgres://x:y@production.invalid/ecg_test").is_err());
    assert!(guard_database("postgres://x:y@localhost/postgres").is_err());
    assert!(guard_database("postgres://x:y@localhost/ecg_test").is_ok());
}
