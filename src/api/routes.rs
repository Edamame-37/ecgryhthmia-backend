use std::fs;
use std::path::Path;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use chrono::Utc;
use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
use jsonwebtoken::{decode, Validation, DecodingKey};
use sqlx::PgPool;
use uuid::Uuid;
use axum::{
    async_trait,
    routing::{get, post, put},
    Router,
    extract::{Path as AxumPath, State, Query, Json, FromRequestParts, FromRef, Multipart, DefaultBodyLimit},
    http::{request::Parts, StatusCode, Method, HeaderValue, header},
    response::IntoResponse,
};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, error};
use bcrypt::{hash, DEFAULT_COST};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub mqtt_clients: std::sync::Arc<tokio::sync::RwLock<HashMap<String, rumqttc::Client>>>,
    pub pacer_tx: tokio::sync::mpsc::UnboundedSender<crate::models::device::DevicePayload>,
    pub db_tx: tokio::sync::mpsc::UnboundedSender<crate::models::device::DevicePayload>,
    pub jwt_secret: String,
    pub api_url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppMetadata {
    pub role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub app_metadata: Option<AppMetadata>,
    pub exp: usize,
}

pub struct FullClaims {
    pub sub: String,
    pub role: String,
}

pub struct AdminClaims(pub FullClaims);

#[async_trait]
impl<S> FromRequestParts<S> for AdminClaims
where
    S: Send + Sync,
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        
        if let Some(auth_header) = parts.headers.get("Authorization").and_then(|v| v.to_str().ok()) {
            if auth_header.starts_with("Bearer ") {
                let token = &auth_header[7..];
                if let Some(claims) = validate_jwt(token, &app_state.jwt_secret) {
                    let mut role = claims.app_metadata.and_then(|m| m.role).unwrap_or_default();
                    
                    if role.is_empty() {
                        if let Ok(record) = sqlx::query!("SELECT role FROM accounts WHERE id = $1", claims.sub).fetch_one(&app_state.pool).await {
                            role = record.role;
                        }
                    }

                    if role == "admin" || claims.sub == "acc_admin" {
                        return Ok(AdminClaims(FullClaims { sub: claims.sub, role }));
                    }
                }
            }
        }
        
        Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Admin access required"}))))
    }
}

pub struct UserClaims(pub FullClaims);

#[async_trait]
impl<S> FromRequestParts<S> for UserClaims
where
    S: Send + Sync,
    AppState: axum::extract::FromRef<S>,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        
        let auth_header = parts.headers.get("Authorization").and_then(|v| v.to_str().ok())
            .ok_or((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Header Authorization tidak ditemukan"}))))?;

        if !auth_header.starts_with("Bearer ") {
            return Err((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Format token tidak valid"}))));
        }

        let token = &auth_header[7..];
        let claims = validate_jwt(token, &app_state.jwt_secret)
            .ok_or((StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "Sesi tidak valid atau kedaluwarsa"}))))?;

        let mut role = claims.app_metadata.and_then(|m| m.role).unwrap_or_default();
        if role.is_empty() {
            if let Ok(record) = sqlx::query!("SELECT role FROM accounts WHERE id = $1", claims.sub).fetch_one(&app_state.pool).await {
                role = record.role;
            }
        }

        Ok(UserClaims(FullClaims { sub: claims.sub, role }))
    }
}

#[derive(Serialize)]
pub struct SessionRecord {
    pub id: String,
    pub device_id: String,
    pub patient_id: Option<String>,
    pub patient_name: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub file_path: String,
    pub ecg_paper: Option<String>,
}

#[derive(Serialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub mqtt_broker: Option<String>,
    pub mqtt_port: Option<i32>,
    pub mqtt_topic: Option<String>,
    pub mqtt_username: Option<String>,
    pub assigned_to: Option<String>,
}

#[derive(Serialize)]
pub struct AdminStats {
    pub total_patients: i64,
    pub total_doctors: i64,
    pub active_devices: i64,
    pub critical_alerts: i64,
}

#[derive(Serialize)]
pub struct AdminUser {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub role: String,
    pub status: String,
    pub registered_at: Option<String>,
    pub connected_doctor_id: Option<String>,
    pub connected_device_id: Option<String>,
    pub profile_photo: Option<String>,
}

#[derive(Serialize)]
pub struct PatientRecord {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub age: String,
    pub gender: String,
    pub primary_doctor_id: Option<String>,
    pub profile_photo: Option<String>,
    pub device_id: Option<String>,
}

#[derive(Serialize)]
pub struct DoctorRecord {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub profile_photo: Option<String>,
}

#[derive(Serialize)]
pub struct PatientProfileResponse {
    pub patient: PatientRecord,
    pub doctor: Option<DoctorRecord>,
}

