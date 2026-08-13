use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use tower::ServiceExt; // for oneshot
use std::collections::HashMap;
use ecg_backend::api::routes::{AppState, RegisterRequest, LoginRequest, AuthResponse};
use ecg_backend::models::device::{DevicePayload, DeviceValidation, DeviceEcg, DevicePrediction};
use ecg_backend::db::sqlite;

fn setup_test_state() -> (AppState, tokio::sync::mpsc::UnboundedReceiver<DevicePayload>, tokio::sync::mpsc::UnboundedReceiver<DevicePayload>) {
    // Gunakan shared in-memory SQLite DB cache agar pool connection mengakses DB yang sama
    let db_path = "file::memory_db?mode=memory&cache=shared";
    let sqlite_key = "test_secure_key_123";
    let pool = sqlite::create_pool(db_path, sqlite_key);

    // Jalankan migrasi
    {
        let conn = pool.get().unwrap();
        sqlite::run_migrations(&conn, "admin@test.com", "adminpassword").unwrap();
    }

    let (pacer_tx, pacer_rx) = tokio::sync::mpsc::unbounded_channel();
    let (db_tx, db_rx) = tokio::sync::mpsc::unbounded_channel();
    let mqtt_clients = std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new()));

    let state = AppState {
        pool,
        mqtt_clients,
        pacer_tx,
        db_tx,
        jwt_secret: "test_jwt_secret_key_extremely_long_and_secure".to_string(),
        api_url: "http://127.0.0.1:8081".to_string(),
    };

    (state, pacer_rx, db_rx)
}

