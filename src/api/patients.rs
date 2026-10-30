use super::{
    accounts::Page,
    authorization::{Identity, Role},
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/patients", get(list))
        .route("/api/patients/:id", get(profile).put(update_profile))
        .route("/api/doctors/:id", get(doctor_profile).put(update_doctor))
        .route("/api/doctors/:id/patients", get(doctor_patients))
        .route("/api/patients/:id/connect", post(connect))
        .route("/api/patients/:id/disconnect", post(disconnect))
        .route("/api/patients/:id/pairing-code", post(pairing_code))
        .route("/api/patients/:id/recording-status", get(recording_status))
}
async fn patient_value(state: &AppState, id: &str) -> ApiResult<Value> {
    Ok(sqlx::query_scalar("SELECT to_jsonb(p)||jsonb_build_object('profile_photo',a.profile_photo,'age',CASE WHEN p.date_of_birth IS NULL THEN p.age ELSE extract(year from age(current_date,p.date_of_birth))::integer END) FROM patients p JOIN accounts a ON a.id=p.account_id WHERE p.id=$1").bind(id).fetch_one(&state.pool).await?)
}
async fn profile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = identity.patient(&state.pool, &id).await?;
    let patient = patient_value(&state, &id).await?;
    let doctor = if let Some(doctor_id) = patient["primary_doctor_id"].as_str() {
        sqlx::query_scalar::<_,Value>("SELECT to_jsonb(d)||jsonb_build_object('profile_photo',a.profile_photo) FROM doctors d JOIN accounts a ON a.id=d.account_id WHERE d.id=$1").bind(doctor_id).fetch_optional(&state.pool).await?
    } else {
        None
    };
    Ok(Json(json!({"patient":patient,"doctor":doctor})))
}
async fn doctor_profile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = identity.doctor(&state.pool, &id).await?;
    Ok(Json(sqlx::query_scalar("SELECT to_jsonb(d)||jsonb_build_object('email',a.email,'role',a.role,'profile_photo',a.profile_photo) FROM doctors d JOIN accounts a ON a.id=d.account_id WHERE d.id=$1").bind(id).fetch_one(&state.pool).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileUpdate {
    first_name: Option<String>,
    last_name: Option<String>,
    age: Option<i32>,
    gender: Option<String>,
    date_of_birth: Option<chrono::NaiveDate>,
    profile_photo: Option<String>,
}
impl ProfileUpdate {
    fn validate(&self) -> ApiResult<()> {
        if self
            .first_name
            .as_ref()
            .is_some_and(|v| v.trim().is_empty() || v.len() > 100)
            || self.last_name.as_ref().is_some_and(|v| v.len() > 100)
            || self.age.is_some_and(|v| !(0..=130).contains(&v))
            || self
                .gender
                .as_ref()
                .is_some_and(|v| !matches!(v.as_str(), "L" | "P" | "other" | ""))
            || self
                .date_of_birth
                .is_some_and(|v| v > chrono::Utc::now().date_naive())
        {
            return Err(ApiError::bad("Invalid profile fields"));
        }
        Ok(())
    }
}
async fn update_profile(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ProfileUpdate>,
) -> ApiResult<Json<Value>> {
    let id = identity.patient(&state.pool, &id).await?;
    let account: String = sqlx::query_scalar("SELECT account_id FROM patients WHERE id=$1")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    if identity.role != Role::Admin && identity.account_id != account {
        return Err(ApiError::forbidden());
    }
    body.validate()?;
    let photo =
        super::recordings::save_photo(&state, &account, body.profile_photo.as_deref()).await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE patients SET first_name=COALESCE($1,first_name),last_name=COALESCE($2,last_name),age=COALESCE($3,age),gender=COALESCE($4,gender),date_of_birth=COALESCE($5,date_of_birth) WHERE id=$6")
  .bind(body.first_name).bind(body.last_name).bind(body.age).bind(body.gender).bind(body.date_of_birth).bind(&id).execute(&mut *transaction).await?;
    if let Some(photo) = photo {
        sqlx::query("UPDATE accounts SET profile_photo=$1 WHERE id=$2")
            .bind(photo)
            .bind(account)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(Json(json!({"success":true})))
}
async fn update_doctor(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ProfileUpdate>,
) -> ApiResult<Json<Value>> {
    let id = identity.doctor(&state.pool, &id).await?;
    if identity.role != Role::Admin && identity.profile_id != id {
        return Err(ApiError::forbidden());
    }
    body.validate()?;
    let account: String = sqlx::query_scalar("SELECT account_id FROM doctors WHERE id=$1")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;
    let photo =
        super::recordings::save_photo(&state, &account, body.profile_photo.as_deref()).await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE doctors SET first_name=COALESCE($1,first_name),last_name=COALESCE($2,last_name) WHERE id=$3").bind(body.first_name).bind(body.last_name).bind(id).execute(&mut *transaction).await?;
    if let Some(photo) = photo {
        sqlx::query("UPDATE accounts SET profile_photo=$1 WHERE id=$2")
            .bind(photo)
            .bind(account)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(Json(json!({"success":true})))
}
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    patient_list(&state, &page, None).await
}
async fn doctor_patients(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let doctor = identity.doctor(&state.pool, &id).await?;
    if identity.role != Role::Admin && identity.role != Role::Doctor {
        return Err(ApiError::forbidden());
    }
    patient_list(&state, &page, Some(doctor)).await
}
async fn patient_list(
    state: &AppState,
    page: &Page,
    doctor: Option<String>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page.bounds()?;
    let rows=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(p) FROM (SELECT p.id,p.account_id,p.first_name||' '||p.last_name AS name,CASE WHEN p.date_of_birth IS NULL THEN p.age ELSE extract(year from age(current_date,p.date_of_birth))::integer END AS age,p.gender,p.primary_doctor_id,p.device_id AS connected_device_id,a.profile_photo FROM patients p JOIN accounts a ON a.id=p.account_id WHERE ($1::text IS NULL OR p.primary_doctor_id=$1) ORDER BY p.id LIMIT $2 OFFSET $3) p").bind(&doctor).bind(limit).bind(offset).fetch_all(&state.pool).await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM patients WHERE ($1::text IS NULL OR primary_doctor_id=$1)",
    )
    .bind(doctor)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(page.response(rows, total)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectInput {
    doctor_id: String,
    code: Option<String>,
}
async fn pairing_code(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = identity.patient(&state.pool, &id).await?;
    if identity.role == Role::Doctor {
        return Err(ApiError::forbidden());
    }
    let code = crate::db::postgres::new_id("pair");
    sqlx::query("INSERT INTO pairing_codes(digest,resource_id,kind,issued_by,expires_at) VALUES($1,$2,'patient',$3,now()+interval '5 minutes')")
  .bind(crate::models::payload::digest(code.as_bytes())).bind(&id).bind(identity.account_id).execute(&state.pool).await?;
    Ok(Json(
        json!({"code":code,"patient_id":id,"expires_in":300,"qr":format!("ecgrhythmia://sync/patient/{id}?token={code}")}),
    ))
}
async fn connect(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<ConnectInput>,
) -> ApiResult<Json<Value>> {
    let patient: String =
        sqlx::query_scalar("SELECT id FROM patients WHERE id=$1 OR account_id=$1")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    let doctor: String = sqlx::query_scalar("SELECT id FROM doctors WHERE id=$1 OR account_id=$1")
        .bind(body.doctor_id)
        .fetch_one(&state.pool)
        .await?;
    if identity.role == Role::Doctor && identity.profile_id != doctor {
        return Err(ApiError::forbidden());
    }
    let mut transaction = state.pool.begin().await?;
    if identity.role != Role::Admin {
        if identity.role != Role::Doctor {
            return Err(ApiError::forbidden());
        }
        let code = body
            .code
            .ok_or_else(|| ApiError::bad("Patient pairing code required"))?;
        let consumed=sqlx::query("UPDATE pairing_codes SET used_at=now() WHERE digest=$1 AND resource_id=$2 AND kind='patient' AND used_at IS NULL AND expires_at>now()")
   .bind(crate::models::payload::digest(code.as_bytes())).bind(&patient).execute(&mut *transaction).await?;
        if consumed.rows_affected() != 1 {
            return Err(ApiError::forbidden());
        }
    }
    sqlx::query("UPDATE patients SET primary_doctor_id=$1 WHERE id=$2")
        .bind(doctor)
        .bind(patient)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Json(json!({"success":true})))
}
async fn disconnect(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let patient = identity.patient(&state.pool, &id).await?;
    sqlx::query("UPDATE patients SET primary_doctor_id=NULL WHERE id=$1")
        .bind(&patient)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE impersonation_sessions SET revoked_at=now() WHERE effective_id=(SELECT account_id FROM patients WHERE id=$1)").bind(patient).execute(&state.pool).await?;
    Ok(Json(json!({"success":true})))
}
async fn recording_status(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let patient = identity.patient(&state.pool, &id).await?;
    let session=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(s) FROM (SELECT id AS session_id,device_id,state,started_at FROM sessions WHERE patient_id=$1 AND state IN ('requested','recording','stopping') AND deleted_at IS NULL ORDER BY started_at DESC LIMIT 1) s").bind(patient).fetch_optional(&state.pool).await?;
    Ok(Json(
        json!({"is_recording":session.as_ref().is_some_and(|s|s["state"]=="recording"||s["state"]=="stopping"),"session":session}),
    ))
}
