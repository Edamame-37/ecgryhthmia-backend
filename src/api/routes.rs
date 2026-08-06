use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::thread;
use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};
use rusqlite::params;
use chrono::Utc;
use base64::{Engine as _, engine::general_purpose::STANDARD as base64_engine};
use crate::db::sqlite::generate_custom_id;
use bcrypt::{hash, verify, DEFAULT_COST};
use jsonwebtoken::{encode, decode, Header, Algorithm, Validation, EncodingKey, DecodingKey};
use dotenvy::dotenv;
use std::env;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    role: String,
    exp: usize,
}

#[derive(Serialize)]
pub struct SessionRecord {
    pub id: String, // as session_id
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
    pub mac: Option<String>,
    pub battery: Option<i64>,
    pub status: Option<String>,
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
    pub name: String,
    pub role: String,
    pub status: String,
    pub registered_at: String,
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
pub struct RegisterRequest {
    pub role: String,
    pub email: String,
    pub password: String,
    pub first_name: String,
    pub last_name: String,
    pub date_of_birth: Option<String>,
    pub gender: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
    pub role: String,
}

#[derive(Serialize)]
pub struct AuthResponse {
    pub success: bool,
    pub message: String,
    pub user_id: Option<String>,
    pub role: Option<String>,
    pub token: Option<String>,
}

fn create_jwt(account_id: &str, role: &str) -> String {
    dotenv().ok();
    let secret = env::var("JWT_SECRET").unwrap_or_else(|_| "secret".to_string());
    let expiration = Utc::now().checked_add_signed(chrono::Duration::hours(2)).expect("valid timestamp").timestamp();
    
    let claims = Claims {
        sub: account_id.to_owned(),
        role: role.to_owned(),
        exp: expiration as usize,
    };
    
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes())).unwrap_or_default()
}

fn validate_jwt(token: &str) -> Option<Claims> {
    dotenv().ok();
    let secret = env::var("JWT_SECRET").unwrap_or_else(|_| "secret".to_string());
    
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    
    match decode::<Claims>(token, &DecodingKey::from_secret(secret.as_bytes()), &validation) {
        Ok(token_data) => Some(token_data.claims),
        Err(_) => None,
    }
}

#[derive(Deserialize)]
pub struct ConfirmationRequest {
    pub confirmation: i32,
    pub doc_classification: String,
}

#[derive(Serialize)]
pub struct ConfirmationResponse {
    pub success: bool,
    pub message: String,
}

pub fn start_rest_api(port: &str, mqtt_client: rumqttc::Client) {
    let address = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&address).expect("Gagal mem-bind port REST API");
    
    println!("[REST API] Server HTTP berjalan di http://{}/api", address);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let client_clone = mqtt_client.clone();
                thread::spawn(move || {
                    handle_http_client(stream, client_clone);
                });
            }
            Err(e) => eprintln!("[REST API] Error menerima koneksi: {}", e),
        }
    }
}

