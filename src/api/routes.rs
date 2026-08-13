use std::fs;
use std::path::Path;
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use chrono::Utc;
use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
use jsonwebtoken::{decode, Algorithm, Validation, DecodingKey};
use sqlx::PgPool;
use axum::{
    async_trait,
    routing::{get, post, put},
    Router,
    extract::{Path as AxumPath, State, Query, Json, FromRequestParts, FromRef},
    http::{request::Parts, StatusCode, Method, HeaderValue, header},
    response::IntoResponse,
};
use tower_http::cors::CorsLayer;
use tracing::{info, error};

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
}

#[derive(Serialize)]
pub struct DeviceRecord {
    pub id: String,
    pub name: String,
    pub mqtt_broker: Option<String>,
    pub mqtt_port: Option<i32>,
    pub mqtt_topic: Option<String>,
    pub mqtt_username: Option<String>,
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
    pub date_of_birth: String,
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
    pub date_of_birth: String,
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

fn validate_jwt(token: &str, secret: &str) -> Option<Claims> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    validation.set_audience(&["authenticated".to_string()]); // Supabase typical audience

    // We disable audience checking for now just in case the Supabase token format differs
    validation.validate_aud = false;
    
    match decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation) {
        Ok(token_data) => Some(token_data.claims),
        Err(e) => {
            error!("JWT Validation error: {}", e);
            None
        }
    }
}

// ROUTE HANDLERS
async fn get_sessions_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let patient_id = params.get("patient_id").cloned();
    let sessions = get_sessions_from_db(patient_id, &state.pool).await;
    Json(serde_json::json!({ "sessions": sessions }))
}

async fn get_patient_sessions_handler(
    State(state): State<AppState>,
    AxumPath(patient_id): AxumPath<String>,
) -> impl IntoResponse {
    let sessions = get_sessions_from_db(Some(patient_id), &state.pool).await;
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
    State(_state): State<AppState>,
    AxumPath(_target_id): AxumPath<String>,
) -> impl IntoResponse {
    // Note: Since we no longer generate JWTs, impersonation might need to be handled differently.
    // For now, we return a mock token if impersonation is truly needed, or we disable it.
    (StatusCode::NOT_IMPLEMENTED, Json(AuthResponse { success: false, message: "Impersonasi tidak didukung dengan otentikasi eksternal Supabase.".into(), user_id: None, role: None, token: None }))
}

async fn doctor_impersonate_handler(
    claims: UserClaims,
    State(_state): State<AppState>,
    AxumPath(_target_id): AxumPath<String>,
) -> impl IntoResponse {
    if claims.0.role != "dokter" {
        return (StatusCode::FORBIDDEN, Json(AuthResponse { success: false, message: "Hanya dokter yang dapat melakukan impersonasi".into(), user_id: None, role: None, token: None }));
    }
    (StatusCode::NOT_IMPLEMENTED, Json(AuthResponse { success: false, message: "Impersonasi tidak didukung dengan otentikasi eksternal Supabase.".into(), user_id: None, role: None, token: None }))
}

async fn get_patients_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let patients = sqlx::query!("SELECT id, first_name, last_name, date_of_birth, gender FROM patients")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "id": row.id,
                "name": format!("{} {}", row.first_name, row.last_name).trim().to_string(),
                "date_of_birth": row.date_of_birth,
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
    match sqlx::query!("UPDATE patients SET primary_doctor_id = $1 WHERE id = $2", req.doctor_id, patient_id)
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
    match sqlx::query!("UPDATE patients SET primary_doctor_id = NULL WHERE id = $1", patient_id)
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
    let patients = sqlx::query!(
        "SELECT p.id, p.first_name, p.last_name, a.profile_photo 
         FROM patients p 
         LEFT JOIN accounts a ON p.account_id = a.id 
         WHERE p.primary_doctor_id = $1", doctor_id
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

    let topic = format!("ecgrhythmia/{}/command", device_id);
    let clients = state.mqtt_clients.read().await;
    if let Some(client) = clients.get(&device_id) {
        if let Err(e) = client.clone().publish(topic, rumqttc::QoS::AtLeastOnce, false, cmd.command) {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"success": false, "message": format!("Gagal mengirim perintah: {}", e)})))
        } else {
            (StatusCode::OK, Json(serde_json::json!({"success": true})))
        }
    } else {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({"success": false, "message": "Perangkat tidak memiliki koneksi MQTT aktif"})))
    }
}

