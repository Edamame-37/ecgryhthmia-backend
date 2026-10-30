use crate::api::{
    authorization::Identity,
    error::{ApiError, ApiResult},
    routes::AppState,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Command {
    Start,
    Stop,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandInput {
    pub command: Command,
    pub patient_id: Option<String>,
    pub session_id: Option<String>,
}
pub async fn command(
    state: &AppState,
    identity: &Identity,
    device: &str,
    input: CommandInput,
    key: &str,
) -> ApiResult<serde_json::Value> {
    let device = identity.device(&state.pool, device).await?;
    super::super::api::authorization::validate_id(key)?;
    let signature = crate::models::payload::digest(
        serde_json::json!({"device":device,"input":input,"effective_id":identity.account_id})
            .to_string()
            .as_bytes(),
    );
    let existing:Option<(String,String,String)>=sqlx::query_as("SELECT c.signature,c.session_id,s.state FROM command_receipts c JOIN sessions s ON s.id=c.session_id WHERE c.actor_id=$1 AND c.request_key=$2").bind(&identity.actor_id).bind(key).fetch_optional(&state.pool).await?;
    if let Some((previous, session, state)) = existing {
        if previous != signature {
            return Err(ApiError::conflict(
                "Idempotency key was used for a different command",
            ));
        }
        if matches!(state.as_str(), "interrupted" | "failed") {
            return Err(ApiError::conflict(
                "Previous command failed; inspect session before retrying",
            ));
        }
        return Ok(serde_json::json!({"success":true,"session_id":session,"state":state}));
    }
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT id FROM devices WHERE id=$1 FOR UPDATE")
        .bind(&device)
        .fetch_one(&mut *transaction)
        .await?;
    let previous:Option<(String,String,String)>=sqlx::query_as("SELECT c.signature,c.session_id,s.state FROM command_receipts c JOIN sessions s ON s.id=c.session_id WHERE c.actor_id=$1 AND c.request_key=$2").bind(&identity.actor_id).bind(key).fetch_optional(&mut *transaction).await?;
    if let Some((previous, session, state)) = previous {
        if previous != signature || matches!(state.as_str(), "interrupted" | "failed") {
            return Err(ApiError::conflict("Previous command differs or failed"));
        }
        return Ok(serde_json::json!({"success":true,"session_id":session,"state":state}));
    }
    let (topic,revision):(String,i64)=sqlx::query_as("SELECT c.mqtt_topic,d.config_revision FROM device_credentials c JOIN devices d ON d.id=c.device_id WHERE d.id=$1").bind(&device).fetch_one(&mut *transaction).await?;
    let connection = state
        .mqtt_clients
        .read()
        .await
        .get(&device)
        .cloned()
        .ok_or_else(|| ApiError::conflict("Device has no MQTT connection"))?;
    if connection.revision != revision {
        return Err(ApiError::conflict(
            "Device configuration is reloading; retry shortly",
        ));
    }
    let client = connection.client;
    let session_key =
        crate::models::payload::digest(format!("{}:{key}", identity.actor_id).as_bytes());
    let (session, command_name) = match input.command {
        Command::Start => {
            let patient = identity
                .patient(
                    &state.pool,
                    input
                        .patient_id
                        .as_deref()
                        .ok_or_else(|| ApiError::bad("patient_id required"))?,
                )
                .await?;
            sqlx::query("SELECT id FROM patients WHERE id=$1 FOR UPDATE")
                .bind(&patient)
                .fetch_one(&mut *transaction)
                .await?;
            let assigned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM patients WHERE id=$1 AND device_id=$2)",
            )
            .bind(&patient)
            .bind(&device)
            .fetch_one(&mut *transaction)
            .await?;
            if !assigned {
                return Err(ApiError::forbidden());
            }
            if let Some(existing)=sqlx::query_scalar::<_,serde_json::Value>("SELECT jsonb_build_object('success',true,'session_id',id,'state',state) FROM sessions WHERE command_key=$1 AND device_id=$2 AND patient_id=$3")
    .bind(&session_key).bind(&device).bind(&patient).fetch_optional(&mut *transaction).await? {return Ok(existing)}
            let id = crate::db::postgres::new_id("ses");
            sqlx::query("INSERT INTO sessions(id,device_id,patient_id,started_at,state,command_key,command_deadline) VALUES($1,$2,$3,now(),'requested',$4,now()+interval '30 seconds')")
    .bind(&id).bind(&device).bind(&patient).bind(&session_key).execute(&mut *transaction).await?;
            (id, "START")
        }
        Command::Stop => {
            let id = input
                .session_id
                .ok_or_else(|| ApiError::bad("session_id required for STOP"))?;
            identity.session(&state.pool, &id).await?;
            let changed=sqlx::query("UPDATE sessions SET state='stopping',command_deadline=now()+interval '30 seconds' WHERE id=$1 AND device_id=$2 AND state IN ('recording','stopping')").bind(&id).bind(&device).execute(&mut *transaction).await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("Session is not recording"));
            }
            (id, "STOP")
        }
    };
    sqlx::query("INSERT INTO command_receipts(actor_id,request_key,signature,session_id) VALUES($1,$2,$3,$4)").bind(&identity.actor_id).bind(key).bind(signature).bind(&session).execute(&mut *transaction).await?;
    transaction.commit().await?;
    let payload =
        serde_json::json!({"command":command_name,"session_id":session,"idempotency_key":key});
    if let Err(error) = client
        .publish(
            format!("{topic}/command"),
            rumqttc::QoS::AtLeastOnce,
            false,
            payload.to_string(),
        )
        .await
    {
        tracing::error!(%error,"MQTT publish failed");
        sqlx::query("UPDATE sessions SET state='interrupted',ended_at=now(),command_deadline=NULL WHERE id=$1").bind(&session).execute(&state.pool).await?;
        return Err(ApiError::conflict("MQTT command could not be queued"));
    }
    Ok(
        serde_json::json!({"success":true,"session_id":session,"state":if command_name=="START"{"requested"}else{"stopping"}}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceAck {
    pub session_id: String,
    pub command: Command,
    pub last_frame_number: Option<i64>,
}
pub async fn acknowledge(state: &AppState, device: &str, ack: DeviceAck) -> ApiResult<()> {
    match ack.command {
        Command::Start => {
            sqlx::query("UPDATE sessions SET state='recording',command_deadline=NULL WHERE id=$1 AND device_id=$2 AND state='requested'").bind(&ack.session_id).bind(device).execute(&state.pool).await?;
        }
        Command::Stop => {
            let last = ack
                .last_frame_number
                .ok_or_else(|| ApiError::bad("STOP acknowledgment requires final frame number"))?;
            let result=sqlx::query("UPDATE sessions SET state='ended',ended_at=now(),command_deadline=NULL WHERE id=$1 AND device_id=$2 AND state='stopping' AND next_frame-1=$3").bind(&ack.session_id).bind(device).bind(last).execute(&state.pool).await?;
            if result.rows_affected() != 1 {
                return Err(ApiError::conflict(
                    "STOP acknowledgment does not match persisted frames",
                ));
            }
        }
    }
    state
        .hub
        .publish(
            &ack.session_id,
            crate::models::payload::encode_packet(
                &serde_json::json!({"type":"status","session_id":ack.session_id}),
            )
            .map_err(|_| ApiError::bad("Status encoding failed"))?,
        )
        .await;
    Ok(())
}
pub async fn expire_commands(state: &AppState) -> ApiResult<()> {
    sqlx::query("UPDATE sessions SET state='interrupted',ended_at=now(),command_deadline=NULL WHERE command_deadline<now() AND state IN ('requested','stopping')").execute(&state.pool).await?;
    Ok(())
}