fn handle_http_client(mut stream: TcpStream, mqtt_client: rumqttc::Client) {
    let mut buffer = [0; 4096];
    let mut request_data = Vec::new();
    let mut content_length = 0;
    let mut headers_parsed = false;
    let mut header_end = 0;
    let mut auth_token = String::new();

    if let Ok(bytes_read) = stream.read(&mut buffer) {
        if bytes_read == 0 { return; }
        request_data.extend_from_slice(&buffer[..bytes_read]);

        if let Some(pos) = request_data.windows(4).position(|w| w == b"\r\n\r\n") {
            headers_parsed = true;
            header_end = pos + 4;
            
            let headers = String::from_utf8_lossy(&request_data[..pos]);
            for line in headers.lines() {
                let lower = line.to_lowercase();
                if lower.starts_with("content-length:") {
                    if let Ok(len) = lower[15..].trim().parse::<usize>() {
                        content_length = len;
                    }
                } else if lower.starts_with("authorization: bearer ") {
                    auth_token = line[22..].trim().to_string();
                }
            }
        }
    } else {
        return;
    }

    if headers_parsed && content_length > 0 {
        let mut body_read = request_data.len() - header_end;
        while body_read < content_length {
            let mut chunk = [0; 8192];
            if let Ok(n) = stream.read(&mut chunk) {
                if n == 0 { break; }
                request_data.extend_from_slice(&chunk[..n]);
                body_read += n;
            } else {
                break;
            }
        }
    }

    let request_str = String::from_utf8_lossy(&request_data);
    let cors_headers = "Access-Control-Allow-Origin: http://localhost:5173\r\n\
                        Access-Control-Allow-Methods: GET, POST, PUT, OPTIONS\r\n\
                        Access-Control-Allow-Headers: Content-Type, Authorization\r\n";

    if request_str.starts_with("OPTIONS") {
        let response = format!("HTTP/1.1 204 No Content\r\n{}\r\n", cors_headers);
        let _ = stream.write_all(response.as_bytes());
        return;
    }

    let first_line = request_str.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 { return; }
    let method = parts[0];
    let path = parts[1];

    let request_body = if headers_parsed && header_end < request_str.len() {
        &request_str[header_end..]
    } else {
        ""
    };

    let request_body = request_body.trim_matches(char::from(0));

        let (status, response_body) = if method == "POST" && path == "/api/auth/register" {
            if let Ok(req) = serde_json::from_str::<RegisterRequest>(request_body) {
                match handle_register(req) {
                    Ok(res) => ("200 OK", serde_json::to_string(&res).unwrap()),
                    Err(msg) => {
                        let res = AuthResponse { success: false, message: msg, user_id: None, role: None, token: None };
                        ("400 Bad Request", serde_json::to_string(&res).unwrap())
                    }
                }
            } else {
                let res = AuthResponse { success: false, message: "Format request tidak valid".to_string(), user_id: None, role: None, token: None };
                ("400 Bad Request", serde_json::to_string(&res).unwrap())
            }
        } else if method == "POST" && path == "/api/auth/login" {
            if let Ok(req) = serde_json::from_str::<LoginRequest>(request_body) {
                match handle_login(req) {
                    Ok(res) => ("200 OK", serde_json::to_string(&res).unwrap()),
                    Err(msg) => {
                        let res = AuthResponse { success: false, message: msg, user_id: None, role: None, token: None };
                        ("401 Unauthorized", serde_json::to_string(&res).unwrap())
                    }
                }
            } else {
                let res = AuthResponse { success: false, message: "Format request tidak valid".to_string(), user_id: None, role: None, token: None };
                ("400 Bad Request", serde_json::to_string(&res).unwrap())
            }
        } else if method == "GET" && path == "/api/sessions" {
            let sessions = get_sessions_from_db(None);
            ("200 OK", serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()))
        } else if method == "GET" && path == "/api/devices" {
            let devices = get_devices_from_db();
            ("200 OK", serde_json::to_string(&devices).unwrap_or_else(|_| "[]".to_string()))
        } else if method == "GET" && path == "/api/admin/stats" {
            if let Some(claims) = validate_jwt(&auth_token) {
                if claims.role == "admin" {
                    let stats = get_admin_stats();
                    ("200 OK", serde_json::to_string(&stats).unwrap_or_else(|_| "{}".to_string()))
                } else {
                    ("403 Forbidden", "{\"error\": \"Akses ditolak\"}".to_string())
                }
            } else {
                ("401 Unauthorized", "{\"error\": \"Sesi tidak valid\"}".to_string())
            }
        } else if method == "GET" && path == "/api/admin/users" {
            if let Some(claims) = validate_jwt(&auth_token) {
                if claims.role == "admin" {
                    let users = get_admin_users();
                    ("200 OK", serde_json::to_string(&users).unwrap_or_else(|_| "[]".to_string()))
                } else {
                    ("403 Forbidden", "{\"error\": \"Akses ditolak\"}".to_string())
                }
            } else {
                ("401 Unauthorized", "{\"error\": \"Sesi tidak valid\"}".to_string())
            }
        } else if method == "GET" && path == "/api/admin/devices" {
            let devices = get_devices_from_db();
            ("200 OK", serde_json::to_string(&devices).unwrap_or_else(|_| "[]".to_string()))
        } else if method == "GET" && path == "/api/patients" {
            let conn = crate::db::sqlite::open_encrypted_db("database.db").unwrap();
            let mut stmt = conn.prepare("SELECT id, first_name, last_name, date_of_birth, gender FROM patients").unwrap();
            let patients_iter = stmt.query_map([], |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "name": format!("{} {}", row.get::<_, String>(1).unwrap_or_default(), row.get::<_, String>(2).unwrap_or_default()).trim().to_string(),
                    "date_of_birth": row.get::<_, String>(3).unwrap_or_default(),
                    "gender": row.get::<_, String>(4).unwrap_or_default()
                }))
            }).unwrap();
            let mut patients_list = Vec::new();
            for p in patients_iter {
                if let Ok(p) = p {
                    patients_list.push(p);
                }
            }
            ("200 OK", serde_json::to_string(&patients_list).unwrap_or_else(|_| "[]".to_string()))
        } else if method == "GET" && path.starts_with("/api/patients/") && path.ends_with("/sessions") {
            let patient_id = path.replace("/api/patients/", "").replace("/sessions", "");
            if !patient_id.is_empty() {
                let sessions = get_sessions_from_db(Some(patient_id));
                ("200 OK", serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()))
            } else {
                ("400 Bad Request", "[]".to_string())
            }
        } else if method == "GET" && path.starts_with("/api/patients/") {
            let patient_id = path.replace("/api/patients/", "");
            if !patient_id.is_empty() {
                if let Some(profile) = get_patient_profile(patient_id) {
                    ("200 OK", serde_json::to_string(&profile).unwrap())
                } else {
                    ("404 Not Found", "{}".to_string())
                }
            } else {
                ("400 Bad Request", "{}".to_string())
            }
        } else if method == "GET" && path.starts_with("/api/doctors/") {
            let doctor_id = path.replace("/api/doctors/", "");
            if !doctor_id.is_empty() {
                if let Some(profile) = get_doctor_profile(doctor_id) {
                    ("200 OK", serde_json::to_string(&profile).unwrap())
                } else {
                    ("404 Not Found", "{}".to_string())
                }
            } else {
                ("400 Bad Request", "{}".to_string())
            }
        } else if method == "PUT" && path.starts_with("/api/doctors/") {
            let doctor_id = path.replace("/api/doctors/", "");
            if !doctor_id.is_empty() {
                if let Ok(req) = serde_json::from_str::<UpdateDoctorProfileRequest>(request_body) {
                    match update_doctor_profile(&doctor_id, req) {
                        Ok(_) => ("200 OK", "{\"success\":true}".to_string()),
                        Err(e) => ("500 Internal Server Error", format!("{{\"success\":false,\"message\":\"{}\"}}", e)),
                    }
                } else {
                    ("400 Bad Request", "{\"success\":false,\"message\":\"Format request tidak valid\"}".to_string())
                }
            } else {
                ("400 Bad Request", "{\"success\":false,\"message\":\"ID dokter tidak valid\"}".to_string())
            }
        } else if method == "PUT" && path.starts_with("/api/patients/") {
            let patient_id = path.replace("/api/patients/", "");
            if !patient_id.is_empty() {
                if let Ok(req) = serde_json::from_str::<UpdatePatientProfileRequest>(request_body) {
                    match update_patient_profile(&patient_id, req) {
                        Ok(_) => ("200 OK", "{\"success\":true}".to_string()),
                        Err(e) => ("500 Internal Server Error", format!("{{\"success\":false,\"message\":\"{}\"}}", e)),
                    }
                } else {
                    ("400 Bad Request", "{\"success\":false,\"message\":\"Format request tidak valid\"}".to_string())
                }
            } else {
                ("400 Bad Request", "{\"success\":false,\"message\":\"ID pasien tidak valid\"}".to_string())
            }
        } else if method == "GET" && path.starts_with("/uploads/") {
            let file_path = path.trim_start_matches('/');
            if let Ok(contents) = fs::read(file_path) {
                let mime_type = if path.ends_with(".jpg") || path.ends_with(".jpeg") {
                    "image/jpeg"
                } else if path.ends_with(".png") {
                    "image/png"
                } else {
                    "application/octet-stream"
                };
                let response = format!("HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n", mime_type, contents.len());
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.write_all(&contents);
                return;
            } else {
                let response = "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
                let _ = stream.write_all(response.as_bytes());
                return;
            }
        } else if method == "GET" && path.starts_with("/api/records/") {
            let session_id = path.replace("/api/records/", "");
            ("200 OK", read_jsonl_file(&session_id))
        } else if method == "POST" && path.starts_with("/api/devices/") && path.ends_with("/command") {
            let device_id = path.replace("/api/devices/", "").replace("/command", "");
            #[derive(Deserialize)]
            struct DeviceCommand {
                command: String,
                patient_id: Option<String>,
            }
            if let Ok(cmd) = serde_json::from_str::<DeviceCommand>(request_body) {
                if let Some(pid) = cmd.patient_id {
                    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
                        let _ = conn.execute("UPDATE devices SET assigned_to = ?1 WHERE name = ?2", params![pid, device_id]);
                    }
                }
                let topic = format!("ecgrhythmia/{}/command", device_id);
                if let Err(e) = mqtt_client.clone().publish(topic, rumqttc::QoS::AtLeastOnce, false, cmd.command) {
                    ("500 Internal Server Error", format!("{{\"success\":false,\"message\":\"Gagal mengirim perintah: {}\"}}", e))
                } else {
                    ("200 OK", "{\"success\":true}".to_string())
                }
            } else {
                ("400 Bad Request", "{\"success\":false,\"message\":\"Format perintah tidak valid\"}".to_string())
            }
        } else if method == "POST" && path.starts_with("/api/sessions/") && path.ends_with("/confirmation") {
            let session_id = path.replace("/api/sessions/", "").replace("/confirmation", "");
            if let Ok(req) = serde_json::from_str::<ConfirmationRequest>(request_body) {
                match handle_confirmation(&session_id, req) {
                    Ok(res) => ("200 OK", serde_json::to_string(&res).unwrap()),
                    Err(msg) => {
                        let res = ConfirmationResponse { success: false, message: msg };
                        ("500 Internal Server Error", serde_json::to_string(&res).unwrap())
                    }
                }
            } else {
                let res = ConfirmationResponse { success: false, message: "Format request tidak valid".to_string() };
                ("400 Bad Request", serde_json::to_string(&res).unwrap())
            }
        } else {
            ("404 Not Found", "{}".to_string())
        };

        let response = format!(
            "HTTP/1.1 {}\r\n\
            Content-Type: application/json\r\n\
            {}\
            Content-Length: {}\r\n\
            \r\n\
            {}",
            status, cors_headers, response_body.len(), response_body
        );
        let _ = stream.write_all(response.as_bytes());
}