#[derive(Serialize)]
pub struct DoctorProfileResponse {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub role: String,
    pub profile_photo: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateDoctorProfileRequest {
    pub first_name: String,
    pub last_name: String,
    pub profile_photo: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdatePatientProfileRequest {
    pub first_name: String,
    pub last_name: String,
    pub age: String,
    pub profile_photo: Option<String>,
}

#[derive(Deserialize)]
pub struct ConnectPatientRequest {
    pub doctor_id: String,
}

#[derive(Deserialize, Serialize)]
pub struct AuthResponse {
    pub success: bool,
    pub message: String,
    pub user_id: Option<String>,
    pub role: Option<String>,
    pub token: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterProfileRequest {
    pub role: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub age: Option<i32>,
    pub gender: Option<String>,
}

#[derive(Deserialize)]
pub struct AdminRegisterRequest {
    pub email: String,
    pub password: String,
    pub role: String,
    pub first_name: String,
    pub last_name: String,
    pub age: Option<i32>,
    pub gender: Option<String>,
}

#[derive(Deserialize)]
pub struct FrameRequest {
    pub id: String,
    pub time_interval: String,
    pub session_id: Option<String>,
}

#[derive(Deserialize)]
pub struct FrameSessionRequest {
    pub session_id: String,
}

#[derive(Serialize)]
pub struct ConfirmationResponse {
    pub success: bool,
    pub message: String,
}

fn validate_jwt(token: &str, _secret: &str) -> Option<Claims> {
    let mut validation = Validation::default();
    validation.insecure_disable_signature_validation();
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    
    match decode::<Claims>(token, &DecodingKey::from_secret(&[]), &validation) {
        Ok(token_data) => Some(token_data.claims),
        Err(e) => {
            error!("JWT Validation error: {}", e);
            None
        }
    }
}

// ROUTE HANDLERS
async fn auth_me_handler(claims: UserClaims) -> impl IntoResponse {
    Json(AuthResponse {
        success: true,
        message: "Profil berhasil diambil".into(),
        user_id: Some(claims.0.sub),
        role: Some(claims.0.role),
        token: None,
    })
}

async fn register_profile_handler(
    claims: UserClaims,
    State(state): State<AppState>,
    Json(req): Json<RegisterProfileRequest>,
) -> impl IntoResponse {
    let account_id = claims.0.sub;
    
    if let Err(e) = sqlx::query!("INSERT INTO accounts (id, email, role, status) VALUES ($1, $2, $3, 'Online') ON CONFLICT (id) DO NOTHING", account_id, req.email, req.role).execute(&state.pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e.to_string()})));
    }

    if req.role == "dokter" {
        let _ = sqlx::query!("INSERT INTO doctors (id, account_id, first_name, last_name) VALUES ($1, $2, $3, $4) ON CONFLICT (id) DO NOTHING", account_id, account_id, req.first_name, req.last_name).execute(&state.pool).await;
    } else if req.role == "pasien" {
        let age = req.age.unwrap_or(0);
        let gender = req.gender.unwrap_or_default();
        let _ = sqlx::query!("INSERT INTO patients (id, account_id, first_name, last_name, age, gender) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (id) DO NOTHING", account_id, account_id, req.first_name, req.last_name, age, gender).execute(&state.pool).await;
    }

    (StatusCode::OK, Json(serde_json::json!({
        "success": true,
        "message": "Profil berhasil disimpan"
    })))
}

async fn admin_register_handler(
    _claims: AdminClaims,
    State(state): State<AppState>,
    Json(req): Json<AdminRegisterRequest>,
) -> impl IntoResponse {
    let new_user_id = Uuid::new_v4();
    let new_user_id_str = new_user_id.to_string();
    
    let hashed_password = match hash(&req.password, DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": format!("Gagal memproses kata sandi: {}", e)}))),
    };

    let raw_user_meta = serde_json::json!({"role": req.role});
    
    let insert_auth_res = sqlx::query(
        "INSERT INTO auth.users (id, instance_id, aud, role, email, encrypted_password, email_confirmed_at, raw_user_meta_data, created_at, updated_at) 
         VALUES ($1, '00000000-0000-0000-0000-000000000000', 'authenticated', 'authenticated', $2, $3, NOW(), $4, NOW(), NOW())"
    )
    .bind(new_user_id)
    .bind(&req.email)
    .bind(&hashed_password)
    .bind(&raw_user_meta)
    .execute(&state.pool).await;

    if let Err(e) = insert_auth_res {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": format!("Gagal mendaftarkan akun: {}", e)})));
    }

    if let Err(e) = sqlx::query!("INSERT INTO accounts (id, email, role, status) VALUES ($1, $2, $3, 'Offline')", new_user_id_str, req.email, req.role).execute(&state.pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": format!("Gagal mendaftarkan profil: {}", e)})));
    }

    if req.role == "dokter" {
        let _ = sqlx::query!("INSERT INTO doctors (id, account_id, first_name, last_name) VALUES ($1, $2, $3, $4)", new_user_id_str, new_user_id_str, req.first_name, req.last_name).execute(&state.pool).await;
    } else if req.role == "pasien" {
        let age = req.age.unwrap_or(0);
        let gender = req.gender.unwrap_or_default();
        let _ = sqlx::query!("INSERT INTO patients (id, account_id, first_name, last_name, age, gender) VALUES ($1, $2, $3, $4, $5, $6)", new_user_id_str, new_user_id_str, req.first_name, req.last_name, age, gender).execute(&state.pool).await;
    }

    (StatusCode::OK, Json(serde_json::json!({
        "success": true,
        "message": "Pengguna berhasil didaftarkan"
    })))
}

async fn get_sessions_handler(
    claims: UserClaims,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut filter_patient_id = params.get("patient_id").cloned();
    let mut filter_doctor_id = params.get("doctor_id").cloned();

    if claims.0.role == "dokter" {
        filter_doctor_id = Some(claims.0.sub.clone());
    } else if claims.0.role == "pasien" {
        filter_patient_id = Some(claims.0.sub.clone());
    }

    let sessions = get_sessions_from_db(filter_patient_id, filter_doctor_id, &state.pool).await;
    Json(serde_json::json!({ "sessions": sessions }))
}

async fn get_patient_sessions_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
) -> impl IntoResponse {
    let sessions = get_sessions_from_db(Some(patient_id), None, &state.pool).await;
    Json(sessions)
}

