use std::{collections::HashMap, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use ecg_backend::{
    api::routes::{create_router, AppMetadata, AppState, AuthResponse, Claims},
    db::postgres,
    models::device::{DeviceEcg, DevicePayload, DevicePrediction, DeviceValidation},
};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::sync::mpsc::unbounded_channel;
use tower::ServiceExt;
use uuid::Uuid;

const JWT_SECRET: &str = "test_jwt_secret_key_extremely_long_and_secure";

fn setup_test_state(pool: PgPool) -> AppState {
    let (pacer_tx, _) = unbounded_channel();
    let (db_tx, _) = unbounded_channel();
    AppState {
        pool,
        mqtt_clients: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        pacer_tx,
        db_tx,
        jwt_secret: JWT_SECRET.to_string(),
        api_url: "http://127.0.0.1:8081".to_string(),
    }
}

fn disconnected_pool() -> PgPool {
    // Routes using token metadata must not need a live database.
    PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(250))
        .connect_lazy("postgres://test:test@127.0.0.1:1/ecg_test")
        .unwrap()
}

fn token_for(user_id: &str, role: &str) -> String {
    encode(
        &Header::default(),
        &Claims {
            sub: user_id.to_string(),
            app_metadata: Some(AppMetadata {
                role: Some(role.to_string()),
            }),
            exp: (chrono::Utc::now().timestamp() + 3600) as usize,
        },
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .unwrap()
}

fn json_request(uri: &str, token: Option<&str>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    request.body(Body::from(body.to_string())).unwrap()
}

async fn response_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 10)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn device_payload(sample_count: usize) -> DevicePayload {
    DevicePayload {
        message_id: "msg_001".to_string(),
        device_id: "device01".to_string(),
        session_id: "device_session".to_string(),
        frame_id: "000001".to_string(),
        created_at: "2026-08-10T10:00:00+07:00".to_string(),
        sampling_rate_hz: 250.0,
        duration_s: sample_count as f64 / 250.0,
        validation: DeviceValidation {
            status: "PASS".to_string(),
            warnings: vec![],
        },
        ecg: DeviceEcg {
            format: "samples_by_time".to_string(),
            samples: vec![vec![0.5, 0.6, 0.7]; sample_count],
        },
        prediction: DevicePrediction {
            status: "PASS".to_string(),
            label: "Non Arrhythmia".to_string(),
            confidence_percent: 99.5,
            probabilities: None,
            threshold: None,
            latency_ms: None,
            runtime: None,
        },
        system: None,
        stress_test: None,
        network: None,
    }
}