fn handle_register(req: RegisterRequest) -> Result<AuthResponse, String> {
    let conn = crate::db::sqlite::open_encrypted_db("database.db").map_err(|e| e.to_string())?;
    
    // Check existing email
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM accounts WHERE email = ?1",
        params![req.email],
        |row| row.get(0)
    ).unwrap_or(0);

    if count > 0 {
        return Err("Email sudah terdaftar".to_string());
    }

    let now = Utc::now().to_rfc3339();
    
    let account_id = generate_custom_id(&conn, "accounts", "acc");
    let hashed_password = hash(&req.password, DEFAULT_COST).unwrap_or(req.password.clone());

    conn.execute(
        "INSERT INTO accounts (id, email, password_hash, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![account_id, req.email, hashed_password, req.role, now]
    ).map_err(|e| e.to_string())?;

    if req.role == "pasien" {
        let dob = req.date_of_birth.unwrap_or_else(|| "2000-01-01".to_string());
        let gender = req.gender.unwrap_or_else(|| "U".to_string());
        let patient_id = generate_custom_id(&conn, "patients", "pat");
        conn.execute(
            "INSERT INTO patients (id, account_id, first_name, last_name, date_of_birth, gender) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![patient_id, account_id, req.first_name, req.last_name, dob, gender]
        ).map_err(|e| e.to_string())?;
    } else if req.role == "dokter" {
        let doctor_id = generate_custom_id(&conn, "doctors", "doc");
        conn.execute(
            "INSERT INTO doctors (id, account_id, first_name, last_name) VALUES (?1, ?2, ?3, ?4)",
            params![doctor_id, account_id, req.first_name, req.last_name]
        ).map_err(|e| e.to_string())?;
    } else {
        return Err("Role tidak valid".to_string());
    }

    Ok(AuthResponse {
        success: true,
        message: "Registrasi berhasil".to_string(),
        user_id: None,
        role: None,
        token: None,
    })
}