async fn get_devices_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let devices = get_devices_from_db(&state.pool).await;
    Json(devices)
}

async fn get_admin_stats_handler(
    _claims: AdminClaims,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let stats = get_admin_stats(&state.pool).await;
    Json(stats)
}

async fn get_admin_users_handler(
    _claims: AdminClaims,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let users = get_admin_users(&state.pool).await;
    Json(users)
}

async fn impersonate_handler(
    _claims: AdminClaims,
    State(state): State<AppState>,
    AxumPath(target_id): AxumPath<String>,
) -> impl IntoResponse {
    let role = sqlx::query!("SELECT role FROM accounts WHERE id = $1", target_id)
        .fetch_one(&state.pool).await.ok().map(|r| r.role);
    if let Some(r) = role {
        (StatusCode::OK, Json(serde_json::json!({
            "success": true,
            "user_id": target_id,
            "role": r
        })))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"success": false, "message": "User tidak ditemukan"})))
    }
}

async fn doctor_impersonate_handler(
    claims: UserClaims,
    State(state): State<AppState>,
    AxumPath(target_id): AxumPath<String>,
) -> impl IntoResponse {
    if claims.0.role != "dokter" {
        return (StatusCode::FORBIDDEN, Json(serde_json::json!({"success": false, "message": "Hanya dokter yang dapat melakukan impersonasi"})));
    }
    
    let doctor_account_id = claims.0.sub;
    let doc_res = sqlx::query!("SELECT id FROM doctors WHERE account_id = $1", doctor_account_id).fetch_one(&state.pool).await;
    let doc_id = match doc_res {
        Ok(rec) => rec.id,
        Err(_) => return (StatusCode::FORBIDDEN, Json(serde_json::json!({"success": false, "message": "Dokter tidak valid"})))
    };
    
    let target_patient = sqlx::query!("SELECT id FROM patients WHERE account_id = $1 AND primary_doctor_id = $2", target_id, doc_id).fetch_optional(&state.pool).await;
    
    match target_patient {
        Ok(Some(_)) => {
            (StatusCode::OK, Json(serde_json::json!({
                "success": true,
                "user_id": target_id,
                "role": "pasien"
            })))
        },
        _ => (StatusCode::FORBIDDEN, Json(serde_json::json!({"success": false, "message": "Pasien bukan milik dokter ini atau tidak ditemukan"})))
    }
}

async fn get_patients_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let patients = sqlx::query!("SELECT id, first_name, last_name, age, gender FROM patients")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "name": format!("{} {}", row.first_name, row.last_name).trim().to_string(),
                "age": row.age,
                "gender": row.gender
            })
        })
        .collect::<Vec<_>>();
        
    (StatusCode::OK, Json(serde_json::json!(patients)))
}

async fn get_patient_profile_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
) -> impl IntoResponse {
    if let Some(profile) = get_patient_profile(patient_id, &state.pool).await {
        (StatusCode::OK, Json(serde_json::json!(profile)))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({})))
    }
}

async fn get_doctor_profile_handler(
    State(state): State<AppState>,
    AxumPath(doctor_id): AxumPath<String>,
) -> impl IntoResponse {
    if let Some(profile) = get_doctor_profile(doctor_id, &state.pool).await {
        (StatusCode::OK, Json(serde_json::json!(profile)))
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({})))
    }
}

async fn update_doctor_profile_handler(
    State(state): State<AppState>,
    AxumPath(doctor_id): AxumPath<String>,
    Json(req): Json<UpdateDoctorProfileRequest>,
) -> impl IntoResponse {
    match update_doctor_profile(&doctor_id, req, &state.pool, &state.api_url).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"success": true}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e}))),
    }
}

async fn update_patient_profile_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
    Json(req): Json<UpdatePatientProfileRequest>,
) -> impl IntoResponse {
    match update_patient_profile(&patient_id, req, &state.pool, &state.api_url).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"success": true}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e}))),
    }
}

async fn connect_patient_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
    Json(req): Json<ConnectPatientRequest>,
) -> impl IntoResponse {
    let actual_patient_id = sqlx::query!("SELECT id FROM patients WHERE id = $1 OR account_id = $1", patient_id)
        .fetch_one(&state.pool).await.map(|r| r.id).unwrap_or(patient_id.to_string());
    match sqlx::query!("UPDATE patients SET primary_doctor_id = $1 WHERE id = $2", req.doctor_id, actual_patient_id)
        .execute(&state.pool).await 
    {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"success": true}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e.to_string()}))),
    }
}

async fn disconnect_patient_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
) -> impl IntoResponse {
    let actual_patient_id = sqlx::query!("SELECT id FROM patients WHERE id = $1 OR account_id = $1", patient_id)
        .fetch_one(&state.pool).await.map(|r| r.id).unwrap_or(patient_id.to_string());
    match sqlx::query!("UPDATE patients SET primary_doctor_id = NULL WHERE id = $1", actual_patient_id)
        .execute(&state.pool).await 
    {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"success": true}))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e.to_string()}))),
    }
}

async fn get_doctor_patients_handler(
    State(state): State<AppState>,
    AxumPath(doctor_id): AxumPath<String>,
) -> impl IntoResponse {
    let actual_doctor_id = sqlx::query!("SELECT id FROM doctors WHERE id = $1 OR account_id = $1", doctor_id)
        .fetch_one(&state.pool).await.map(|r| r.id).unwrap_or(doctor_id.to_string());
    let patients = sqlx::query!(
        "SELECT p.id, p.first_name, p.last_name, a.profile_photo 
         FROM patients p 
         LEFT JOIN accounts a ON p.account_id = a.id 
         WHERE p.primary_doctor_id = $1", actual_doctor_id
    ).fetch_all(&state.pool).await.unwrap_or_default()
    .into_iter()
    .map(|row| {
        serde_json::json!({
            "id": row.id,
            "name": format!("{} {}", row.first_name, row.last_name).trim().to_string(),
            "profile_photo": row.profile_photo,
        })
    }).collect::<Vec<_>>();

    (StatusCode::OK, Json(serde_json::json!(patients)))
}

