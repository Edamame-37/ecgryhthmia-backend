use super::{
    authorization::{Identity, Role},
    error::{ApiError, ApiResult},
    routes::AppState,
};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/devices", get(list))
        .route("/api/admin/devices", get(list).post(add))
        .route("/api/admin/devices/:id", put(edit))
        .route("/api/devices/:id/command", post(command))
        .route("/api/devices/:id/assign", post(assign))
        .route("/api/devices/:id/pairing-code", post(pairing_code))
}
async fn list(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> ApiResult<Json<Value>> {
    let devices=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(d) FROM (SELECT d.id,d.name,d.last_seen_at,d.telemetry,CASE WHEN d.last_seen_at IS NULL THEN 'Unknown' WHEN d.last_seen_at>now()-interval '30 seconds' THEN 'Online' ELSE 'Offline' END AS status,p.id AS assigned_to FROM devices d LEFT JOIN patients p ON p.device_id=d.id WHERE $1='admin' OR ($1='pasien' AND p.account_id=$2) OR ($1='dokter' AND p.primary_doctor_id=$3) ORDER BY d.id) d").bind(identity.role.as_str()).bind(&identity.account_id).bind(&identity.profile_id).fetch_all(&state.pool).await?;
    if identity.role != Role::Admin {
        return Ok(Json(json!(devices)));
    }
    let mut output = Vec::new();
    for mut device in devices {
        let configuration=sqlx::query_scalar::<_,Value>("SELECT jsonb_build_object('mqtt_broker',mqtt_broker,'mqtt_port',mqtt_port,'mqtt_topic',mqtt_topic,'mqtt_username',mqtt_username) FROM device_credentials WHERE device_id=$1").bind(device["id"].as_str().unwrap_or_default()).fetch_optional(&state.pool).await?;
        if let Some(Value::Object(configuration)) = configuration {
            device.as_object_mut().unwrap().extend(configuration);
        }
        output.push(device);
    }
    Ok(Json(json!(output)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceInput {
    name: String,
    mqtt_broker: String,
    mqtt_port: i32,
    mqtt_topic: String,
    mqtt_username: String,
    mqtt_password: Option<String>,
}
impl DeviceInput {
    fn validate(&self, state: &AppState) -> ApiResult<()> {
        super::authorization::validate_id(&self.name)?;
        if self.mqtt_username.len() > 200
            || self.mqtt_password.as_ref().is_some_and(|v| v.len() > 1024)
        {
            return Err(ApiError::bad("Invalid MQTT credentials"));
        }
        crate::network::mqtt_listener::validate_broker(
            state,
            &self.mqtt_broker,
            self.mqtt_port,
            &self.mqtt_topic,
        )
    }
}
async fn add(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<DeviceInput>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    body.validate(&state)?;
    let password = body
        .mqtt_password
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::bad("MQTT password required"))?;
    let id = crate::db::postgres::new_id("dev");
    let mut transaction = state.pool.begin().await?;
    sqlx::query("INSERT INTO devices(id,name) VALUES($1,$2)")
        .bind(&id)
        .bind(&body.name)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("INSERT INTO device_credentials(device_id,mqtt_broker,mqtt_port,mqtt_topic,mqtt_username,mqtt_password) VALUES($1,$2,$3,$4,$5,$6)").bind(&id).bind(body.mqtt_broker).bind(body.mqtt_port).bind(body.mqtt_topic).bind(body.mqtt_username).bind(password).execute(&mut *transaction).await?;
    transaction.commit().await?;
    let ingestion = crate::services::ingestion::start(state.clone());
    crate::network::mqtt_listener::connect_device(state, &id, ingestion).await?;
    Ok(Json(json!({"success":true,"id":id})))
}
async fn edit(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<DeviceInput>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    body.validate(&state)?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT id FROM devices WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE device_id=$1 AND state IN ('requested','recording','stopping'))").bind(&id).fetch_one(&mut *transaction).await?;
    if active {
        return Err(ApiError::conflict("Cannot edit a recording device"));
    }
    sqlx::query("UPDATE devices SET name=$1,config_revision=config_revision+1 WHERE id=$2")
        .bind(body.name)
        .bind(&id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE device_credentials SET mqtt_broker=$1,mqtt_port=$2,mqtt_topic=$3,mqtt_username=$4,mqtt_password=COALESCE(NULLIF($5,''),mqtt_password) WHERE device_id=$6").bind(body.mqtt_broker).bind(body.mqtt_port).bind(body.mqtt_topic).bind(body.mqtt_username).bind(body.mqtt_password).bind(&id).execute(&mut *transaction).await?;
    transaction.commit().await?;
    let ingestion = crate::services::ingestion::start(state.clone());
    crate::network::mqtt_listener::connect_device(state, &id, ingestion).await?;
    Ok(Json(json!({"success":true})))
}
async fn command(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<crate::services::recording::CommandInput>,
) -> ApiResult<Json<Value>> {
    let key = headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::bad("Idempotency-Key required"))?;
    Ok(Json(
        crate::services::recording::command(&state, &identity, &id, body, key).await?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignInput {
    patient_id: Option<String>,
    code: Option<String>,
}
async fn assign(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(device): Path<String>,
    Json(body): Json<AssignInput>,
) -> ApiResult<Json<Value>> {
    let device: String = sqlx::query_scalar("SELECT id FROM devices WHERE id=$1 OR name=$1")
        .bind(device)
        .fetch_one(&state.pool)
        .await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT id FROM devices WHERE id=$1 FOR UPDATE")
        .bind(&device)
        .fetch_one(&mut *transaction)
        .await?;
    let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE device_id=$1 AND state IN ('requested','recording','stopping'))").bind(&device).fetch_one(&mut *transaction).await?;
    if active {
        return Err(ApiError::conflict("Device is recording"));
    }
    if identity.role != Role::Admin {
        if identity.role != Role::Patient {
            return Err(ApiError::forbidden());
        }
        let code = body
            .code
            .as_deref()
            .ok_or_else(|| ApiError::bad("Device pairing code required"))?;
        let result=sqlx::query("UPDATE pairing_codes SET used_at=now() WHERE digest=$1 AND resource_id=$2 AND kind='device' AND used_at IS NULL AND expires_at>now()").bind(crate::models::payload::digest(code.as_bytes())).bind(&device).execute(&mut *transaction).await?;
        if result.rows_affected() != 1 {
            return Err(ApiError::forbidden());
        }
    }
    if let Some(patient) = body.patient_id {
        let patient = identity.patient(&state.pool, &patient).await?;
        sqlx::query("SELECT id FROM patients WHERE id=$1 FOR UPDATE")
            .bind(&patient)
            .fetch_one(&mut *transaction)
            .await?;
        let previous_active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE patient_id=$1 AND state IN ('requested','recording','stopping'))").bind(&patient).fetch_one(&mut *transaction).await?;
        if previous_active {
            return Err(ApiError::conflict("Patient has an active recording"));
        }
        let occupied: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM patients WHERE device_id=$1 AND id<>$2)",
        )
        .bind(&device)
        .bind(&patient)
        .fetch_one(&mut *transaction)
        .await?;
        if occupied {
            return Err(ApiError::conflict("Device is assigned to another patient"));
        }
        sqlx::query("UPDATE patients SET device_id=$1 WHERE id=$2")
            .bind(device)
            .bind(patient)
            .execute(&mut *transaction)
            .await?;
    } else {
        identity.admin()?;
        sqlx::query("UPDATE patients SET device_id=NULL WHERE device_id=$1")
            .bind(device)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(Json(json!({"success":true})))
}
async fn pairing_code(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    let id = identity.device(&state.pool, &id).await?;
    let code = crate::db::postgres::new_id("pair");
    sqlx::query("INSERT INTO pairing_codes(digest,resource_id,kind,issued_by,expires_at) VALUES($1,$2,'device',$3,now()+interval '5 minutes')").bind(crate::models::payload::digest(code.as_bytes())).bind(&id).bind(identity.account_id).execute(&state.pool).await?;
    Ok(Json(
        json!({"code":code,"device_id":id,"expires_in":300,"qr":json!({"type":"device_sync","deviceId":id,"code":code}).to_string()}),
    ))
}