fn handle_login(req: LoginRequest) -> Result<AuthResponse, String> {
    let conn = crate::db::sqlite::open_encrypted_db("database.db").map_err(|e| e.to_string())?;
    
    let result = conn.query_row(
        "SELECT id, role, password_hash FROM accounts WHERE email = ?1",
        params![req.email],
        |row| {
            let id: String = row.get(0)?;
            let role: String = row.get(1)?;
            let password_hash: String = row.get(2)?;
            Ok((id, role, password_hash))
        }
    );

    match result {
        Ok((account_id, role, password_hash)) => {
            let password_match = verify(&req.password, &password_hash).unwrap_or(false) || req.password == password_hash;
            if password_match && req.role == role {
                // Find specific user ID (patient.id or doctor.id) based on account_id
                let specific_id: Option<String> = if role == "pasien" {
                    conn.query_row("SELECT id FROM patients WHERE account_id = ?1", params![account_id], |row| row.get(0)).ok()
                } else if role == "dokter" {
                    conn.query_row("SELECT id FROM doctors WHERE account_id = ?1", params![account_id], |row| row.get(0)).ok()
                } else {
                    Some(account_id.clone())
                };

                let token = create_jwt(&account_id, &role);

                Ok(AuthResponse {
                    success: true,
                    message: "Login berhasil".to_string(),
                    user_id: specific_id,
                    role: Some(role),
                    token: Some(token),
                })
            } else {
                Err("Password atau role tidak cocok".to_string())
            }
        },
        Err(_) => Err("Email tidak ditemukan".to_string())
    }
}