async fn frame_preregister_handler(
    State(state): State<AppState>,
    Json(req): Json<FrameRequest>,
) -> impl IntoResponse {
    match sqlx::query!("INSERT INTO frame_records (id, session_id, time_interval) VALUES ($1, $2, $3)", req.id, req.session_id, req.time_interval)
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
async fn get_sessions_from_db(filter_patient_id: Option<String>, pool: &PgPool) -> Vec<SessionRecord> {
    if let Some(pid) = filter_patient_id {
        sqlx::query!(
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name as patient_name, s.started_at, s.ended_at, s.file_path 
             FROM sessions s LEFT JOIN patients p ON s.patient_id = p.id 
             WHERE s.patient_id = $1 ORDER BY s.started_at DESC", pid
        ).fetch_all(pool).await.unwrap_or_default()
        .into_iter().map(|row| SessionRecord {
            id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
            started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path.unwrap_or_default()
        }).collect()
    } else {
        sqlx::query!(
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name as patient_name, s.started_at, s.ended_at, s.file_path 
             FROM sessions s LEFT JOIN patients p ON s.patient_id = p.id 
             ORDER BY s.started_at DESC"
        ).fetch_all(pool).await.unwrap_or_default()
        .into_iter().map(|row| SessionRecord {
            id: row.id, device_id: row.device_id, patient_id: Some(row.patient_id), patient_name: row.patient_name,
            started_at: row.started_at.to_rfc3339(), ended_at: row.ended_at.map(|d| d.to_rfc3339()), file_path: row.file_path.unwrap_or_default()
        }).collect()
    }
}

async fn get_devices_from_db(pool: &PgPool) -> Vec<DeviceRecord> {
    sqlx::query!(
        "SELECT id, name, mqtt_broker, mqtt_port, mqtt_topic, mqtt_username FROM devices"
    ).fetch_all(pool).await.unwrap_or_default()
    .into_iter().map(|row| DeviceRecord {
        id: row.id, name: row.name, mqtt_broker: row.mqtt_broker, mqtt_port: row.mqtt_port,
        mqtt_topic: row.mqtt_topic, mqtt_username: row.mqtt_username,
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
        "SELECT p.id, p.first_name || ' ' || p.last_name as name, a.role, COALESCE(a.status, 'Offline') as status, a.created_at, p.primary_doctor_id as connected_doctor_id, p.device_id as connected_device_id, a.profile_photo
         FROM patients p JOIN accounts a ON p.account_id = a.id
         UNION ALL
         SELECT d.id, d.first_name || ' ' || d.last_name as name, a.role, COALESCE(a.status, 'Offline') as status, a.created_at, NULL as connected_doctor_id, NULL as connected_device_id, a.profile_photo
         FROM doctors d JOIN accounts a ON d.account_id = a.id
         ORDER BY created_at DESC"
    ).fetch_all(pool).await.unwrap_or_default()
    .into_iter().map(|row| AdminUser {
        id: row.id.unwrap_or_default(), name: row.name.unwrap_or_default(), role: row.role.unwrap_or_default(), status: row.status.unwrap_or_default(), 
        registered_at: row.created_at.map(|t| t.to_string()), connected_doctor_id: row.connected_doctor_id, connected_device_id: row.connected_device_id, profile_photo: row.profile_photo
    }).collect()
}

async fn get_patient_profile(patient_id: String, pool: &PgPool) -> Option<PatientProfileResponse> {
    let patient_res = sqlx::query!(
        "SELECT p.id, p.first_name, p.last_name, p.date_of_birth, p.gender, p.primary_doctor_id, a.profile_photo, p.device_id
         FROM patients p LEFT JOIN accounts a ON p.account_id = a.id WHERE p.id = $1", patient_id
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
            id: patient_res.id, first_name: patient_res.first_name, last_name: patient_res.last_name, date_of_birth: patient_res.date_of_birth,
            gender: patient_res.gender.unwrap_or_default(), primary_doctor_id: patient_res.primary_doctor_id, profile_photo: patient_res.profile_photo, device_id: patient_res.device_id
        },
        doctor,
    })
}

async fn get_doctor_profile(doctor_id: String, pool: &PgPool) -> Option<DoctorProfileResponse> {
    let res = sqlx::query!(
        "SELECT d.id, d.first_name, d.last_name, a.email, a.role, a.profile_photo FROM doctors d LEFT JOIN accounts a ON d.account_id = a.id WHERE d.id = $1", doctor_id
    ).fetch_one(pool).await.ok()?;

    Some(DoctorProfileResponse {
        id: res.id, first_name: res.first_name, last_name: res.last_name,
        email: res.email, role: res.role, profile_photo: res.profile_photo
    })
}

async fn update_doctor_profile(doctor_id: &str, req: UpdateDoctorProfileRequest, pool: &PgPool, api_url: &str) -> Result<(), String> {
    let account_id = sqlx::query!("SELECT account_id FROM doctors WHERE id = $1", doctor_id)
        .fetch_one(pool).await.map_err(|_| "Dokter tidak ditemukan".to_string())?.account_id;

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

    sqlx::query!("UPDATE doctors SET first_name = $1, last_name = $2 WHERE id = $3", req.first_name, req.last_name, doctor_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    sqlx::query!("UPDATE accounts SET profile_photo = $1 WHERE id = $2", final_photo_url, account_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    Ok(())
}

async fn update_patient_profile(patient_id: &str, req: UpdatePatientProfileRequest, pool: &PgPool, api_url: &str) -> Result<(), String> {
    let account_id = sqlx::query!("SELECT account_id FROM patients WHERE id = $1", patient_id)
        .fetch_one(pool).await.map_err(|_| "Pasien tidak ditemukan".to_string())?.account_id;

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

    sqlx::query!("UPDATE patients SET first_name = $1, last_name = $2, date_of_birth = $3 WHERE id = $4", req.first_name, req.last_name, req.date_of_birth, patient_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    sqlx::query!("UPDATE accounts SET profile_photo = $1 WHERE id = $2", final_photo_url, account_id)
        .execute(pool).await.map_err(|e| e.to_string())?;

    Ok(())
}

fn read_jsonl_file(session_id: &str) -> String {
    let file_path = format!("records/{}.jsonl", session_id);
    if let Ok(contents) = fs::read_to_string(&file_path) {
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
        ])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT])
        .allow_credentials(true);

    Router::new()
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
        .route("/api/devices/:device_id/command", post(device_command_handler))
        .route("/api/devices/:device_id/assign", post(assign_device_handler))
        .route("/api/frames", post(frame_preregister_handler))
        .route("/api/frames/:id/session", put(frame_session_update_handler))
        .nest_service("/uploads", tower_http::services::ServeDir::new("uploads"))
        .layer(cors)
        .with_state(state)
}
