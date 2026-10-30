use crate::api::error::{ApiError, ApiResult};
use std::{env, path::PathBuf};

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host_ip: String,
    pub rest_port: u16,
    pub ws_port: u16,
    pub database_url: String,
    pub supabase_url: String,
    pub supabase_anon_key: String,
    pub supabase_service_key: String,
    pub jwt_secret: String,
    pub require_auth_session: bool,
    pub allowed_origins: Vec<String>,
    pub records_root: PathBuf,
    pub uploads_root: PathBuf,
    pub apply_migrations: bool,
}
impl AppConfig {
    pub fn load() -> ApiResult<Self> {
        dotenvy::dotenv().ok();
        let required = |name: &str| {
            env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or_else(|| ApiError::bad(format!("Missing {name}")))
        };
        let port = |name: &str, default: u16| -> ApiResult<u16> {
            env::var(name)
                .ok()
                .map(|s| {
                    s.parse()
                        .map_err(|_| ApiError::bad(format!("Invalid {name}")))
                })
                .unwrap_or(Ok(default))
        };
        let supabase_url = required("SUPABASE_URL")?.trim_end_matches('/').to_string();
        if !supabase_url.starts_with("https://") {
            return Err(ApiError::bad("SUPABASE_URL must use HTTPS"));
        }
        Ok(Self {
            host_ip: env::var("HOST_IP").unwrap_or_else(|_| "127.0.0.1".into()),
            rest_port: port("REST_PORT", 8081)?,
            ws_port: port("WS_PORT", 8080)?,
            database_url: required("DATABASE_URL")?,
            supabase_url,
            supabase_anon_key: required("SUPABASE_ANON_KEY")?,
            supabase_service_key: env::var("SUPABASE_SERVICE_ROLE_KEY").unwrap_or_default(),
            jwt_secret: env::var("SUPABASE_JWT_SECRET").unwrap_or_default(),
            require_auth_session: true,
            allowed_origins: env::var("ALLOWED_ORIGINS")
                .unwrap_or_else(|_| "http://localhost:5173,http://127.0.0.1:5173".into())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            records_root: PathBuf::from(
                env::var("RECORDS_ROOT").unwrap_or_else(|_| "records".into()),
            ),
            uploads_root: PathBuf::from(
                env::var("UPLOADS_ROOT").unwrap_or_else(|_| "uploads".into()),
            ),
            apply_migrations: env::var("APPLY_MIGRATIONS").is_ok_and(|v| v == "true"),
        })
    }
    pub fn issuer(&self) -> String {
        format!("{}/auth/v1", self.supabase_url)
    }
}