fn handle_confirmation(session_id: &str, req: ConfirmationRequest) -> Result<ConfirmationResponse, String> {
    let conn = crate::db::sqlite::open_encrypted_db("database.db").map_err(|e| e.to_string())?;
    
    let result = conn.execute(
        "UPDATE sessions SET confirmation = ?1, doc_classification = ?2 WHERE id = ?3",
        params![req.confirmation, req.doc_classification, session_id]
    );

    match result {
        Ok(rows_affected) => {
            if rows_affected > 0 {
                Ok(ConfirmationResponse {
                    success: true,
                    message: "Konfirmasi berhasil disimpan".to_string(),
                })
            } else {
                Err("Session ID tidak ditemukan".to_string())
            }
        },
        Err(e) => Err(format!("Gagal menyimpan konfirmasi: {}", e))
    }
}

fn get_sessions_from_db(filter_patient_id: Option<String>) -> Vec<SessionRecord> {
    let mut sessions = Vec::new();
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        let query = if filter_patient_id.is_some() {
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name, s.started_at, s.ended_at, s.file_path 
             FROM sessions s 
             LEFT JOIN patients p ON s.patient_id = p.id 
             WHERE s.patient_id = ?1
             ORDER BY s.started_at DESC"
        } else {
            "SELECT s.id, s.device_id, s.patient_id, p.first_name || ' ' || p.last_name, s.started_at, s.ended_at, s.file_path 
             FROM sessions s 
             LEFT JOIN patients p ON s.patient_id = p.id 
             ORDER BY s.started_at DESC"
        };

        let mut stmt = match conn.prepare(query) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Query error: {}", e);
                return sessions;
            }
        };
        
        if let Some(pid) = filter_patient_id {
            let session_iter = stmt.query_map([pid], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    device_id: row.get(1)?,
                    patient_id: row.get(2)?,
                    patient_name: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    file_path: row.get(6)?,
                })
            });
            if let Ok(iter) = session_iter {
                for session in iter {
                    if let Ok(s) = session {
                        sessions.push(s);
                    }
                }
            }
        } else {
            let session_iter = stmt.query_map([], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    device_id: row.get(1)?,
                    patient_id: row.get(2)?,
                    patient_name: row.get(3)?,
                    started_at: row.get(4)?,
                    ended_at: row.get(5)?,
                    file_path: row.get(6)?,
                })
            });
            if let Ok(iter) = session_iter {
                for session in iter {
                    if let Ok(s) = session {
                        sessions.push(s);
                    }
                }
            }
        }
    }
    sessions
}

fn get_devices_from_db() -> Vec<DeviceRecord> {
    let mut devices = Vec::new();
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        if let Ok(mut stmt) = conn.prepare("SELECT id, name, mac, battery, status, assigned_to FROM devices") {
            if let Ok(device_iter) = stmt.query_map([], |row| {
                Ok(DeviceRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    mac: row.get(2)?,
                    battery: row.get(3)?,
                    status: row.get(4)?,
                    assigned_to: row.get(5)?,
                })
            }) {
                for device in device_iter {
                    if let Ok(d) = device {
                        devices.push(d);
                    }
                }
            }
        }
    }
    devices
}