#[tokio::test]
async fn test_api_register_and_login() {
    let (state, _pacer_rx, _db_rx) = setup_test_state();
    let app = ecg_backend::api::routes::create_router(state);

    // 1. Uji Register Pasien
    let reg_req = RegisterRequest {
        role: "pasien".to_string(),
        email: "pasien@test.com".to_string(),
        password: "password123".to_string(),
        first_name: "John".to_string(),
        last_name: "Doe".to_string(),
        date_of_birth: Some("1995-05-15".to_string()),
        gender: Some("L".to_string()),
    };
    let req_body = serde_json::to_vec(&reg_req).unwrap();

    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/register")
                .header("Content-Type", "application/json")
                .body(Body::from(req_body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 10).await.unwrap();
    let reg_res: AuthResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert!(reg_res.success);

    // 2. Uji Login Pasien
    let login_req = LoginRequest {
        email: "pasien@test.com".to_string(),
        password: "password123".to_string(),
        role: Some("pasien".to_string()),
    };
    let login_body = serde_json::to_vec(&login_req).unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header("Content-Type", "application/json")
                .body(Body::from(login_body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = axum::body::to_bytes(response.into_body(), 1024 * 10).await.unwrap();
    let login_res: AuthResponse = serde_json::from_slice(&body_bytes).unwrap();
    assert!(login_res.success);
    assert_eq!(login_res.role.unwrap(), "pasien");
    assert!(login_res.token.is_some());
}

#[tokio::test]
async fn test_db_worker_session_writing() {
    let db_path = "file::memory_worker_db?mode=memory&cache=shared";
    let sqlite_key = "worker_secure_key_123";
    let pool = sqlite::create_pool(db_path, sqlite_key);

    // Jalankan migrasi
    {
        let conn = pool.get().unwrap();
        sqlite::run_migrations(&conn, "admin@test.com", "adminpassword").unwrap();
        
        // Daftarkan perangkat agar relasi asing (foreign key) ke devices valid
        conn.execute("INSERT OR IGNORE INTO devices (id, name) VALUES ('dev_001', 'device01')", []).unwrap();
    }

    // Jalankan Db Worker asinkron
    let (pacer_tx, _) = tokio::sync::mpsc::unbounded_channel();
    let db_tx = sqlite::start_db_worker(pool.clone(), pacer_tx);

    // Kirim Payload dummy
    let payload = DevicePayload {
        message_id: "msg_001".to_string(),
        device_id: "device01".to_string(),
        session_id: "session_integration_test".to_string(),
        frame_id: "frame_001".to_string(),
        created_at: "2026-08-10T10:00:00+07:00".to_string(),
        sampling_rate_hz: 250.0,
        duration_s: 1.0,
        validation: DeviceValidation {
            status: "PASS".to_string(),
            warnings: vec![],
        },
        ecg: DeviceEcg {
            format: "samples_by_time".to_string(),
            samples: vec![vec![0.1, 0.2, 0.3]],
        },
        prediction: DevicePrediction {
            status: "PASS".to_string(),
            label: "Normal".to_string(),
            confidence_percent: 99.5,
            probabilities: None,
            threshold: None,
            latency_ms: None,
            runtime: None,
        },
        system: None,
        stress_test: None,
        network: None,
    };

    db_tx.send(payload).unwrap();

    // Tunggu worker menulis ke DB & File
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verifikasi sesi tersimpan di DB
    let conn = pool.get().unwrap();
    let session_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE device_id = 'dev_001')",
        [],
        |row| row.get(0)
    ).unwrap();
    assert!(session_exists);

    // Verifikasi berkas .jsonl dibuat
    let session_id_in_db: String = conn.query_row(
        "SELECT id FROM sessions WHERE device_id = 'dev_001' LIMIT 1",
        [],
        |row| row.get(0)
    ).unwrap();
    let expected_file_path = format!("records/{}.jsonl", session_id_in_db);
    let path = std::path::Path::new(&expected_file_path);
    assert!(path.exists());

    // Bersihkan berkas dan folder pengujian
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir("records");
}

#[tokio::test]
async fn test_pacer_streaming() {
    let clients = ecg_backend::network::websocket::ClientList::default();
    let (ws_tx, mut ws_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    
    // Daftarkan channel penerima ws broadcast
    {
        let mut lock = clients.lock().unwrap();
        lock.push(ws_tx);
    }

    let pacer_tx = ecg_backend::network::pacer::start_pacer(clients);

    // Mengirim 50 data ekg sampel dengan sampling rate 250Hz.
    // Ukuran chunk: fs * 0.1 = 25 sampel.
    // Berarti 50 sampel dipecah menjadi 2 chunk berdurasi @100ms.
    let payload = DevicePayload {
        message_id: "pacer_msg_001".to_string(),
        device_id: "device01".to_string(),
        session_id: "session_pacer".to_string(),
        frame_id: "frame_001".to_string(),
        created_at: "2026-08-10T10:00:00+07:00".to_string(),
        sampling_rate_hz: 250.0,
        duration_s: 0.2,
        validation: DeviceValidation {
            status: "PASS".to_string(),
            warnings: vec![],
        },
        ecg: DeviceEcg {
            format: "samples_by_time".to_string(),
            samples: vec![vec![0.5, 0.6, 0.7]; 50],
        },
        prediction: DevicePrediction {
            status: "PASS".to_string(),
            label: "Normal".to_string(),
            confidence_percent: 99.5,
            probabilities: None,
            threshold: None,
            latency_ms: None,
            runtime: None,
        },
        system: None,
        stress_test: None,
        network: None,
    };

    pacer_tx.send(payload).unwrap();

    // Terima chunk ke-1
    let msg1 = tokio::time::timeout(std::time::Duration::from_millis(500), ws_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let parsed1: serde_json::Value = serde_json::from_str(&msg1).unwrap();
    assert_eq!(parsed1["type"], "live_data");
    assert_eq!(parsed1["measurement_id"], "pacer_msg_001");
    let raw_data1 = &parsed1["data_payload"]["raw"];
    assert_eq!(raw_data1["ch1"].as_array().unwrap().len(), 25);

    // Terima chunk ke-2
    let msg2 = tokio::time::timeout(std::time::Duration::from_millis(500), ws_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let parsed2: serde_json::Value = serde_json::from_str(&msg2).unwrap();
    let raw_data2 = &parsed2["data_payload"]["raw"];
    assert_eq!(raw_data2["ch1"].as_array().unwrap().len(), 25);
}

#[tokio::test]
async fn test_doctor_impersonate() {
    let (state, _pacer_rx, _db_rx) = setup_test_state();
    let app = ecg_backend::api::routes::create_router(state);

    // 1. Register Dokter
    let doc_reg = RegisterRequest {
        role: "dokter".to_string(),
        email: "doctor@test.com".to_string(),
        password: "password123".to_string(),
        first_name: "Dr. House".to_string(),
        last_name: "MD".to_string(),
        date_of_birth: None,
        gender: None,
    };
    let doc_req_body = serde_json::to_vec(&doc_reg).unwrap();
    let _ = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header("Content-Type", "application/json")
            .body(Body::from(doc_req_body))
            .unwrap(),
    ).await.unwrap();

    // 2. Register Pasien
    let pat_reg = RegisterRequest {
        role: "pasien".to_string(),
        email: "patient@test.com".to_string(),
        password: "password123".to_string(),
        first_name: "John".to_string(),
        last_name: "Doe".to_string(),
        date_of_birth: Some("1995-05-15".to_string()),
        gender: Some("L".to_string()),
    };
    let pat_req_body = serde_json::to_vec(&pat_reg).unwrap();
    let _ = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/auth/register")
            .header("Content-Type", "application/json")
            .body(Body::from(pat_req_body))
            .unwrap(),
    ).await.unwrap();

    // 3. Login Dokter & Get Token
    let doc_login = LoginRequest {
        email: "doctor@test.com".to_string(),
        password: "password123".to_string(),
        role: Some("dokter".to_string()),
    };
    let doc_login_body = serde_json::to_vec(&doc_login).unwrap();
    let login_resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("Content-Type", "application/json")
            .body(Body::from(doc_login_body))
            .unwrap(),
    ).await.unwrap();
    let body_bytes = axum::body::to_bytes(login_resp.into_body(), 1024 * 10).await.unwrap();
    let login_res: AuthResponse = serde_json::from_slice(&body_bytes).unwrap();
    let doc_token = login_res.token.unwrap();

    // 4. Login Pasien untuk mendapatkan patient_id
    let pat_login = LoginRequest {
        email: "patient@test.com".to_string(),
        password: "password123".to_string(),
        role: Some("pasien".to_string()),
    };
    let pat_login_body = serde_json::to_vec(&pat_login).unwrap();
    let pat_login_resp = app.clone().oneshot(
        Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("Content-Type", "application/json")
            .body(Body::from(pat_login_body))
            .unwrap(),
    ).await.unwrap();
    let body_bytes_pat = axum::body::to_bytes(pat_login_resp.into_body(), 1024 * 10).await.unwrap();
    let pat_login_res: AuthResponse = serde_json::from_slice(&body_bytes_pat).unwrap();
    let patient_id = pat_login_res.user_id.unwrap();

    // 5. Test Impersonation Route using Doctor Token
    let imp_resp = app.oneshot(
        Request::builder()
            .method("POST")
            .uri(format!("/api/doctors/impersonate/{}", patient_id))
            .header("Authorization", format!("Bearer {}", doc_token))
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();

    assert_eq!(imp_resp.status(), StatusCode::OK);
    let imp_body_bytes = axum::body::to_bytes(imp_resp.into_body(), 1024 * 10).await.unwrap();
    let imp_res: AuthResponse = serde_json::from_slice(&imp_body_bytes).unwrap();
    
    assert!(imp_res.success);
    assert_eq!(imp_res.role.unwrap(), "pasien");
    assert!(imp_res.token.is_some());
}
