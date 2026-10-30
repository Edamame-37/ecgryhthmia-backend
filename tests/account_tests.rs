mod support;
use axum::{body::Body, http::Request};
use serde_json::json;
use support::*;
use tower::ServiceExt;
#[tokio::test]
async fn provisioning_creates_parent_account_first_and_retry_is_idempotent() {
    let context = TestContext::new().await;
    let profile = json!({"role":"pasien","email":"auth-new@test.invalid","first_name":"Synthetic","last_name":"Patient","date_of_birth":"2000-01-01","gender":"L"});
    let response = context
        .request(
            "POST",
            "/api/auth/register_profile",
            Some("new"),
            profile.clone(),
        )
        .await;
    assert_eq!(response.status().as_u16(), 200);
    let first = json_response(response).await["account_id"]
        .as_str()
        .unwrap()
        .to_string();
    let retry = context
        .request("POST", "/api/auth/register_profile", Some("new"), profile)
        .await;
    assert_eq!(retry.status().as_u16(), 200);
    assert_eq!(json_response(retry).await["account_id"], first);
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM patients p JOIN accounts a ON a.id=p.account_id WHERE a.auth_id='auth-new'").fetch_one(&context.state.pool).await.unwrap();
    assert_eq!(count, 1);
    assert_eq!(context.request("POST","/api/auth/register_profile",Some("new"),json!({"role":"admin","email":"auth-new@test.invalid","first_name":"Escalate","last_name":""})).await.status().as_u16(),400);
    assert_eq!(context.request("POST","/api/auth/register_profile",Some("new"),json!({"role":"pasien","email":"different@test.invalid","first_name":"Mismatch","last_name":""})).await.status().as_u16(),403);
    context.cleanup().await;
}
#[tokio::test]
async fn impersonation_is_actor_bound_scoped_audited_and_revocable() {
    let context = TestContext::new().await;
    let response = context
        .request(
            "POST",
            "/api/admin/impersonate/acc_patient",
            Some("admin"),
            json!({}),
        )
        .await;
    assert_eq!(response.status().as_u16(), 200);
    let id = json_response(response).await["impersonation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let make_request = |path: &str, user: &str| {
        Request::builder()
            .uri(path)
            .header("Authorization", format!("Bearer {}", token(user)))
            .header("X-Impersonation-Id", &id)
            .body(Body::empty())
            .unwrap()
    };
    let me = json_response(
        context
            .app
            .clone()
            .oneshot(make_request("/api/auth/me", "admin"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(me["actor_id"], "acc_admin");
    assert_eq!(me["account_id"], "acc_patient");
    assert_eq!(me["role"], "pasien");
    assert_eq!(
        context
            .app
            .clone()
            .oneshot(make_request("/api/admin/stats", "admin"))
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        context
            .app
            .clone()
            .oneshot(make_request("/api/auth/me", "other"))
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        context
            .request(
                "POST",
                "/api/doctors/impersonate/acc_other",
                Some("doctor"),
                json!({})
            )
            .await
            .status()
            .as_u16(),
        403
    );
    assert_eq!(
        context
            .request(
                "DELETE",
                &format!("/api/impersonation/{id}"),
                Some("admin"),
                json!({})
            )
            .await
            .status()
            .as_u16(),
        200
    );
    assert_eq!(
        context
            .app
            .clone()
            .oneshot(make_request("/api/auth/me", "admin"))
            .await
            .unwrap()
            .status()
            .as_u16(),
        403
    );
    let audit:i64=sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE actor_id='acc_admin' AND action='POST' AND resource_id='/api/admin/impersonate/acc_patient'").fetch_one(&context.state.pool).await.unwrap();
    assert_eq!(audit, 1);
    context.cleanup().await;
}
#[tokio::test]
async fn account_token_cutoff_and_rate_limit_are_enforced() {
    let context = TestContext::new().await;
    sqlx::query("UPDATE accounts SET tokens_valid_after=extract(epoch FROM now())::bigint+10 WHERE id='acc_patient'").execute(&context.state.pool).await.unwrap();
    assert_eq!(
        context
            .request("GET", "/api/auth/me", Some("patient"), json!({}))
            .await
            .status()
            .as_u16(),
        401
    );
    let limits = ecg_backend::api::limits::RateLimits::default();
    limits.check("test-actor", 2).await.unwrap();
    limits.check("test-actor", 2).await.unwrap();
    assert_eq!(
        limits.check("test-actor", 2).await.unwrap_err().0.as_u16(),
        429
    );
    context.cleanup().await;
}