fn get_admin_stats() -> AdminStats {
    let mut stats = AdminStats {
        total_patients: 0,
        total_doctors: 0,
        active_devices: 0,
        critical_alerts: 0,
    };
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        stats.total_patients = conn.query_row("SELECT COUNT(*) FROM patients", [], |row| row.get(0)).unwrap_or(0);
        stats.total_doctors = conn.query_row("SELECT COUNT(*) FROM doctors", [], |row| row.get(0)).unwrap_or(0);
        stats.active_devices = conn.query_row("SELECT COUNT(*) FROM devices WHERE status = 'Active'", [], |row| row.get(0)).unwrap_or(0);
        
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let today_prefix = format!("{}%", today);
        
        if let Ok(mut stmt) = conn.prepare("SELECT file_path FROM sessions WHERE started_at LIKE ?1") {
            let mut critical_count = 0;
            if let Ok(paths_iter) = stmt.query_map([&today_prefix], |row| row.get::<_, String>(0)) {
                for path in paths_iter {
                    if let Ok(file_path) = path {
                        if let Ok(contents) = fs::read_to_string(&file_path) {
                            for line in contents.lines() {
                                if !line.contains("\"label\":\"Normal\"") && line.contains("\"label\":") {
                                    critical_count += 1;
                                }
                            }
                        }
                    }
                }
            }
            stats.critical_alerts = critical_count;
        }
    }
    stats
}

fn get_admin_users() -> Vec<AdminUser> {
    let mut users = Vec::new();
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        let query = "
            SELECT p.id, p.first_name || ' ' || p.last_name, a.role, IFNULL(a.status, 'Offline'), a.created_at
            FROM patients p
            JOIN accounts a ON p.account_id = a.id
            UNION ALL
            SELECT d.id, d.first_name || ' ' || d.last_name, a.role, IFNULL(a.status, 'Offline'), a.created_at
            FROM doctors d
            JOIN accounts a ON d.account_id = a.id
            ORDER BY created_at DESC
        ";
        if let Ok(mut stmt) = conn.prepare(query) {
            if let Ok(user_iter) = stmt.query_map([], |row| {
                Ok(AdminUser {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                    status: row.get(3)?,
                    registered_at: row.get(4)?,
                })
            }) {
                for user in user_iter {
                    if let Ok(u) = user {
                        users.push(u);
                    }
                }
            }
        }
    }
    users
}

fn get_patient_profile(patient_id: String) -> Option<PatientProfileResponse> {
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        // Get patient
        let mut stmt = conn.prepare("
            SELECT p.id, p.first_name, p.last_name, p.date_of_birth, p.gender, p.primary_doctor_id, a.profile_photo
            FROM patients p
            LEFT JOIN accounts a ON p.account_id = a.id
            WHERE p.id = ?1
        ").ok()?;
        
        let mut patient_iter = stmt.query_map([patient_id], |row| {
            Ok(PatientRecord {
                id: row.get(0)?,
                first_name: row.get(1)?,
                last_name: row.get(2)?,
                date_of_birth: row.get(3)?,
                gender: row.get(4)?,
                primary_doctor_id: row.get(5)?,
                profile_photo: row.get(6)?,
            })
        }).ok()?;
        
        if let Some(Ok(patient)) = patient_iter.next() {
            let mut doctor = None;
            if let Some(ref doc_id) = patient.primary_doctor_id {
                if let Ok(mut doc_stmt) = conn.prepare("
                    SELECT d.id, d.first_name, d.last_name, a.profile_photo
                    FROM doctors d
                    LEFT JOIN accounts a ON d.account_id = a.id
                    WHERE d.id = ?1
                ") {
                    if let Ok(mut doc_iter) = doc_stmt.query_map([doc_id], |row| {
                        Ok(DoctorRecord {
                            id: row.get(0)?,
                            first_name: row.get(1)?,
                            last_name: row.get(2)?,
                            profile_photo: row.get(3)?,
                        })
                    }) {
                        if let Some(Ok(doc)) = doc_iter.next() {
                            doctor = Some(doc);
                        }
                    }
                }
            }
            
            return Some(PatientProfileResponse {
                patient,
                doctor,
            });
        }
    }
    None
}