async fn get_record_handler(
    AxumPath(session_id): AxumPath<String>,
) -> impl IntoResponse {
    let response_body = read_jsonl_file(&session_id);
    axum::response::Response::builder()
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(response_body))
        .unwrap()
}

#[derive(Deserialize, Serialize)]
struct AssignRequest {
    patient_id: Option<String>,
}

async fn assign_device_handler(
    State(state): State<AppState>,
    AxumPath(device_id): AxumPath<String>,
    Json(req): Json<AssignRequest>,
) -> impl IntoResponse {
    if let Some(pid) = req.patient_id {
        let _ = sqlx::query!("UPDATE patients SET device_id = NULL WHERE device_id = $1", device_id).execute(&state.pool).await;
        let _ = sqlx::query!("UPDATE patients SET device_id = $1 WHERE id = $2", device_id, pid).execute(&state.pool).await;
    } else {
        let _ = sqlx::query!("UPDATE patients SET device_id = NULL WHERE device_id = $1", device_id).execute(&state.pool).await;
    }
    (StatusCode::OK, Json(serde_json::json!({"success": true})))
}

#[derive(Deserialize, Serialize)]
struct DeviceCommand {
    command: String,
    patient_id: Option<String>,
}

async fn device_command_handler(
    State(state): State<AppState>,
    AxumPath(device_id): AxumPath<String>,
    Json(cmd): Json<DeviceCommand>,
) -> impl IntoResponse {
    if cmd.command.to_uppercase() == "START" {
        info!(device_id = %device_id, "Perekaman Dimulai");
    } else if cmd.command.to_uppercase() == "STOP" {
        info!(device_id = %device_id, "Perekaman Selesai");
        let now = chrono::Utc::now();
        if let Err(e) = sqlx::query!(
            "UPDATE sessions SET ended_at = $1 WHERE ended_at IS NULL AND device_id = (SELECT id FROM devices WHERE name = $2 OR id = $2 LIMIT 1)",
            now, device_id
        ).execute(&state.pool).await {
            error!(error = %e, device_id = %device_id, "Gagal mengupdate ended_at untuk sesi perekaman");
        }
    }

    // Query mqtt_topic from db
    let db_topic_record = sqlx::query!("SELECT mqtt_topic FROM devices WHERE id = $1", device_id)
        .fetch_one(&state.pool).await.ok();
        
    let base_topic = if let Some(record) = db_topic_record {
        record.mqtt_topic.unwrap_or_else(|| format!("ecgrhythmia/{}", device_id))
    } else {
        format!("ecgrhythmia/{}", device_id)
    };
    
    let topic = format!("{}/command", base_topic);
    let clients = state.mqtt_clients.read().await;
    
    if let Some(client) = clients.get(&device_id) {
        let payload = cmd.command.clone();
        if let Err(e) = client.clone().publish(&topic, rumqttc::QoS::AtLeastOnce, false, payload) {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": format!("Gagal mengirim perintah: {}", e)})))
        } else {
            info!(device_id = %device_id, topic = %topic, command = %cmd.command, "Berhasil mengirim perintah MQTT ke perangkat");
            (StatusCode::OK, Json(serde_json::json!({"success": true})))
        }
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"success": false, "message": "Perangkat tidak memiliki koneksi MQTT aktif"})))
    }
}

fn parse_time_seconds(time_str: &str) -> f64 {
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() == 2 {
        let m: f64 = parts[0].parse().unwrap_or(0.0);
        let s: f64 = parts[1].parse().unwrap_or(0.0);
        return m * 60.0 + s;
    }
    0.0
}

async fn frame_preregister_handler(
    State(state): State<AppState>,
    Json(req): Json<FrameRequest>,
) -> impl IntoResponse {
    let parts: Vec<&str> = req.time_interval.split(" - ").collect();
    let mut start_time = 0.0;
    let mut end_time = 10.0;
    if parts.len() == 2 {
        start_time = parse_time_seconds(parts[0]);
        end_time = parse_time_seconds(parts[1]);
    }

    match sqlx::query!(
        "INSERT INTO frame_records (id, session_id, time_interval, start_time, end_time, label, hidden) VALUES ($1, $2, $3, $4, $5, 'Processing', FALSE)", 
        req.id, req.session_id, req.time_interval, start_time, end_time
    )
        .execute(&state.pool).await 
    {
        Ok(_) => (StatusCode::OK, Json(ConfirmationResponse { success: true, message: "Frame pre-registered".to_string() })),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ConfirmationResponse { success: false, message: e.to_string() }))
    }
}

async fn frame_session_update_handler(
    State(state): State<AppState>,
    AxumPath(frame_id): AxumPath<String>,
    Json(req): Json<FrameSessionRequest>,
) -> impl IntoResponse {
    match sqlx::query!("UPDATE frame_records SET session_id = $1 WHERE id = $2", req.session_id, frame_id)
        .execute(&state.pool).await 
    {
        Ok(_) => (StatusCode::OK, Json(ConfirmationResponse { success: true, message: "Frame session updated".to_string() })),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ConfirmationResponse { success: false, message: e.to_string() }))
    }
}

