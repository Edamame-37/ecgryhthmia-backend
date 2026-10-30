mod support;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use support::*;
use tower::ServiceExt;

const PRIVATE_IMAGE: &[u8] = b"\x89PNG\r\n\x1a\nsynthetic-private-image";

async fn private_image_fixture(context: &TestContext, category: &str) -> String {
    let directory = context.state.config.uploads_root.join(category);
    tokio::fs::create_dir_all(&directory).await.unwrap();
    tokio::fs::write(directory.join("private_image.png"), PRIVATE_IMAGE)
        .await
        .unwrap();
    format!("/uploads/{category}/private_image.png")
}

async fn set_image_reference(
    context: &TestContext,
    category: &str,
    owner: &str,
    reference: Option<&str>,
) {
    let statement = match category {
        "profiles" => "UPDATE accounts SET profile_photo=$1 WHERE id=$2",
        "ecg_papers" => "UPDATE sessions SET ecg_paper=$1 WHERE id=$2",
        _ => panic!("Unsupported private image fixture category"),
    };
    sqlx::query(statement)
        .bind(reference)
        .bind(owner)
        .execute(&context.state.pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn private_routes_deny_anonymous_and_cross_patient_access() {
    let context = TestContext::new().await;
    for path in [
        "/api/auth/me",
        "/api/patients",
        "/api/devices",
        "/api/sessions",
        "/api/sessions/ses_pat_A/frames",
        "/api/records/ses_pat_A",
        "/api/records/ses_pat_A/download",
        "/api/patients/pat_A/recording-status",
        "/uploads/profiles/unknown.png",
    ] {
        assert_eq!(
            context.request("GET", path, None, json!({})).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
    for path in [
        "/api/patients/pat_B",
        "/api/sessions/ses_pat_B",
        "/api/sessions/ses_pat_B/frames",
        "/api/records/ses_pat_B",
        "/api/records/ses_pat_B/download",
    ] {
        assert_eq!(
            context
                .request("GET", path, Some("patient"), json!({}))
                .await
                .status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    for (method, path) in [
        ("PUT", "/api/patients/pat_B"),
        ("DELETE", "/api/sessions/ses_pat_B"),
        ("POST", "/api/devices/dev_B/command"),
    ] {
        let response = context
            .request(method, path, Some("patient"), json!({}))
            .await;
        assert!(!response.status().is_success(), "{method} {path}");
    }
    assert_eq!(
        context
            .request(
                "POST",
                "/api/auth/login",
                None,
                json!({"password":"$2b$literal-hash"})
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    context.cleanup().await;
}

#[tokio::test]
async fn private_images_accept_canonical_and_legacy_references_with_the_same_access_scope() {
    for (category, owner) in [("profiles", "acc_patient"), ("ecg_papers", "ses_pat_A")] {
        let denied_status = if category == "profiles" {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::FORBIDDEN
        };
        for origin in [
            "",
            "http://legacy.ecg.invalid:8080",
            "https://legacy.ecg.invalid",
        ] {
            let context = TestContext::new().await;
            let path = private_image_fixture(&context, category).await;
            let reference = format!("{origin}{path}");
            set_image_reference(&context, category, owner, Some(&reference)).await;
            for (user, expected) in [
                (Some("patient"), StatusCode::OK),
                (Some("doctor"), StatusCode::OK),
                (Some("admin"), StatusCode::OK),
                (Some("other"), denied_status),
                (Some("other-doc"), denied_status),
                (None, StatusCode::UNAUTHORIZED),
            ] {
                let response = context.request("GET", &path, user, json!({})).await;
                assert_eq!(
                    response.status(),
                    expected,
                    "category={category}, reference={reference}, user={user:?}"
                );
                if expected == StatusCode::OK {
                    assert_eq!(response.headers()["content-type"], "image/png");
                    assert_eq!(response.headers()["cache-control"], "no-store");
                    let bytes = axum::body::to_bytes(response.into_body(), 1024)
                        .await
                        .unwrap();
                    assert_eq!(bytes.as_ref(), PRIVATE_IMAGE);
                }
            }
            context.cleanup().await;
        }
    }
}

#[tokio::test]
async fn private_images_require_an_exact_supported_reference_before_reading_a_file() {
    for (category, owner) in [("profiles", "acc_patient"), ("ecg_papers", "ses_pat_A")] {
        let context = TestContext::new().await;
        let path = private_image_fixture(&context, category).await;
        assert_eq!(
            context
                .request("GET", &path, Some("admin"), json!({}))
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "An unreferenced local file must remain private: {path}"
        );
        set_image_reference(&context, category, owner, Some(&path)).await;
        assert_eq!(
            context
                .request(
                    "GET",
                    &format!("/uploads/{category}/unknown.png"),
                    Some("admin"),
                    json!({})
                )
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            context
                .request(
                    "GET",
                    &format!("/uploads/{category}/%2e%2e%5coutside.png"),
                    Some("admin"),
                    json!({})
                )
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        context.cleanup().await;

        for (prefix, suffix) in [
            ("ftp://legacy.ecg.invalid", ""),
            ("//legacy.ecg.invalid", ""),
            ("https://legacy.ecg.invalid/prefix", ""),
            ("https://legacy.ecg.invalid", ".backup"),
            ("https://legacy.ecg.invalid", "?token=stale"),
            ("https://legacy.ecg.invalid", "#fragment"),
        ] {
            let context = TestContext::new().await;
            let path = private_image_fixture(&context, category).await;
            let reference = format!("{prefix}{path}{suffix}");
            set_image_reference(&context, category, owner, Some(&reference)).await;
            assert_eq!(
                context
                    .request("GET", &path, Some("admin"), json!({}))
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "Unsupported reference must not match by filename: {reference}"
            );
            context.cleanup().await;
        }
    }
}

#[tokio::test]
async fn ambiguous_private_image_references_fail_closed_and_deleted_papers_are_ignored() {
    let context = TestContext::new().await;
    for (category, first_owner, second_owner) in [
        ("profiles", "acc_patient", "acc_other"),
        ("ecg_papers", "ses_pat_A", "ses_pat_B"),
    ] {
        let path = private_image_fixture(&context, category).await;
        let legacy_reference = format!("https://legacy.ecg.invalid{path}");
        set_image_reference(&context, category, first_owner, Some(&path)).await;
        set_image_reference(&context, category, second_owner, Some(&legacy_reference)).await;
        for user in ["patient", "other", "doctor", "other-doc", "admin"] {
            assert_eq!(
                context
                    .request("GET", &path, Some(user), json!({}))
                    .await
                    .status(),
                StatusCode::CONFLICT,
                "Ambiguous {category} reference must fail closed for user={user}"
            );
        }
        if category == "profiles" {
            set_image_reference(&context, category, second_owner, None).await;
        } else {
            sqlx::query("UPDATE sessions SET deleted_at=now() WHERE id=$1")
                .bind(second_owner)
                .execute(&context.state.pool)
                .await
                .unwrap();
        }
        assert_eq!(
            context
                .request("GET", &path, Some("patient"), json!({}))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            context
                .request("GET", &path, Some("other"), json!({}))
                .await
                .status(),
            if category == "profiles" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::FORBIDDEN
            }
        );
    }
    sqlx::query("UPDATE sessions SET deleted_at=now() WHERE id='ses_pat_A'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        context
            .request(
                "GET",
                "/uploads/ecg_papers/private_image.png",
                Some("admin"),
                json!({})
            )
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    context.cleanup().await;
}

#[tokio::test]
async fn signed_user_metadata_never_grants_admin() {
    let context = TestContext::new().await;
    let mut claims = serde_json::to_value(claims("patient")).unwrap();
    claims["user_metadata"] = json!({"role":"admin"});
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap();
    let request = Request::builder()
        .uri("/api/admin/users")
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        context.app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    context.cleanup().await;
}
#[tokio::test]
async fn pairing_code_is_scoped_expiring_and_single_use() {
    let context = TestContext::new().await;
    let response = context
        .request(
            "POST",
            "/api/patients/pat_A/pairing-code",
            Some("patient"),
            json!({}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let code = json_response(response).await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = json!({"doctor_id":"doc_B","code":code});
    assert_eq!(
        context
            .request(
                "POST",
                "/api/patients/pat_A/connect",
                Some("other-doc"),
                body.clone()
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        context
            .request(
                "POST",
                "/api/patients/pat_A/connect",
                Some("other-doc"),
                body
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        context
            .request("GET", "/api/patients/pat_A", Some("doctor"), json!({}))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let code = json_response(
        context
            .request(
                "POST",
                "/api/patients/pat_A/pairing-code",
                Some("patient"),
                json!({}),
            )
            .await,
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE pairing_codes SET expires_at=now()-interval '1 second' WHERE digest=$1")
        .bind(ecg_backend::models::payload::digest(code.as_bytes()))
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        context
            .request(
                "POST",
                "/api/patients/pat_A/connect",
                Some("doctor"),
                json!({"doctor_id":"doc_A","code":code})
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    context.cleanup().await;
}
#[tokio::test]
async fn hidden_deleted_frames_do_not_reappear_in_records_or_export() {
    let context = TestContext::new().await;
    let first =
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .unwrap();
    ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(2), true)
        .await
        .unwrap();
    assert_eq!(
        context
            .request(
                "PUT",
                &format!("/api/frames/{}", first.id),
                Some("patient"),
                json!({"hidden":true})
            )
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        context
            .request(
                "PUT",
                &format!("/api/frames/{}", first.id),
                Some("admin"),
                json!({"hidden":true})
            )
            .await
            .status(),
        StatusCode::OK
    );
    let records = json_response(
        context
            .request("GET", "/api/records/ses_pat_A", Some("patient"), json!({}))
            .await,
    )
    .await;
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["start_time"], 0.2);
    assert_eq!(records[0]["frame_number"], 2);
    let metadata = json_response(
        context
            .request(
                "GET",
                "/api/sessions/ses_pat_A/frames",
                Some("patient"),
                json!({}),
            )
            .await,
    )
    .await;
    assert_eq!(metadata["pagination"]["total"], 1);
    let exported = json_response(
        context
            .request(
                "GET",
                "/api/records/ses_pat_A/download",
                Some("patient"),
                json!({}),
            )
            .await,
    )
    .await;
    assert_eq!(exported.as_array().unwrap().len(), 1);
    assert_eq!(exported[0]["frame_number"], 2);
    assert_eq!(
        context
            .request(
                "DELETE",
                &format!("/api/frames/{}", first.id),
                Some("admin"),
                json!({})
            )
            .await
            .status(),
        StatusCode::OK
    );
    assert!(!std::path::Path::new(
        &context
            .state
            .config
            .records_root
            .join("ses_pat_A")
            .join(format!("{}.jsonl", first.id))
    )
    .exists());
    context.cleanup().await;
}
#[tokio::test]
async fn private_tables_have_rls_and_no_browser_grants() {
    let context = TestContext::new().await;
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname=$1 AND tablename IN ('accounts','patients','doctors','devices','sessions','frame_records','device_credentials') AND rowsecurity").bind(&context.schema).fetch_one(&context.state.pool).await.unwrap();
    assert_eq!(count, 7);
    for role in ["anon", "authenticated"] {
        let grants:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.role_table_grants WHERE table_schema=$1 AND grantee=$2").bind(&context.schema).bind(role).fetch_one(&context.state.pool).await.unwrap();
        assert_eq!(grants, 0);
    }
    context.cleanup().await;
}
#[tokio::test]
async fn traversal_and_corrupt_storage_fail_closed() {
    let context = TestContext::new().await;
    let outside = context
        .state
        .config
        .records_root
        .parent()
        .unwrap()
        .join("outside.jsonl");
    tokio::fs::write(&outside, b"marker").await.unwrap();
    assert!(ecg_backend::api::recordings::confined_path(
        &context.state.config.records_root,
        &outside
    )
    .await
    .is_err());
    for id in ["../outside", "a/b", "C:\\secret", "%2e%2e", ""] {
        assert!(ecg_backend::api::authorization::validate_id(id).is_err());
    }
    let frame =
        ecg_backend::services::ingestion::persist(&context.state, "dev_A", payload(1), true)
            .await
            .unwrap();
    let path = context
        .state
        .config
        .records_root
        .join("ses_pat_A")
        .join(format!("{}.jsonl", frame.id));
    tokio::fs::write(path, b"modified").await.unwrap();
    assert_eq!(
        context
            .request("GET", "/api/records/ses_pat_A", Some("patient"), json!({}))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    context.cleanup().await;
}