fn get_doctor_profile(doctor_id: String) -> Option<DoctorProfileResponse> {
    if let Ok(conn) = crate::db::sqlite::open_encrypted_db("database.db") {
        let mut stmt = conn.prepare("
            SELECT d.id, d.first_name, d.last_name, a.email, a.role, a.profile_photo
            FROM doctors d
            LEFT JOIN accounts a ON d.account_id = a.id
            WHERE d.id = ?1
        ").ok()?;
        
        let mut doctor_iter = stmt.query_map([doctor_id], |row| {
            Ok(DoctorProfileResponse {
                id: row.get(0)?,
                first_name: row.get(1)?,
                last_name: row.get(2)?,
                email: row.get(3)?,
                role: row.get(4)?,
                profile_photo: row.get(5)?,
            })
        }).ok()?;
        
        if let Some(Ok(doctor)) = doctor_iter.next() {
            return Some(doctor);
        }
    }
    None
}

fn update_doctor_profile(doctor_id: &str, req: UpdateDoctorProfileRequest) -> Result<(), String> {
    let conn = crate::db::sqlite::open_encrypted_db("database.db").map_err(|e| e.to_string())?;
    let account_id: String = conn.query_row(
        "SELECT account_id FROM doctors WHERE id = ?1",
        params![doctor_id],
        |row| row.get(0)
    ).map_err(|_| "Dokter tidak ditemukan".to_string())?;

    let mut final_photo_url = req.profile_photo.filter(|s| !s.is_empty());

    if let Some(ref photo_data) = final_photo_url {
        if photo_data.starts_with("data:image/") {
            if let Some(comma_pos) = photo_data.find(',') {
                let base64_str = &photo_data[comma_pos + 1..];
                if let Ok(image_bytes) = base64_engine.decode(base64_str) {
                    let uploads_dir = Path::new("uploads/profiles");
                    if !uploads_dir.exists() {
                        let _ = fs::create_dir_all(uploads_dir);
                    }
                    let filename = format!("{}_{}.jpg", doctor_id, Utc::now().timestamp());
                    let filepath = uploads_dir.join(&filename);
                    if fs::write(&filepath, image_bytes).is_ok() {
                        final_photo_url = Some(format!("http://127.0.0.1:8081/uploads/profiles/{}", filename));
                    }
                }
            }
        }
    }

    conn.execute(
        "UPDATE doctors SET first_name = ?1, last_name = ?2 WHERE id = ?3",
        params![req.first_name, req.last_name, doctor_id]
    ).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE accounts SET profile_photo = ?1 WHERE id = ?2",
        params![final_photo_url, account_id]
    ).map_err(|e| e.to_string())?;

    Ok(())
}

fn update_patient_profile(patient_id: &str, req: UpdatePatientProfileRequest) -> Result<(), String> {
    let conn = crate::db::sqlite::open_encrypted_db("database.db").map_err(|e| e.to_string())?;
    let account_id: String = conn.query_row(
        "SELECT account_id FROM patients WHERE id = ?1",
        params![patient_id],
        |row| row.get(0)
    ).map_err(|_| "Pasien tidak ditemukan".to_string())?;

    let mut final_photo_url = req.profile_photo.filter(|s| !s.is_empty());

    if let Some(ref photo_data) = final_photo_url {
        if photo_data.starts_with("data:image/") {
            if let Some(comma_pos) = photo_data.find(',') {
                let base64_str = &photo_data[comma_pos + 1..];
                if let Ok(image_bytes) = base64_engine.decode(base64_str) {
                    let uploads_dir = Path::new("uploads/profiles");
                    if !uploads_dir.exists() {
                        let _ = fs::create_dir_all(uploads_dir);
                    }
                    let filename = format!("{}_{}.jpg", patient_id, Utc::now().timestamp());
                    let filepath = uploads_dir.join(&filename);
                    if fs::write(&filepath, image_bytes).is_ok() {
                        final_photo_url = Some(format!("http://127.0.0.1:8081/uploads/profiles/{}", filename));
                    }
                }
            }
        }
    }

    conn.execute(
        "UPDATE patients SET first_name = ?1, last_name = ?2, date_of_birth = ?3 WHERE id = ?4",
        params![req.first_name, req.last_name, req.date_of_birth, patient_id]
    ).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE accounts SET profile_photo = ?1 WHERE id = ?2",
        params![final_photo_url, account_id]
    ).map_err(|e| e.to_string())?;

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