// DATABASE UTILITIES & CRUDS
async fn get_sessions_from_db(
    filter_patient_id: Option<String>,
    filter_doctor_id: Option<String>,
    pool: &PgPool
) -> Vec<SessionRecord> {
    let mut actual_doc_id = None;
    let mut is_doctor_filtered = false;
    
    if let Some(did) = filter_doctor_id {
        is_doctor_filtered = true;
        match sqlx::query!("SELECT id FROM doctors WHERE id = $1 OR account_id = $1", did).fetch_one(pool).await {
            Ok(r) => actual_doc_id = Some(r.id),
            Err(e) => {
                tracing::error!("Failed to find doctor for id {}: {}", did, e);
            }
        }
    }

    let mut actual_pat_id = None;
    let mut is_patient_filtered = false;
    
    if let Some(pid) = filter_patient_id {
        is_patient_filtered = true;
        match sqlx::query!("SELECT id FROM patients WHERE id = $1 OR account_id = $1", pid).fetch_one(pool).await {
            Ok(r) => actual_pat_id = Some(r.id),
            Err(e) => {
                tracing::error!("Failed to find patient for id {}: {}", pid, e);
            }
        }
    }

    // SECURITY FAILSAFE: If a doctor or patient filter was requested but NOT found in DB, return empty immediately!
    if (is_doctor_filtered && actual_doc_id.is_none()) || (is_patient_filtered && actual_pat_id.is_none()) {
        tracing::warn!("Security failsafe triggered: requested filter not found in database. Returning empty sessions array.");
        return vec![];
    }

    if let (Some(pid), Some(did)) = (&actual_pat_id, &actual_doc_id) {
        let belongs = sqlx::query!("SELECT 1 as x FROM patients WHERE id = $1 AND primary_doctor_id = $2", pid, did)
            .fetch_optional(pool).await.unwrap_or_default().is_some();
        if !belongs {
            return vec![];
        }
    }

    if let Some(pid) = actual_pat_id {
        sqlx::query!(
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name as patient_name, s.started_at, s.ended_at, s.file_path, s.ecg_paper 
             FROM sessions s LEFT JOIN patients p ON s.patient_id = p.id 
             WHERE s.patient_id = $1 ORDER BY s.started_at DESC", pid
        ).fetch_all(pool).await.unwrap_or_default()
        .into_iter().map(|row| SessionRecord {
            id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
            started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path.unwrap_or_default(),
            ecg_paper: row.ecg_paper
        }).collect()
    } else if let Some(did) = actual_doc_id {
        sqlx::query!(
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name as patient_name, s.started_at, s.ended_at, s.file_path, s.ecg_paper 
             FROM sessions s JOIN patients p ON s.patient_id = p.id 
             WHERE p.primary_doctor_id = $1 ORDER BY s.started_at DESC", did
        ).fetch_all(pool).await.unwrap_or_default()
        .into_iter().map(|row| SessionRecord {
            id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
            started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path.unwrap_or_default(),
            ecg_paper: row.ecg_paper
        }).collect()
    } else {
        sqlx::query!(
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name as patient_name, s.started_at, s.ended_at, s.file_path, s.ecg_paper 
             FROM sessions s LEFT JOIN patients p ON s.patient_id = p.id 
             ORDER BY s.started_at DESC"
        ).fetch_all(pool).await.unwrap_or_default()
        .into_iter().map(|row| SessionRecord {
            id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
            started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path.unwrap_or_default(),
            ecg_paper: row.ecg_paper
        }).collect()
    }
}

async fn get_devices_from_db(pool: &PgPool) -> Vec<DeviceRecord> {
    sqlx::query!(
        "SELECT d.id as \"id!\", d.name as \"name!\", d.mqtt_broker, d.mqtt_port, d.mqtt_topic, d.mqtt_username, p.id as \"assigned_to?\"
         FROM devices d LEFT JOIN patients p ON d.id = p.device_id"
    ).fetch_all(pool).await.unwrap_or_default()
    .into_iter().map(|row| DeviceRecord {
        id: row.id, name: row.name, mqtt_broker: row.mqtt_broker, mqtt_port: row.mqtt_port,
        mqtt_topic: row.mqtt_topic, mqtt_username: row.mqtt_username, assigned_to: row.assigned_to
    }).collect()
}

async fn get_admin_stats(pool: &PgPool) -> AdminStats {
    let mut stats = AdminStats { total_patients: 0, total_doctors: 0, active_devices: 0, critical_alerts: 0 };
    stats.total_patients = sqlx::query!("SELECT COUNT(*) FROM patients").fetch_one(pool).await.map(|r| r.count.unwrap_or(0)).unwrap_or(0);
    stats.total_doctors = sqlx::query!("SELECT COUNT(*) FROM doctors").fetch_one(pool).await.map(|r| r.count.unwrap_or(0)).unwrap_or(0);
    stats.active_devices = sqlx::query!("SELECT COUNT(*) FROM devices").fetch_one(pool).await.map(|r| r.count.unwrap_or(0)).unwrap_or(0);
    
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_prefix = format!("{}%", today);
    
    let paths = sqlx::query!("SELECT file_path FROM sessions WHERE CAST(started_at AS TEXT) LIKE $1", today_prefix).fetch_all(pool).await.unwrap_or_default();
    let mut critical_count = 0;
    for path in paths {
        if let Ok(contents) = fs::read_to_string(path.file_path.as_deref().unwrap_or_default()) {
            for line in contents.lines() {
                if !line.contains("\"label\":\"Normal\"") && line.contains("\"label\":") {
                    critical_count += 1;
                }
            }
        }
    }
    stats.critical_alerts = critical_count as i64;
    stats
}

