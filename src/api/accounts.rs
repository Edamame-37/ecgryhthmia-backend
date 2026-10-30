use super::{
    authorization::Identity,
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{
    extract::{Request, State},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/me", get(me))
        .route("/api/auth/register", post(admin_register))
        .route("/api/admin/users", get(users))
        .route("/api/admin/stats", get(stats))
}
async fn me(Extension(identity): Extension<Identity>) -> Json<Value> {
    Json(
        json!({"success":true,"user_id":identity.account_id,"auth_id":identity.auth_id,"account_id":identity.account_id,"profile_id":identity.profile_id,"role":identity.role,"actor_id":identity.actor_id}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    pub role: String,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub age: Option<i32>,
    pub gender: Option<String>,
    pub date_of_birth: Option<chrono::NaiveDate>,
}
impl ProfileInput {
    pub fn validate(&self) -> ApiResult<()> {
        if !matches!(self.role.as_str(), "pasien" | "dokter")
            || self.first_name.trim().is_empty()
            || self.first_name.len() > 100
            || self.last_name.len() > 100
            || self.email.len() > 254
            || !self.email.contains('@')
            || self.age.is_some_and(|n| !(0..=130).contains(&n))
        {
            return Err(ApiError::bad("Invalid profile"));
        }
        if self
            .date_of_birth
            .is_some_and(|date| date > chrono::Utc::now().date_naive())
        {
            return Err(ApiError::bad("Birth date is in the future"));
        }
        Ok(())
    }
}
pub async fn save_profile(
    state: &AppState,
    auth_id: &str,
    input: &ProfileInput,
) -> ApiResult<String> {
    input.validate()?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("provision:{auth_id}"))
        .execute(&mut *transaction)
        .await?;
    let existing: Option<(String, String)> =
        sqlx::query_as("SELECT id,role FROM accounts WHERE auth_id=$1 FOR UPDATE")
            .bind(auth_id)
            .fetch_optional(&mut *transaction)
            .await?;
    let account_id = if let Some((id, role)) = existing {
        if role != input.role {
            return Err(ApiError::conflict("Account role already provisioned"));
        }
        id
    } else {
        let id = crate::db::postgres::new_id("acc");
        sqlx::query("INSERT INTO accounts(id,auth_id,email,role) VALUES($1,$2,$3,$4)")
            .bind(&id)
            .bind(auth_id)
            .bind(input.email.trim().to_lowercase())
            .bind(&input.role)
            .execute(&mut *transaction)
            .await?;
        id
    };
    if input.role == "pasien" {
        let profiles: i64 = sqlx::query_scalar("SELECT count(*) FROM patients WHERE account_id=$1")
            .bind(&account_id)
            .fetch_one(&mut *transaction)
            .await?;
        if profiles > 1 {
            return Err(ApiError::conflict(
                "Account has multiple patient profiles; administrator review is required",
            ));
        }
        sqlx::query("INSERT INTO patients(id,account_id,first_name,last_name,age,gender,date_of_birth) SELECT $1,$2,$3,$4,$5,$6,$7 WHERE NOT EXISTS(SELECT 1 FROM patients WHERE account_id=$2)")
   .bind(crate::db::postgres::new_id("pat")).bind(&account_id).bind(input.first_name.trim()).bind(input.last_name.trim()).bind(input.age.unwrap_or(0)).bind(&input.gender).bind(input.date_of_birth).execute(&mut *transaction).await?;
    } else {
        sqlx::query("INSERT INTO doctors(id,account_id,first_name,last_name) VALUES($1,$2,$3,$4) ON CONFLICT(account_id) DO NOTHING")
   .bind(crate::db::postgres::new_id("doc")).bind(&account_id).bind(input.first_name.trim()).bind(input.last_name.trim()).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(account_id)
}
pub async fn register_profile(
    State(state): State<AppState>,
    request: Request,
) -> ApiResult<Json<Value>> {
    let token = super::auth::bearer(&request)?.to_string();
    let claims = state.auth.verify(&token).await?;
    super::auth::validate_session(&state, &claims).await?;
    state
        .limits
        .check(&format!("provision:{}", claims.sub), 5)
        .await?;
    let bytes = axum::body::to_bytes(request.into_body(), 8192)
        .await
        .map_err(|_| ApiError::bad("Profile too large"))?;
    let profile: ProfileInput =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Invalid profile"))?;
    if claims.email.as_ref().map(|email| email.to_lowercase())
        != Some(profile.email.trim().to_lowercase())
    {
        return Err(ApiError::forbidden());
    }
    let account_id = save_profile(&state, &claims.sub, &profile).await?;
    Ok(Json(json!({"success":true,"account_id":account_id})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminInput {
    email: String,
    password: String,
    role: String,
    first_name: String,
    last_name: String,
    age: Option<i32>,
    gender: Option<String>,
    date_of_birth: Option<chrono::NaiveDate>,
}
async fn admin_register(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(input): Json<AdminInput>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    if input.password.len() < 12 || input.password.len() > 128 {
        return Err(ApiError::bad("Password must contain 12 to 128 characters"));
    }
    let profile = ProfileInput {
        email: input.email,
        role: input.role,
        first_name: input.first_name,
        last_name: input.last_name,
        age: input.age,
        gender: input.gender,
        date_of_birth: input.date_of_birth,
    };
    create_auth_profile(&state, profile, &input.password).await
}
async fn create_auth_profile(
    state: &AppState,
    profile: ProfileInput,
    password: &str,
) -> ApiResult<Json<Value>> {
    profile.validate()?;
    if state.config.supabase_service_key.is_empty() {
        return Err(ApiError::conflict("Account provisioning is not configured"));
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| ApiError::conflict("Auth unavailable"))?;
    let response = client
        .post(format!("{}/auth/v1/admin/users", state.config.supabase_url))
        .header("apikey", &state.config.supabase_service_key)
        .bearer_auth(&state.config.supabase_service_key)
        .json(&json!({"email":profile.email,"password":password,"email_confirm":true}))
        .send()
        .await
        .map_err(|_| ApiError::conflict("Auth unavailable"))?;
    if !response.status().is_success() {
        return Err(ApiError::conflict(
            "Auth account could not be created; verify email and retry",
        ));
    }
    let user: Value = response
        .json()
        .await
        .map_err(|_| ApiError::conflict("Invalid auth response"))?;
    let id = user["id"]
        .as_str()
        .ok_or_else(|| ApiError::conflict("Missing Auth user ID"))?;
    match save_profile(state, id, &profile).await {
        Ok(account_id) => Ok(Json(json!({"success":true,"account_id":account_id}))),
        Err(error) => {
            let cleanup = client
                .delete(format!(
                    "{}/auth/v1/admin/users/{id}",
                    state.config.supabase_url
                ))
                .header("apikey", &state.config.supabase_service_key)
                .bearer_auth(&state.config.supabase_service_key)
                .send()
                .await;
            if !cleanup.is_ok_and(|r| r.status().is_success()) {
                tracing::error!(auth_id = id, "Partial registration requires recovery")
            }
            Err(error)
        }
    }
}
#[derive(Deserialize)]
pub struct Page {
    pub page: Option<i64>,
    pub latest: Option<bool>,
    pub limit: Option<i64>,
    pub role: Option<String>,
    pub patient_id: Option<String>,
    pub doctor_id: Option<String>,
}
impl Page {
    pub fn bounds(&self) -> ApiResult<(i64, i64)> {
        let page = self.page.unwrap_or(1);
        let limit = self.limit.unwrap_or(10);
        if !(1..=100000).contains(&page) || !(1..=100).contains(&limit) {
            return Err(ApiError::bad(
                "Page must be positive; limit must be between 1 and 100",
            ));
        }
        Ok((limit, (page - 1) * limit))
    }
    pub fn response(&self, data: Vec<Value>, total: i64) -> Value {
        let limit = self.limit.unwrap_or(10);
        json!({"data":data,"pagination":{"total":total,"page":self.page.unwrap_or(1),"limit":limit,"total_pages":(total+limit-1)/limit}})
    }
}
async fn users(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    axum::extract::Query(page): axum::extract::Query<Page>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    let (limit, offset) = page.bounds()?;
    let data=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(u) FROM (SELECT COALESCE(p.id,d.id,a.id) AS id,a.id AS account_id,a.email,a.role,a.status,a.created_at AS registered_at,a.profile_photo,COALESCE(p.first_name,d.first_name,'')||' '||COALESCE(p.last_name,d.last_name,'') AS name,p.primary_doctor_id AS connected_doctor_id,p.device_id AS connected_device_id FROM accounts a LEFT JOIN patients p ON p.account_id=a.id LEFT JOIN doctors d ON d.account_id=a.id WHERE ($1::text IS NULL OR a.role=$1) ORDER BY a.created_at DESC,a.id LIMIT $2 OFFSET $3) u").bind(&page.role).bind(limit).bind(offset).fetch_all(&state.pool).await?;
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM accounts WHERE ($1::text IS NULL OR role=$1)")
            .bind(&page.role)
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(page.response(data, total)))
}
async fn stats(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    let value:Value=sqlx::query_scalar("SELECT jsonb_build_object('total_patients',(SELECT count(*) FROM patients),'total_doctors',(SELECT count(*) FROM doctors),'active_devices',(SELECT count(*) FROM devices WHERE last_seen_at>now()-interval '30 seconds'),'critical_alerts',(SELECT count(*) FROM frame_records WHERE regexp_replace(upper(btrim(label)),'\\s+',' ','g') NOT IN ('','NORMAL','NORM','NSR','NON ARITMIA','NON ARRHYTHMIA','UNKNOWN','UNAVAILABLE','UNCLASSIFIED') AND created_at>=current_date AND deleted_at IS NULL))").fetch_one(&state.pool).await?;
    Ok(Json(value))
}
