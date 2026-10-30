//! Offline maintenance: dry-run by default. Never deletes a source recording.
use ecg_backend::{
    api::{authorization::validate_id, recordings::confined_path},
    config::AppConfig,
    models::{
        device::DevicePayload,
        payload::{digest, RecordedFrame},
    },
};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use std::{
    collections::HashSet,
    error::Error,
    path::{Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    frame_id: String,
    line_number: usize,
}
fn argument(name: &str) -> Option<String> {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}
fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = AppConfig::load().map_err(|error| error.1)?;
    let pool = ecg_backend::db::postgres::create_pool(&config.database_url)
        .await
        .map_err(|error| error.1)?;
    let apply = std::env::args().any(|arg| arg == "--apply");
    let Some(session_id) = argument("--session") else {
        if apply {
            return Err("--apply requires --session, --manifest and --backup".into());
        }
        let report:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object('legacy_frames',count(*) FILTER(WHERE storage_path IS NULL),'invalid_intervals',count(*) FILTER(WHERE end_time<=start_time),'deleted_frames',count(*) FILTER(WHERE deleted_at IS NOT NULL)) FROM frame_records").fetch_one(&pool).await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    };
    validate_id(&session_id).map_err(|error| error.1)?;
    let mut transaction = pool.begin().await?;
    let session=sqlx::query("SELECT s.*,d.name FROM sessions s JOIN devices d ON d.id=s.device_id WHERE s.id=$1 FOR UPDATE OF s").bind(&session_id).fetch_one(&mut *transaction).await?;
    let state: String = session.get("state");
    if matches!(
        state.as_str(),
        "requested" | "recording" | "stopping" | "importing"
    ) {
        return Err("Stop recording before reconciliation".into());
    }
    let rows =
        sqlx::query("SELECT * FROM frame_records WHERE session_id=$1 ORDER BY start_time,id")
            .bind(&session_id)
            .fetch_all(&mut *transaction)
            .await?;
    let manifest = argument("--manifest");
    if manifest.is_none() {
        println!(
            "{}",
            json!({"dry_run":true,"session_id":session_id,"frame_count":rows.len(),"requires_explicit_line_mapping":true})
        );
        return Ok(());
    }
    let mappings: Vec<Mapping> = serde_json::from_slice(&std::fs::read(manifest.unwrap())?)?;
    if rows.len() != mappings.len() {
        return Err("Manifest must map every frame exactly once".into());
    }
    let legacy: Option<String> = session.get("file_path");
    let source = confined_path(
        &config.records_root,
        Path::new(&legacy.ok_or("No legacy source file")?),
    )
    .await
    .map_err(|error| error.1)?;
    if std::fs::metadata(&source)?.len() > 100 * 1024 * 1024 {
        return Err("Split legacy sources larger than 100 MiB offline first".into());
    }
    let source_bytes = std::fs::read(&source)?;
    let source_text = std::str::from_utf8(&source_bytes)?;
    let lines: Vec<_> = source_text.lines().collect();
    let mut ids = HashSet::new();
    let mut used_lines = HashSet::new();
    let mut prepared = Vec::new();
    let mut start = 0.0;
    for (index, mapping) in mappings.iter().enumerate() {
        validate_id(&mapping.frame_id).map_err(|error| error.1)?;
        if !ids.insert(&mapping.frame_id) || !used_lines.insert(mapping.line_number) {
            return Err("Duplicate mapping".into());
        }
        let row = rows
            .iter()
            .find(|row| row.get::<String, _>("id") == mapping.frame_id)
            .ok_or("Unknown frame in manifest")?;
        if row.get::<Option<String>, _>("storage_path").is_some() {
            return Err("Only unreconciled legacy sessions may be repaired".into());
        }
        let line = lines
            .get(
                mapping
                    .line_number
                    .checked_sub(1)
                    .ok_or("Line numbers start at 1")?,
            )
            .ok_or("Line out of range")?;
        let payload: DevicePayload = serde_json::from_str(line)?;
        payload.validate().map_err(std::io::Error::other)?;
        if payload.device_id != session.get::<String, _>("name") {
            return Err("Source device mismatch; explicit investigation required".into());
        }
        let end = start + payload.duration_s;
        let frame = RecordedFrame {
            id: mapping.frame_id.clone(),
            session_id: session_id.clone(),
            patient_id: session.get("patient_id"),
            device_id: session.get("device_id"),
            frame_number: index as i64 + 1,
            start_time: start,
            end_time: end,
            payload,
        };
        prepared.push(frame);
        start = end;
    }
    println!(
        "{}",
        json!({"dry_run":!apply,"session_id":session_id,"frames":prepared.len(),"duration_s":start,"source_sha256":digest(&source_bytes)})
    );
    if !apply {
        return Ok(());
    }
    let backup_root =
        PathBuf::from(argument("--backup").ok_or("--backup is required for --apply")?);
    std::fs::create_dir_all(&backup_root)?;
    let backup = backup_root.join(format!("{}-{}", session_id, uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&backup)?;
    durable_write(&backup.join("source.jsonl"), &source_bytes)?;
    let original:serde_json::Value=sqlx::query_scalar("SELECT jsonb_build_object('session',(SELECT to_jsonb(s) FROM sessions s WHERE id=$1),'frames',(SELECT jsonb_agg(to_jsonb(f)) FROM frame_records f WHERE session_id=$1))").bind(&session_id).fetch_one(&mut *transaction).await?;
    durable_write(
        &backup.join("metadata.json"),
        &serde_json::to_vec_pretty(&original)?,
    )?;
    durable_write(
        &backup.join("manifest.json"),
        &std::fs::read(argument("--manifest").unwrap())?,
    )?;
    let directory = config.records_root.join(&session_id);
    std::fs::create_dir_all(&directory)?;
    let mut written = Vec::new();
    let result=async {
  for frame in &prepared {
   let bytes=serde_json::to_vec(frame)?;
   let path=directory.join(format!("reconciled_{}.jsonl",uuid::Uuid::new_v4().simple()));
   durable_write(&path,&bytes)?;written.push(path.clone());
   sqlx::query("UPDATE frame_records SET frame_number=$1,start_time=$2,end_time=$3,time_interval=$4,storage_path=$5,checksum=$6,sampling_rate_hz=$7,sample_count=$8,source_message_id=$9 WHERE id=$10")
    .bind(frame.frame_number).bind(frame.start_time).bind(frame.end_time).bind(format!("{:.3} - {:.3}",frame.start_time,frame.end_time)).bind(path.to_string_lossy().as_ref()).bind(digest(&bytes)).bind(frame.payload.sampling_rate_hz).bind(frame.payload.ecg.samples.len() as i32).bind(&frame.payload.message_id).bind(&frame.id).execute(&mut *transaction).await?;
  }
  sqlx::query("UPDATE sessions SET next_frame=$1,next_time=$2,file_path=NULL WHERE id=$3").bind(prepared.len() as i64+1).bind(start).bind(&session_id).execute(&mut *transaction).await?;
  transaction.commit().await?;Ok::<(),Box<dyn Error>>(())
 }.await;
    if result.is_err() {
        for path in written {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}
