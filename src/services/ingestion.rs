use crate::{
    api::{
        error::{ApiError, ApiResult},
        routes::AppState,
    },
    models::{
        device::DevicePayload,
        payload::{digest, RecordedFrame},
    },
};
use sqlx::Row;
use tokio::io::AsyncWriteExt;
pub enum Inbound {
    Frame {
        device_id: String,
        payload: Box<DevicePayload>,
    },
    Ack {
        device_id: String,
        ack: super::recording::DeviceAck,
    },
}
pub fn start(state: AppState) -> tokio::sync::mpsc::Sender<Inbound> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<Inbound>(64);
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            let result = match message {
                Inbound::Frame { device_id, payload } => {
                    persist(&state, &device_id, *payload, true)
                        .await
                        .map(|_| ())
                }
                Inbound::Ack { device_id, ack } => {
                    super::recording::acknowledge(&state, &device_id, ack).await
                }
            };
            if let Err(error) = result {
                tracing::warn!(error=?error,"Incoming message rejected")
            }
        }
    });
    sender
}
pub async fn persist(
    state: &AppState,
    device_id: &str,
    mut payload: DevicePayload,
    live: bool,
) -> ApiResult<RecordedFrame> {
    payload.validate().map_err(ApiError::bad)?;
    let id = crate::db::postgres::new_id("fra");
    let directory = state.config.records_root.join(&payload.session_id);
    tokio::fs::create_dir_all(&directory).await?;
    let path = directory.join(format!("{id}.jsonl"));
    let temporary = directory.join(format!("{id}.pending"));
    super::storage::reserve(state, "records", &path).await?;
    super::storage::reserve(state, "records", &temporary).await?;
    let mut transaction = state.pool.begin().await?;
    let session=sqlx::query("SELECT s.id,s.patient_id,s.device_id,s.next_frame,s.next_time,s.state,d.name FROM sessions s JOIN devices d ON d.id=s.device_id WHERE s.id=$1 AND s.device_id=$2 AND s.deleted_at IS NULL FOR UPDATE OF s")
  .bind(&payload.session_id).bind(device_id).fetch_optional(&mut *transaction).await?.ok_or_else(ApiError::missing)?;
    let state_name: String = session.get("state");
    if live && !matches!(state_name.as_str(), "recording" | "stopping") {
        return Err(ApiError::conflict("Session is not recording"));
    }
    if !live && state_name != "importing" {
        return Err(ApiError::conflict("Session is not importing"));
    }
    if payload.device_id != session.get::<String, _>("name") {
        return Err(ApiError::bad("Payload device does not match listener"));
    }
    if live {
        let created = chrono::DateTime::parse_from_rfc3339(&payload.created_at)
            .map_err(|_| ApiError::bad("Invalid frame timestamp"))?;
        if (chrono::Utc::now() - created.with_timezone(&chrono::Utc))
            .num_seconds()
            .abs()
            > 120
        {
            return Err(ApiError::bad("Stale or future frame"));
        }
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM frame_records WHERE session_id=$1 AND source_message_id=$2)",
    )
    .bind(&payload.session_id)
    .bind(&payload.message_id)
    .fetch_one(&mut *transaction)
    .await?;
    if duplicate {
        return Err(ApiError::conflict("Duplicate source message"));
    }
    let number: i64 = session.get("next_frame");
    if live && payload.frame_id.parse::<i64>().ok() != Some(number) {
        return Err(ApiError::conflict("Unexpected frame sequence"));
    }
    let start: f64 = session.get("next_time");
    let end = start + payload.duration_s;
    payload.frame_id = number.to_string();
    let frame = RecordedFrame {
        id: id.clone(),
        device_id: device_id.into(),
        patient_id: session.get("patient_id"),
        session_id: payload.session_id.clone(),
        frame_number: number,
        start_time: start,
        end_time: end,
        payload,
    };
    let bytes = serde_json::to_vec(&frame).map_err(|_| ApiError::bad("Frame cannot be encoded"))?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await?;
    file.write_all(&bytes).await?;
    file.write_all(b"\n").await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&temporary, &path).await?;
    let label = frame
        .payload
        .prediction
        .as_ref()
        .map(|p| p.label.as_str())
        .unwrap_or("UNKNOWN");
    let result=async {
  sqlx::query("INSERT INTO frame_records(id,session_id,start_time,end_time,time_interval,label,frame_number,source_message_id,storage_path,checksum,sampling_rate_hz,sample_count) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)")
   .bind(&id).bind(&frame.session_id).bind(start).bind(end).bind(format!("{start:.3} - {end:.3}")).bind(label).bind(number).bind(&frame.payload.message_id).bind(path.to_string_lossy().as_ref()).bind(digest(&bytes)).bind(frame.payload.sampling_rate_hz).bind(frame.payload.ecg.samples.len() as i32).execute(&mut *transaction).await?;
  sqlx::query("UPDATE sessions SET next_frame=next_frame+1,next_time=$1 WHERE id=$2").bind(end).bind(&frame.session_id).execute(&mut *transaction).await?;
  sqlx::query("UPDATE devices SET last_seen_at=now(),telemetry=$2 WHERE id=$1").bind(device_id).bind(serde_json::to_value(&frame.payload.system).unwrap_or(serde_json::Value::Null)).execute(&mut *transaction).await?;
  transaction.commit().await
 }.await;
    if let Err(error) = result {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error.into());
    }
    if live {
        if let Err(error) = state.pacer.enqueue(frame.clone()).await {
            tracing::warn!(?error, "Frame persisted; live delivery delayed or dropped")
        }
    }
    Ok(frame)
}
