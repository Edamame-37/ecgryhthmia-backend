use super::{
    authorization::Identity,
    error::{ApiError, ApiResult},
    jwks::JwksCache,
    routes::AppState,
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    pub iss: String,
    pub aud: String,
    pub email: Option<String>,
    pub session_id: Option<String>,
}
#[derive(Clone)]
pub struct JwtVerifier {
    config: Arc<crate::config::AppConfig>,
    jwks: JwksCache,
}
impl JwtVerifier {
    pub fn new(config: Arc<crate::config::AppConfig>) -> Self {
        Self {
            jwks: JwksCache::new(format!("{}/.well-known/jwks.json", config.issuer())),
            config,
        }
    }
    pub async fn verify(&self, token: &str) -> ApiResult<Claims> {
        if token.len() > 16384 {
            return Err(ApiError::unauthorized());
        }
        let header = decode_header(token).map_err(|_| ApiError::unauthorized())?;
        let key = match header.alg {
            Algorithm::ES256 | Algorithm::RS256 => {
                self.jwks
                    .key(header.kid.as_deref().ok_or_else(ApiError::unauthorized)?)
                    .await?
            }
            Algorithm::HS256 if self.config.jwt_secret.len() >= 32 => {
                DecodingKey::from_secret(self.config.jwt_secret.as_bytes())
            }
            _ => return Err(ApiError::unauthorized()),
        };
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.config.issuer()]);
        validation.set_audience(&["authenticated"]);
        validation.set_required_spec_claims(&["sub", "exp", "iat", "iss", "aud"]);
        validation.leeway = 5;
        let claims = decode::<Claims>(token, &key, &validation)
            .map_err(|_| ApiError::unauthorized())?
            .claims;
        let now = chrono::Utc::now().timestamp() as u64;
        if claims.sub.is_empty()
            || claims.iat > now + 5
            || now.saturating_sub(claims.iat) > 3600
            || claims.exp <= claims.iat
        {
            return Err(ApiError::unauthorized());
        }
        Ok(claims)
    }
}
pub fn bearer(request: &Request) -> ApiResult<&str> {
    request
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty())
        .ok_or_else(ApiError::unauthorized)
}
pub async fn authenticate(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> ApiResult<Response> {
    let token = bearer(&request)?.to_owned();
    let claims = state.auth.verify(&token).await?;
    let impersonation = request
        .headers()
        .get("X-Impersonation-Id")
        .map(|value| value.to_str().map(str::to_string))
        .transpose()
        .map_err(|_| ApiError::forbidden())?;
    let stopping_impersonation = request.method() == axum::http::Method::DELETE
        && request.uri().path().starts_with("/api/impersonation/");
    let context = if stopping_impersonation {
        None
    } else {
        impersonation.as_deref()
    };
    let mut identity = authorize(&state, &claims, context).await?;
    identity.access_token = Some(token);
    let group = if request.uri().path().contains("/command") {
        "command"
    } else if request.uri().path().contains("upload") {
        "upload"
    } else {
        "api"
    };
    state
        .limits
        .check(
            &format!("{}:{group}", identity.actor_id),
            match group {
                "command" => 30,
                "upload" => 6,
                _ => 180,
            },
        )
        .await?;
    let path = request.uri().path();
    let heavy = path.contains("upload")
        || path.ends_with("ecg_paper")
        || (request.method() == axum::http::Method::PUT
            && (path.starts_with("/api/patients/") || path.starts_with("/api/doctors/")));
    let _permit = if heavy {
        Some(
            state
                .heavy_requests
                .clone()
                .try_acquire_owned()
                .map_err(|_| {
                    ApiError(
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        "Upload capacity reached; retry later".into(),
                    )
                })?,
        )
    } else {
        None
    };
    let limit = if heavy { 20 * 1024 * 1024 } else { 8192 };
    let (parts, body) = request.into_parts();
    let bytes = axum::body::to_bytes(body, limit).await.map_err(|_| {
        ApiError(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "Request body too large".into(),
        )
    })?;
    request = Request::from_parts(parts, axum::body::Body::from(bytes));
    if request.method() != axum::http::Method::GET {
        sqlx::query("INSERT INTO audit_events(actor_id,effective_id,action,resource_id) VALUES($1,$2,$3,$4)")
   .bind(&identity.actor_id).bind(&identity.account_id).bind(request.method().as_str()).bind(request.uri().path()).execute(&state.pool).await?;
    }
    request.extensions_mut().insert(identity);
    Ok(next.run(request).await)
}

pub async fn authorize(
    state: &AppState,
    claims: &Claims,
    impersonation: Option<&str>,
) -> ApiResult<Identity> {
    let now = chrono::Utc::now().timestamp() as u64;
    if claims.exp <= now || now.saturating_sub(claims.iat) > 3600 {
        return Err(ApiError::unauthorized());
    }
    let mut identity = Identity::load(&state.pool, claims).await?;
    validate_session(state, claims).await?;
    if let Some(id) = impersonation {
        identity = super::impersonation::resolve(state, &identity, id).await?;
        identity.impersonation_id = Some(id.to_string());
    }
    Ok(identity)
}

pub async fn validate_session(state: &AppState, claims: &Claims) -> ApiResult<()> {
    if state.config.require_auth_session {
        let session_id = claims
            .session_id
            .as_deref()
            .ok_or_else(ApiError::unauthorized)?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth.sessions WHERE id::text=$1 AND user_id::text=$2 AND (not_after IS NULL OR not_after>now()))").bind(session_id).bind(&claims.sub).fetch_one(&state.pool).await?;
        if !active {
            return Err(ApiError::unauthorized());
        }
    }
    Ok(())
}