async fn get_admin_users(pool: &PgPool) -> Vec<AdminUser> {
    sqlx::query!(
        "SELECT p.id, a.id as account_id, p.first_name || ' ' || p.last_name as name, a.role, COALESCE(a.status, 'Offline') as status, a.created_at, p.primary_doctor_id as connected_doctor_id, p.device_id as connected_device_id, a.profile_photo
         FROM patients p JOIN accounts a ON p.account_id = a.id
         UNION ALL
         SELECT d.id, a.id as account_id, d.first_name || ' ' || d.last_name as name, a.role, COALESCE(a.status, 'Offline') as status, a.created_at, NULL as connected_doctor_id, NULL as connected_device_id, a.profile_photo
         FROM doctors d JOIN accounts a ON d.account_id = a.id
         ORDER BY created_at DESC"
    ).fetch_all(pool).await.unwrap_or_default()
    .into_iter().map(|row| AdminUser {
        id: row.id.unwrap_or_default(), account_id: row.account_id.unwrap_or_default(), name: row.name.unwrap_or_default(), role: row.role.unwrap_or_default(), status: row.status.unwrap_or_default(), 
        registered_at: row.created_at.map(|t| t.to_string()), connected_doctor_id: row.connected_doctor_id, connected_device_id: row.connected_device_id, profile_photo: row.profile_photo
    }).collect()
}

async fn get_patient_profile(patient_id: String, pool: &PgPool) -> Option<PatientProfileResponse> {
    let patient_res = sqlx::query!(
        "SELECT p.id, p.first_name, p.last_name, p.age, p.gender, p.primary_doctor_id, a.profile_photo, p.device_id
         FROM patients p LEFT JOIN accounts a ON p.account_id = a.id WHERE p.id = $1 OR p.account_id = $1", patient_id
    ).fetch_one(pool).await.ok()?;

    let mut doctor = None;
    if let Some(doc_id) = patient_res.primary_doctor_id.clone() {
        if let Ok(doc_res) = sqlx::query!(
            "SELECT d.id, d.first_name, d.last_name, a.profile_photo FROM doctors d LEFT JOIN accounts a ON d.account_id = a.id WHERE d.id = $1", doc_id
        ).fetch_one(pool).await {
            doctor = Some(DoctorRecord {
                id: doc_res.id, first_name: doc_res.first_name, last_name: doc_res.last_name, profile_photo: doc_res.profile_photo
            });
        }
    }

    Some(PatientProfileResponse {
        patient: PatientRecord {
            id: patient_res.id, first_name: patient_res.first_name, last_name: patient_res.last_name, age: patient_res.age.to_string(),
            gender: patient_res.gender.unwrap_or_default(), primary_doctor_id: patient_res.primary_doctor_id, profile_photo: patient_res.profile_photo, device_id: patient_res.device_id
        },
        doctor,
    })
}

async fn get_doctor_profile(doctor_id: String, pool: &PgPool) -> Option<DoctorProfileResponse> {
    let res = sqlx::query!(
        "SELECT d.id, d.first_name, d.last_name, a.email, a.role, a.profile_photo FROM doctors d LEFT JOIN accounts a ON d.account_id = a.id WHERE d.id = $1 OR d.account_id = $1", doctor_id
    ).fetch_one(pool).await.ok()?;

    Some(DoctorProfileResponse {
        id: res.id, first_name: res.first_name, last_name: res.last_name,
        email: res.email, role: res.role, profile_photo: res.profile_photo
    })
}

