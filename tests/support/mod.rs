#![allow(dead_code)]
use axum::{body::Body, http::Request, response::Response, Router};
use ecg_backend::{
    api::{
        auth::Claims,
        routes::{create_router, AppState},
    },
    config::AppConfig,
};
use serde_json::{json, Value};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::{str::FromStr, time::Duration};
use tempfile::TempDir;
pub const SECRET: &str = "local-test-signing-secret-with-at-least-32-bytes";
pub struct TestContext {
    pub state: AppState,
    pub app: Router,
    pub schema: String,
    admin: PgPool,
    _directory: TempDir,
}
pub fn test_config(database_url: String, directory: &std::path::Path) -> AppConfig {
    AppConfig {
        host_ip: "127.0.0.1".into(),
        rest_port: 0,
        ws_port: 0,
        database_url,
        supabase_url: "https://auth.ecg.test".into(),
        supabase_anon_key: "test-public".into(),
        supabase_service_key: String::new(),
        jwt_secret: SECRET.into(),
        require_auth_session: false,
        allowed_origins: vec!["https://app.ecg.test".into()],
        records_root: directory.join("records"),
        uploads_root: directory.join("uploads"),
        apply_migrations: false,
    }
}
pub fn guard_database(url: &str) -> Result<(), String> {
    let options = PgConnectOptions::from_str(url).map_err(|_| "Invalid test database URL")?;
    if !matches!(options.get_host(), "127.0.0.1" | "localhost" | "::1")
        || options.get_database() != Some("ecg_test")
    {
        return Err("Tests require loopback host and database named ecg_test".into());
    }
    Ok(())
}
impl TestContext {
    pub async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL is required; tests never use DATABASE_URL");
        guard_database(&url).expect("Refusing non-test database before connection");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        let schema = format!("ecg_test_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&admin)
            .await
            .unwrap();
        let options = PgConnectOptions::from_str(&url)
            .unwrap()
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await
            .unwrap();
        ecg_backend::db::postgres::run_migrations(&pool)
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = test_config(url, directory.path());
        tokio::fs::create_dir_all(&config.records_root)
            .await
            .unwrap();
        let state = AppState::new(pool, config);
        let app = create_router(state.clone());
        let context = Self {
            state,
            app,
            schema,
            admin,
            _directory: directory,
        };
        context.fixtures().await;
        context
    }
    async fn fixtures(&self) {
        for (id, auth, role) in [
            ("acc_admin", "auth-admin", "admin"),
            ("acc_doctor", "auth-doctor", "dokter"),
            ("acc_other_doc", "auth-other-doc", "dokter"),
            ("acc_patient", "auth-patient", "pasien"),
            ("acc_other", "auth-other", "pasien"),
        ] {
            sqlx::query("INSERT INTO accounts(id,auth_id,email,role) VALUES($1,$2,$3,$4)")
                .bind(id)
                .bind(auth)
                .bind(format!("{auth}@test.invalid"))
                .bind(role)
                .execute(&self.state.pool)
                .await
                .unwrap();
        }
        for (id, account) in [("doc_A", "acc_doctor"), ("doc_B", "acc_other_doc")] {
            sqlx::query("INSERT INTO doctors(id,account_id,first_name,last_name) VALUES($1,$2,'Test','Doctor')").bind(id).bind(account).execute(&self.state.pool).await.unwrap();
        }
        for (id, account, doctor, device) in [
            ("pat_A", "acc_patient", "doc_A", "dev_A"),
            ("pat_B", "acc_other", "doc_B", "dev_B"),
        ] {
            sqlx::query("INSERT INTO devices(id,name,hardware_id) VALUES($1,$2,$2)")
                .bind(device)
                .bind(format!("device_{id}"))
                .execute(&self.state.pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO patients(id,account_id,first_name,last_name,age,primary_doctor_id,device_id) VALUES($1,$2,'Test','Patient',30,$3,$4)").bind(id).bind(account).bind(doctor).bind(device).execute(&self.state.pool).await.unwrap();
            sqlx::query("INSERT INTO sessions(id,device_id,patient_id,started_at,state) VALUES($1,$2,$3,now(),'recording')").bind(format!("ses_{id}")).bind(device).bind(id).execute(&self.state.pool).await.unwrap();
        }
    }
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        user: Option<&str>,
        body: Value,
    ) -> Response {
        use tower::ServiceExt;
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("Content-Type", "application/json");
        if let Some(user) = user {
            builder = builder.header("Authorization", format!("Bearer {}", token(user)));
        }
        self.app
            .clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }
    pub async fn cleanup(self) {
        let tasks = std::mem::take(&mut *self.state.mqtt_tasks.lock().await);
        for task in tasks.into_values() {
            task.abort();
            let _ = task.await;
        }
        self.state.mqtt_clients.write().await.clear();
        drop(self.app);
        self.state.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.admin)
            .await
            .unwrap();
        self.admin.close().await;
    }
}
pub fn token(user: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims(user),
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}
pub fn claims(user: &str) -> Claims {
    let now = chrono::Utc::now().timestamp() as u64;
    Claims {
        sub: format!("auth-{user}"),
        exp: now + 3600,
        iat: now,
        iss: "https://auth.ecg.test/auth/v1".into(),
        aud: "authenticated".into(),
        email: Some(format!("auth-{user}@test.invalid")),
        session_id: None,
    }
}
pub async fn json_response(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 20 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
pub fn payload(number: i64) -> ecg_backend::models::device::DevicePayload {
    serde_json::from_value(json!({"message_id":format!("source_{number}"),"device_id":"device_pat_A","session_id":"ses_pat_A","frame_id":number.to_string(),"created_at":chrono::Utc::now().to_rfc3339(),"sampling_rate_hz":250.0,"duration_s":0.2,"ecg":{"format":"samples_by_time","samples":vec![vec![0.5,0.6,0.1];50]}})).unwrap()
}