#[tokio::test]
async fn test_api_auth_me() {
    let app = create_router(setup_test_state(disconnected_pool()));
    let token = token_for("patient-test", "pasien");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header("Authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let auth: AuthResponse = serde_json::from_value(response_json(response).await).unwrap();
    assert!(auth.success);
    assert_eq!(auth.user_id.as_deref(), Some("patient-test"));
    assert_eq!(auth.role.as_deref(), Some("pasien"));
    // Supabase handles login and token issuance; /auth/me only reads claims.
    assert!(auth.token.is_none());
}

#[tokio::test]
async fn test_api_auth_me_rejects_missing_or_malformed_authorization() {
    let app = create_router(setup_test_state(disconnected_pool()));
    for authorization in [None, Some("Basic invalid"), Some("Bearer invalid")] {
        let mut request = Request::builder().uri("/api/auth/me");
        if let Some(authorization) = authorization {
            request = request.header("Authorization", authorization);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn test_profile_registration_requires_authentication() {
    let app = create_router(setup_test_state(disconnected_pool()));
    let response = app
        .oneshot(json_request(
            "/api/auth/register_profile",
            None,
            json!({
                "role": "pasien", "email": "patient@test.invalid",
                "first_name": "John", "last_name": "Doe", "age": 31, "gender": "L"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_admin_registration_rejects_patient_token() {
    let app = create_router(setup_test_state(disconnected_pool()));
    let token = token_for("patient-test", "pasien");
    let response = app
        .oneshot(json_request(
            "/api/auth/register",
            Some(&token),
            json!({
                "email": "new-user@test.invalid", "password": "password123",
                "role": "pasien", "first_name": "John", "last_name": "Doe", "age": 31
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_doctor_impersonate_rejects_patient_token() {
    let app = create_router(setup_test_state(disconnected_pool()));
    let token = token_for("patient-test", "pasien");
    let response = app
        .oneshot(json_request(
            "/api/doctors/impersonate/other-patient",
            Some(&token),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_pacer_streaming() {
    let clients = ecg_backend::network::websocket::ClientList::default();
    let (ws_tx, mut ws_rx) = unbounded_channel::<String>();
    clients.lock().unwrap().push(ws_tx);
    let pacer_tx = ecg_backend::network::pacer::start_pacer(clients);
    pacer_tx.send(device_payload(50)).unwrap();

    for chunk_index in 0..2 {
        let message = tokio::time::timeout(Duration::from_secs(2), ws_rx.recv())
            .await
            .expect("Pacer did not broadcast a chunk")
            .unwrap();
        let parsed: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(parsed["type"], "live_data");
        assert_eq!(parsed["measurement_id"], "msg_001");
        assert_eq!(parsed["session_id"], "device_session");
        let raw = &parsed["data_payload"]["raw"];
        for channel in ["time", "ch1", "ch2", "ch3"] {
            assert_eq!(raw[channel].as_array().unwrap().len(), 25);
        }
        assert_eq!(raw["time"][0], chunk_index as f64 * 0.1);
        assert_eq!(raw["ch1"][0], 0.5);
    }
}

// Persistence tests explicitly require TEST_DATABASE_URL, never the app database.
// Every test gets a unique schema so its migrations and fixtures remain isolated.
struct TestDatabase {
    pool: PgPool,
    admin_pool: PgPool,
    schema: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("Set TEST_DATABASE_URL to a dedicated PostgreSQL test database");
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&url)
            .await
            .expect("Cannot connect to TEST_DATABASE_URL");
        let schema = format!("ecg_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin_pool)
            .await
            .unwrap();

        let search_path = schema.clone();
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(5))
            .after_connect(move |connection, _| {
                let search_path = search_path.clone();
                Box::pin(async move {
                    sqlx::query("SELECT set_config('search_path', $1, false)")
                        .bind(search_path)
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        postgres::run_migrations(&pool).await.unwrap();
        Self {
            pool,
            admin_pool,
            schema,
        }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin_pool)
            .await
            .unwrap();
        self.admin_pool.close().await;
    }
}

async fn register_profile(app: &Router, user_id: &str, role: &str) {
    let token = token_for(user_id, role);
    let response = app
        .clone()
        .oneshot(json_request(
            "/api/auth/register_profile",
            Some(&token),
            json!({
                "role": role, "email": format!("{user_id}@test.invalid"),
                "first_name": "Test", "last_name": role, "age": 31, "gender": "L"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["success"], true);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL; see tests/README.md"]
async fn test_api_register_profile() {
    let db = TestDatabase::new().await;
    let app = create_router(setup_test_state(db.pool.clone()));
    for (user_id, role, table) in [
        ("patient-test", "pasien", "patients"),
        ("doctor-test", "dokter", "doctors"),
    ] {
        register_profile(&app, user_id, role).await;
        let saved_role: String = sqlx::query_scalar("SELECT role FROM accounts WHERE id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(saved_role, role);
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS(SELECT 1 FROM {table} WHERE account_id = $1)"
        ))
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert!(exists);
    }
    drop(app);
    db.cleanup().await;
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL; see tests/README.md"]
async fn test_db_worker_session_writing() {
    let db = TestDatabase::new().await;
    let app = create_router(setup_test_state(db.pool.clone()));
    register_profile(&app, "patient-test", "pasien").await;
    sqlx::query("INSERT INTO devices (id, name) VALUES ('dev_001', 'device01')")
        .execute(&db.pool)
        .await
        .unwrap();

    // START now creates an active session before the worker accepts payloads.
    let session_id = format!("ses_test_{}", Uuid::new_v4().simple());
    let file_path = format!("records/{session_id}.jsonl");
    sqlx::query(
        "INSERT INTO sessions (id, device_id, patient_id, started_at, file_path)
         VALUES ($1, 'dev_001', 'patient-test', NOW(), $2)",
    )
    .bind(&session_id)
    .bind(&file_path)
    .execute(&db.pool)
    .await
    .unwrap();

    let (pacer_tx, mut pacer_rx) = unbounded_channel();
    let db_tx = postgres::start_db_worker(db.pool.clone(), pacer_tx);
    db_tx.send(device_payload(50)).unwrap();
    // Forwarding follows persistence; wait for completion instead of sleeping.
    let forwarded = tokio::time::timeout(Duration::from_secs(5), pacer_rx.recv())
        .await
        .expect("Database worker did not finish writing the frame")
        .unwrap();
    assert_eq!(forwarded.session_id, session_id);
    assert_eq!(forwarded.frame_id, "000001");

    let contents = std::fs::read_to_string(&file_path).unwrap();
    let saved: DevicePayload = serde_json::from_str(contents.trim()).unwrap();
    assert_eq!(saved.message_id, forwarded.message_id);
    assert_eq!(saved.ecg.samples.len(), 50);
    let frame: (String, f64, f64) = sqlx::query_as(
        "SELECT label, start_time, end_time FROM frame_records WHERE session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(frame, ("Non Arrhythmia".to_string(), 0.0, 0.2));

    drop(db_tx);
    drop(app);
    db.cleanup().await;
    std::fs::remove_file(file_path).unwrap();
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL; see tests/README.md"]
async fn test_doctor_impersonate() {
    let db = TestDatabase::new().await;
    let app = create_router(setup_test_state(db.pool.clone()));
    register_profile(&app, "doctor-test", "dokter").await;
    register_profile(&app, "patient-test", "pasien").await;
    let token = token_for("doctor-test", "dokter");

    // Only patients assigned to the doctor may be impersonated.
    let response = app
        .clone()
        .oneshot(json_request(
            "/api/doctors/impersonate/patient-test",
            Some(&token),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    sqlx::query("UPDATE patients SET primary_doctor_id = 'doctor-test' WHERE id = 'patient-test'")
        .execute(&db.pool)
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            "/api/doctors/impersonate/patient-test",
            Some(&token),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let result = response_json(response).await;
    assert_eq!(result["success"], true);
    assert_eq!(result["user_id"], "patient-test");
    assert_eq!(result["role"], "pasien");
    assert!(result.get("token").is_none());

    drop(app);
    db.cleanup().await;
}