async fn update_doctor_profile(doctor_id: &str, req: UpdateDoctorProfileRequest, pool: &PgPool, api_url: &str) -> Result<(), String> {
    let doctor_record = sqlx::query!("SELECT id, account_id FROM doctors WHERE id = $1 OR account_id = $1", doctor_id)
        .fetch_one(pool).await.map_err(|_| "Dokter tidak ditemukan".to_string())?;
    let actual_doctor_id = doctor_record.id;
    let account_id = doctor_record.account_id;

    let mut final_photo_url = req.profile_photo.filter(|s| !s.is_empty());

    if let Some(ref photo_data) = final_photo_url {
        if photo_data.starts_with("data:image/") {
            if let Some(comma_pos) = photo_data.find(',') {
                let base64_str = &photo_data[comma_pos + 1..];
                if let Ok(image_bytes) = base64_engine.decode(base64_str) {
                    let uploads_dir = Path::new("uploads/profiles");
                    let _ = fs::create_dir_all(uploads_dir);
                    let filename = format!("{}_{}.jpg", doctor_id, Utc::now().timestamp());
                    let filepath = uploads_dir.join(&filename);
                    if fs::write(&filepath, image_bytes).is_ok() {
                        final_photo_url = Some(format!("{}/uploads/profiles/{}", api_url.trim_end_matches('/'), filename));
                    }
                }
            }
        }
    }

    sqlx::query!("UPDATE doctors SET first_name = $1, last_name = $2 WHERE id = $3", req.first_name, req.last_name, actual_doctor_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    sqlx::query!("UPDATE accounts SET profile_photo = $1 WHERE id = $2", final_photo_url, account_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    Ok(())
}

async fn update_patient_profile(patient_id: &str, req: UpdatePatientProfileRequest, pool: &PgPool, api_url: &str) -> Result<(), String> {
    let patient_record = sqlx::query!("SELECT id, account_id FROM patients WHERE id = $1 OR account_id = $1", patient_id)
        .fetch_one(pool).await.map_err(|_| "Pasien tidak ditemukan".to_string())?;
    let actual_patient_id = patient_record.id;
    let account_id = patient_record.account_id;

    let mut final_photo_url = req.profile_photo.filter(|s| !s.is_empty());

    if let Some(ref photo_data) = final_photo_url {
        if photo_data.starts_with("data:image/") {
            if let Some(comma_pos) = photo_data.find(',') {
                let base64_str = &photo_data[comma_pos + 1..];
                if let Ok(image_bytes) = base64_engine.decode(base64_str) {
                    let uploads_dir = Path::new("uploads/profiles");
                    let _ = fs::create_dir_all(uploads_dir);
                    let filename = format!("{}_{}.jpg", patient_id, Utc::now().timestamp());
                    let filepath = uploads_dir.join(&filename);
                    if fs::write(&filepath, image_bytes).is_ok() {
                        final_photo_url = Some(format!("{}/uploads/profiles/{}", api_url.trim_end_matches('/'), filename));
                    }
                }
            }
        }
    }

    sqlx::query!("UPDATE patients SET first_name = $1, last_name = $2, age = $3 WHERE id = $4", req.first_name, req.last_name, req.age.parse::<i32>().unwrap_or(0), actual_patient_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    sqlx::query!("UPDATE accounts SET profile_photo = $1 WHERE id = $2", final_photo_url, account_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    Ok(())
}

fn read_jsonl_file(session_id: &str) -> String {
    let file_path = format!("records_local/{}.jsonl", session_id);
    let fallback_path = format!("records/records_local/{}.jsonl", session_id);
    if let Ok(contents) = fs::read_to_string(&file_path) {
        let lines: Vec<&str> = contents.lines().filter(|l| !l.is_empty()).collect();
        format!("[{}]", lines.join(","))
    } else if let Ok(contents) = fs::read_to_string(&fallback_path) {
        let lines: Vec<&str> = contents.lines().filter(|l| !l.is_empty()).collect();
        format!("[{}]", lines.join(","))
    } else {
        "[]".to_string()
    }
}

#[derive(Deserialize)]
pub struct NewDeviceReq {
    pub name: String,
    pub mqtt_broker: String,
    pub mqtt_port: i32,
    pub mqtt_topic: String,
    pub mqtt_username: String,
    pub mqtt_password: String,
}

#[derive(Deserialize)]
pub struct EditDeviceReq {
    pub name: String,
    pub mqtt_broker: String,
    pub mqtt_port: i32,
    pub mqtt_topic: String,
    pub mqtt_username: String,
    pub mqtt_password: String,
}

pub async fn add_device_handler(
    State(state): State<AppState>,
    _claims: AdminClaims,
    Json(req): Json<NewDeviceReq>,
) -> impl IntoResponse {
    let dev_id = format!("dev_{}", chrono::Utc::now().timestamp_millis());
    if let Err(e) = sqlx::query!(
        "INSERT INTO devices (id, name, mqtt_broker, mqtt_port, mqtt_topic, mqtt_username, mqtt_password) VALUES ($1, $2, $3, $4, $5, $6, $7)",
        dev_id, req.name, req.mqtt_broker, req.mqtt_port, req.mqtt_topic, req.mqtt_username, req.mqtt_password
    ).execute(&state.pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e.to_string()})));
    }
    
    let db_tx = state.db_tx.clone();
    let port = req.mqtt_port as u16;
    let client = crate::network::mqtt_listener::start_mqtt_listener(
        &req.mqtt_broker, port, &req.mqtt_topic, &req.mqtt_username, &req.mqtt_password,
        move |payload_str| {
            if let Ok(device_payload) = serde_json::from_str::<crate::models::device::DevicePayload>(&payload_str) {
                let _ = db_tx.send(device_payload);
            }
        }
    );
    
    let mut clients = state.mqtt_clients.write().await;
    clients.insert(req.name.clone(), client);
    (StatusCode::OK, Json(serde_json::json!({"success": true, "message": "Perangkat didaftarkan dan pairing dimulai"})))
}

pub async fn edit_device_handler(
    State(state): State<AppState>,
    _claims: AdminClaims,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<EditDeviceReq>,
) -> impl IntoResponse {
    let old_name = sqlx::query!("SELECT name FROM devices WHERE id = $1", id).fetch_one(&state.pool).await.map(|r| r.name).ok();
    if let Err(e) = sqlx::query!(
        "UPDATE devices SET name = $1, mqtt_broker = $2, mqtt_port = $3, mqtt_topic = $4, mqtt_username = $5, mqtt_password = $6 WHERE id = $7",
        req.name, req.mqtt_broker, req.mqtt_port, req.mqtt_topic, req.mqtt_username, req.mqtt_password, id
    ).execute(&state.pool).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": e.to_string()})));
    }

    if let Some(old_name) = old_name {
        let mut clients = state.mqtt_clients.write().await;
        if let Some(old_client) = clients.remove(&old_name) {
            let _ = old_client.disconnect();
        }
    }

    let db_tx = state.db_tx.clone();
    let port = req.mqtt_port as u16;
    let client = crate::network::mqtt_listener::start_mqtt_listener(
        &req.mqtt_broker, port, &req.mqtt_topic, &req.mqtt_username, &req.mqtt_password,
        move |payload_str| {
            if let Ok(device_payload) = serde_json::from_str::<crate::models::device::DevicePayload>(&payload_str) {
                let _ = db_tx.send(device_payload);
            }
        }
    );
    
    let mut clients = state.mqtt_clients.write().await;
    clients.insert(req.name.clone(), client);
    (StatusCode::OK, Json(serde_json::json!({"success": true, "message": "Perangkat berhasil diupdate"})))
}

// AXUM ROUTER GENERATOR
pub fn create_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin([
            "https://ecgrhythmia.cloud".parse::<HeaderValue>().unwrap(),
            "https://www.ecgrhythmia.cloud".parse::<HeaderValue>().unwrap(),
            "http://localhost:5173".parse::<HeaderValue>().unwrap(),
            "http://localhost:5174".parse::<HeaderValue>().unwrap(),
            "http://localhost:5175".parse::<HeaderValue>().unwrap(),
        ])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
        .allow_credentials(true);

    Router::new()
        .route("/api/auth/me", get(auth_me_handler))
        .route("/api/auth/register_profile", post(register_profile_handler))
        .route("/api/auth/register", post(admin_register_handler))
        .route("/api/sessions", get(get_sessions_handler))
        .route("/api/devices", get(get_devices_handler))
        .route("/api/admin/stats", get(get_admin_stats_handler))
        .route("/api/admin/users", get(get_admin_users_handler))
        .route("/api/admin/impersonate/:target_id", post(impersonate_handler))
        .route("/api/admin/devices", get(get_devices_handler).post(add_device_handler))
        .route("/api/admin/devices/:id", put(edit_device_handler))
        .route("/api/patients", get(get_patients_handler))
        .route("/api/patients/:patient_id/sessions", get(get_patient_sessions_handler))
        .route("/api/patients/:patient_id", get(get_patient_profile_handler).put(update_patient_profile_handler))
        .route("/api/patients/:patient_id/connect", post(connect_patient_handler))
        .route("/api/patients/:patient_id/disconnect", post(disconnect_patient_handler))
        .route("/api/doctors/:doctor_id/patients", get(get_doctor_patients_handler))
        .route("/api/doctors/:doctor_id", get(get_doctor_profile_handler).put(update_doctor_profile_handler))
        .route("/api/doctors/impersonate/:target_id", post(doctor_impersonate_handler))
        .route("/api/records/:session_id", get(get_record_handler))
        .route("/api/sessions/:session_id/ecg_paper", post(upload_ecg_paper_handler).delete(delete_ecg_paper_handler))
        .route("/api/devices/:device_id/command", post(device_command_handler))
        .route("/api/devices/:device_id/assign", post(assign_device_handler))
        .route("/api/frames", post(frame_preregister_handler))
        .route("/api/frames/:id/session", put(frame_session_update_handler))
        .nest_service("/uploads", tower_http::services::ServeDir::new("uploads"))
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}


#[derive(Serialize)]
pub struct UploadResponse {
    pub success: bool,
    pub path: Option<String>,
    pub message: Option<String>,
}

pub async fn upload_ecg_paper_handler(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    while let Some(field) = multipart.next_field().await.unwrap_or(None) {
        if field.name() == Some("paper") {
            let data = match field.bytes().await {
                Ok(d) => d,
                Err(_) => return (StatusCode::BAD_REQUEST, Json(UploadResponse { success: false, path: None, message: Some("Failed to read file data".to_string()) })),
            };

            // Hapus file lama jika ada
            if let Ok(record) = sqlx::query!("SELECT ecg_paper FROM sessions WHERE id = $1", session_id).fetch_one(&state.pool).await {
                if let Some(old_path) = record.ecg_paper {
                    if let Some(filename) = old_path.split('/').last() {
                        let old_file_path = format!("uploads/ecg_papers/{}", filename);
                        let _ = tokio::fs::remove_file(&old_file_path).await;
                    }
                }
            }

            let file_name = format!("{}_{}.jpg", session_id, uuid::Uuid::new_v4());
            let file_path = format!("uploads/ecg_papers/{}", file_name);
            let public_path = format!("/uploads/ecg_papers/{}", file_name);

            match tokio::fs::write(&file_path, &data).await {
                Ok(_) => {
                    let update_result = sqlx::query!(
                        "UPDATE sessions SET ecg_paper = $1 WHERE id = $2",
                        public_path,
                        session_id
                    ).execute(&state.pool).await;

                    if update_result.is_ok() {
                        return (StatusCode::OK, Json(UploadResponse { success: true, path: Some(public_path), message: None }));
                    } else {
                        return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse { success: false, path: None, message: Some("Failed to update database".to_string()) }));
                    }
                }
                Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse { success: false, path: None, message: Some("Failed to save file".to_string()) })),
            }
        }
    }
    (StatusCode::BAD_REQUEST, Json(UploadResponse { success: false, path: None, message: Some("No file uploaded".to_string()) }))
}

pub async fn delete_ecg_paper_handler(
    State(state): State<AppState>,
    AxumPath(session_id): AxumPath<String>,
) -> impl IntoResponse {
    // Hapus file lama jika ada
    if let Ok(record) = sqlx::query!("SELECT ecg_paper FROM sessions WHERE id = $1", session_id).fetch_one(&state.pool).await {
        if let Some(old_path) = record.ecg_paper {
            if let Some(filename) = old_path.split('/').last() {
                let old_file_path = format!("uploads/ecg_papers/{}", filename);
                let _ = tokio::fs::remove_file(&old_file_path).await;
            }
        }
    }

    let update_result = sqlx::query!(
        "UPDATE sessions SET ecg_paper = NULL WHERE id = $1",
        session_id
    ).execute(&state.pool).await;

    if update_result.is_ok() {
        (StatusCode::OK, Json(UploadResponse { success: true, path: None, message: None }))
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(UploadResponse { success: false, path: None, message: Some("Failed to update database".to_string()) }))
    }
}
