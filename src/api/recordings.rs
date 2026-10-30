use super::{
    accounts::Page,
    authorization::{validate_id, Identity, Role},
    error::{ApiError, ApiResult},
    routes::AppState,
};
use crate::models::{
    device::DevicePayload,
    payload::{digest, encode_packet, RecordedFrame},
};
use axum::{
    extract::{Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::{
    collections::BTreeMap,
    path::{Path as FilePath, PathBuf},
};
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sessions", get(sessions))
        .route("/api/patients/:id/sessions", get(patient_sessions))
        .route("/api/sessions/upload", post(upload))
        .route(
            "/api/sessions/:id",
            get(session_detail)
                .put(update_session)
                .delete(delete_session),
        )
        .route("/api/records/:id", get(records))
        .route("/api/records/:id/download", get(download_session))
        .route("/api/sessions/:id/frames", get(frames))
        .route("/api/sessions/:id/validation", get(validation))
        .route("/api/frames/:id", put(update_frame).delete(delete_frame))
        .route(
            "/api/sessions/:id/ecg_paper",
            post(upload_paper).delete(delete_paper),
        )
        .route("/uploads/:category/:filename", get(photo))
}
async fn sessions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let patient = if let Some(patient) = &page.patient_id {
        Some(identity.patient(&state.pool, patient).await?)
    } else if identity.role == Role::Patient {
        Some(identity.profile_id.clone())
    } else {
        None
    };
    if page.doctor_id.as_ref().is_some_and(|id| {
        identity.role == Role::Doctor && id != &identity.profile_id && id != &identity.account_id
    }) {
        return Err(ApiError::forbidden());
    }
    session_list(&state, &identity, &page, patient).await
}
async fn patient_sessions(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let patient = identity.patient(&state.pool, &id).await?;
    session_list(&state, &identity, &page, Some(patient)).await
}
async fn session_list(
    state: &AppState,
    identity: &Identity,
    page: &Page,
    patient: Option<String>,
) -> ApiResult<Json<Value>> {
    let (limit, offset) = page.bounds()?;
    let doctor = if identity.role == Role::Doctor {
        Some(identity.profile_id.clone())
    } else {
        page.doctor_id.clone()
    };
    let data=sqlx::query_scalar::<_,Value>("SELECT to_jsonb(s) FROM (SELECT s.id,s.device_id,s.patient_id,p.first_name||' '||p.last_name AS patient_name,p.primary_doctor_id AS doctor_id,s.started_at,s.ended_at,s.state,s.dev_note,s.ecg_paper FROM sessions s JOIN patients p ON p.id=s.patient_id WHERE s.deleted_at IS NULL AND ($1::text IS NULL OR p.id=$1) AND ($2::text IS NULL OR p.primary_doctor_id=$2) ORDER BY s.started_at DESC,s.id LIMIT $3 OFFSET $4) s").bind(&patient).bind(&doctor).bind(limit).bind(offset).fetch_all(&state.pool).await?;
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM sessions s JOIN patients p ON p.id=s.patient_id WHERE s.deleted_at IS NULL AND ($1::text IS NULL OR p.id=$1) AND ($2::text IS NULL OR p.primary_doctor_id=$2)").bind(patient).bind(doctor).fetch_one(&state.pool).await?;
    Ok(Json(page.response(data, total)))
}
async fn frames(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &id).await?;
    let (limit, offset) = page.bounds()?;
    let direction = if page.latest == Some(true) {
        "DESC"
    } else {
        "ASC"
    };
    let statement=format!("SELECT to_jsonb(f)-'storage_path'-'checksum' FROM frame_records f WHERE session_id=$1 AND deleted_at IS NULL AND ($2 OR hidden=false) ORDER BY start_time {direction},frame_number {direction},id LIMIT $3 OFFSET $4");
    let rows = sqlx::query_scalar::<_, Value>(&statement)
        .bind(&id)
        .bind(identity.role == Role::Admin)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
    let total:i64=sqlx::query_scalar("SELECT count(*) FROM frame_records WHERE session_id=$1 AND deleted_at IS NULL AND ($2 OR hidden=false)").bind(id).bind(identity.role==Role::Admin).fetch_one(&state.pool).await?;
    Ok(Json(page.response(rows, total)))
}
async fn validation(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &id).await?;
    let counts:Value=sqlx::query_scalar("SELECT jsonb_build_object('total',count(*),'validated',count(confirmation)) FROM frame_records WHERE session_id=$1 AND deleted_at IS NULL").bind(id).fetch_one(&state.pool).await?;
    Ok(Json(counts))
}
pub async fn confined_path(root: &FilePath, path: &FilePath) -> ApiResult<PathBuf> {
    let root = tokio::fs::canonicalize(root).await?;
    let canonical = tokio::fs::canonicalize(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::missing()
        } else {
            error.into()
        }
    })?;
    if !canonical.starts_with(&root) || canonical == root {
        return Err(ApiError::forbidden());
    }
    Ok(canonical)
}
pub async fn load_frames(
    state: &AppState,
    identity: &Identity,
    id: &str,
    frame_id: Option<&str>,
    limit: i64,
    offset: i64,
) -> ApiResult<Vec<Value>> {
    identity.session(&state.pool, id).await?;
    let rows=sqlx::query("SELECT * FROM frame_records WHERE session_id=$1 AND deleted_at IS NULL AND ($2::text IS NULL OR id=$2) AND ($3 OR hidden=false) ORDER BY start_time,frame_number,id LIMIT $4 OFFSET $5").bind(id).bind(frame_id).bind(identity.role==Role::Admin).bind(limit).bind(offset).fetch_all(&state.pool).await?;
    let legacy_path: Option<String> =
        sqlx::query_scalar("SELECT file_path FROM sessions WHERE id=$1")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    let mut legacy: Option<Vec<DevicePayload>> = None;
    let mut output = Vec::new();
    for row in rows {
        let frame_id: String = row.get("id");
        let payload = if let Some(path) = row.get::<Option<String>, _>("storage_path") {
            let path = confined_path(&state.config.records_root, FilePath::new(&path)).await?;
            let contents = tokio::fs::read(path).await?;
            let trimmed = contents.strip_suffix(b"\n").unwrap_or(&contents);
            if row.get::<Option<String>, _>("checksum").as_deref() != Some(&digest(trimmed)) {
                return Err(ApiError::conflict("Stored frame integrity check failed"));
            }
            let frame: RecordedFrame = serde_json::from_slice(trimmed)
                .map_err(|_| ApiError::conflict("Invalid stored frame"))?;
            frame.payload
        } else {
            if legacy.is_none() {
                let path = legacy_path.as_ref().ok_or_else(|| {
                    ApiError::conflict("Legacy recording requires reconciliation")
                })?;
                let path = confined_path(&state.config.records_root, FilePath::new(path)).await?;
                if tokio::fs::metadata(&path).await?.len() > 20 * 1024 * 1024 {
                    return Err(ApiError::conflict(
                        "Reconcile legacy files exceeding 20 MiB before reading",
                    ));
                }
                let file = tokio::fs::read_to_string(path).await?;
                let parsed = file
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(serde_json::from_str::<DevicePayload>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| ApiError::conflict("Invalid legacy recording"))?;
                legacy = Some(parsed);
            }
            let sequence = frame_id
                .get(frame_id.len().saturating_sub(6)..)
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| ApiError::conflict("Legacy frame cannot be mapped safely"))?;
            let candidates: Vec<_> = legacy
                .as_ref()
                .unwrap()
                .iter()
                .filter(|frame| frame.frame_id.parse::<u64>().ok() == Some(sequence))
                .collect();
            if candidates.len() != 1 {
                return Err(ApiError::conflict(
                    "Ambiguous legacy frame; run reconciliation",
                ));
            }
            candidates[0].clone()
        };
        let mut value = serde_json::to_value(payload)
            .map_err(|_| ApiError::conflict("Invalid stored frame"))?;
        let metadata = sqlx::query_scalar::<_, Value>(
            "SELECT to_jsonb(f)-'storage_path'-'checksum' FROM frame_records f WHERE id=$1",
        )
        .bind(&frame_id)
        .fetch_one(&state.pool)
        .await?;
        value
            .as_object_mut()
            .unwrap()
            .extend(metadata.as_object().unwrap().clone());
        value["frame_id"] = json!(frame_id);
        if identity.role == Role::Admin || value["hidden"] != true {
            output.push(value)
        }
    }
    Ok(output)
}
#[derive(Deserialize)]
struct RecordQuery {
    frame_id: Option<String>,
    frame_number: Option<i64>,
    page: Option<i64>,
    limit: Option<i64>,
    latest: Option<bool>,
}
async fn records(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Query(query): Query<RecordQuery>,
) -> ApiResult<Json<Value>> {
    let limit = query.limit.unwrap_or(50);
    let page = query.page.unwrap_or(1);
    if !(1..=100).contains(&limit) || !(1..=100000).contains(&page) {
        return Err(ApiError::bad("Invalid recording page"));
    }
    let selected = if query.latest == Some(true) {
        identity.session(&state.pool, &id).await?;
        sqlx::query_scalar::<_,String>("SELECT id FROM frame_records WHERE session_id=$1 AND deleted_at IS NULL AND ($2 OR hidden=false) ORDER BY start_time DESC,frame_number DESC LIMIT 1").bind(&id).bind(identity.role==Role::Admin).fetch_optional(&state.pool).await?
    } else if let Some(number) = query.frame_number {
        identity.session(&state.pool, &id).await?;
        Some(sqlx::query_scalar::<_,String>("SELECT id FROM frame_records WHERE session_id=$1 AND frame_number=$2 AND deleted_at IS NULL AND ($3 OR hidden=false)").bind(&id).bind(number).bind(identity.role==Role::Admin).fetch_optional(&state.pool).await?.ok_or_else(ApiError::missing)?)
    } else {
        query.frame_id
    };
    if let Some(frame_id) = &selected {
        validate_id(frame_id)?;
    }
    Ok(Json(json!(
        load_frames(
            &state,
            &identity,
            &id,
            selected.as_deref(),
            limit,
            (page - 1) * limit
        )
        .await?
    )))
}
async fn session_detail(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &id).await?;
    Ok(Json(
        sqlx::query_scalar(
            "SELECT to_jsonb(s)-'file_path' FROM sessions s WHERE id=$1 AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_one(&state.pool)
        .await?,
    ))
}
async fn download_session(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    identity.session(&state.pool, &id).await?;
    let chunks = futures_util::stream::try_unfold(
        (state, identity, id, 0i64, false),
        |(state, identity, id, offset, opened)| async move {
            if offset < 0 {
                return Ok::<_, std::io::Error>(None);
            }
            if !opened {
                return Ok(Some((b"[".to_vec(), (state, identity, id, offset, true))));
            }
            let frames = load_frames(&state, &identity, &id, None, 1, offset)
                .await
                .map_err(|_| std::io::Error::other("Recording export failed"))?;
            if let Some(frame) = frames.first() {
                let prefix = if offset == 0 { "" } else { "," };
                let content = format!(
                    "{prefix}{}",
                    serde_json::to_string(frame).map_err(std::io::Error::other)?
                );
                Ok(Some((
                    content.into_bytes(),
                    (state, identity, id, offset + 1, true),
                )))
            } else {
                Ok(Some((b"]".to_vec(), (state, identity, id, -1, true))))
            }
        },
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=ecg-session.json",
            ),
        ],
        axum::body::Body::from_stream(chunks),
    )
        .into_response())
}
pub async fn segment_packet(
    state: &AppState,
    identity: &Identity,
    id: &str,
    index: u64,
) -> ApiResult<String> {
    identity.session(&state.pool, id).await?;
    let selected:String=sqlx::query_scalar("SELECT id FROM frame_records WHERE session_id=$1 AND frame_number=$2 AND deleted_at IS NULL AND ($3 OR hidden=false)").bind(id).bind(index as i64+1).bind(identity.role==Role::Admin).fetch_one(&state.pool).await?;
    let frames = load_frames(state, identity, id, Some(&selected), 1, 0).await?;
    let frame = frames.first().ok_or_else(ApiError::missing)?;
    let samples = frame["ecg"]["samples"]
        .as_array()
        .ok_or_else(|| ApiError::conflict("Missing samples"))?;
    let rate = frame["sampling_rate_hz"]
        .as_f64()
        .ok_or_else(|| ApiError::conflict("Missing sampling rate"))?;
    let start = frame["start_time"]
        .as_f64()
        .ok_or_else(|| ApiError::conflict("Missing frame time"))?;
    encode_packet(&json!({"type":"segment_data","session_id":id,"device_id":frame["device_id"],"frame_number":frame["frame_number"],"start_time":start,"sampling_rate_hz":rate,"duration_s":frame["duration_s"],"sample_count":samples.len(),
 "data_payload":{"raw":{"time":(0..samples.len()).map(|i|start+i as f64/rate).collect::<Vec<_>>(),"ch1":samples.iter().map(|v|v[0].clone()).collect::<Vec<_>>(),"ch2":samples.iter().map(|v|v[1].clone()).collect::<Vec<_>>(),"ch3":samples.iter().map(|v|v[2].clone()).collect::<Vec<_>>()},"classification_result":frame["label"],"prediction_details":frame["prediction"],"anomaly_indices":[]}})).map_err(|_|ApiError::conflict("Packet encoding failed"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionUpdate {
    patient_id: Option<String>,
    doctor_id: Option<String>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    dev_note: Option<String>,
}
async fn update_session(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<SessionUpdate>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    identity.session(&state.pool, &id).await?;
    if let Some(patient) = &body.patient_id {
        identity.patient(&state.pool, patient).await?;
    }
    if body.doctor_id.is_some() {
        return Err(ApiError::bad(
            "Assign doctors through the patient connection endpoint",
        ));
    }
    if body.dev_note.as_ref().is_some_and(|v| v.len() > 4000) {
        return Err(ApiError::bad("Note too long"));
    }
    let changed=sqlx::query("UPDATE sessions SET patient_id=COALESCE($1,patient_id),started_at=COALESCE($2,started_at),dev_note=$3 WHERE id=$4 AND state NOT IN ('requested','recording','stopping')")
  .bind(body.patient_id).bind(body.started_at).bind(body.dev_note).bind(id).execute(&state.pool).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::conflict("Cannot edit an active session"));
    }
    Ok(Json(json!({"success":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameUpdate {
    hidden: Option<bool>,
    confirmation: Option<bool>,
    doc_classification: Option<String>,
    doc_note: Option<String>,
    clear_validation: Option<bool>,
}
async fn update_frame(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(body): Json<FrameUpdate>,
) -> ApiResult<Json<Value>> {
    let session: String = sqlx::query_scalar(
        "SELECT session_id FROM frame_records WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await?;
    identity.session(&state.pool, &session).await?;
    if body.hidden.is_some() {
        identity.admin()?;
    }
    let validation = body.confirmation.is_some()
        || body.doc_classification.is_some()
        || body.doc_note.is_some()
        || body.clear_validation == Some(true);
    if validation && identity.role != Role::Doctor {
        return Err(ApiError::forbidden());
    }
    if body.doc_note.as_ref().is_some_and(|v| v.len() > 4000)
        || body.doc_classification.as_ref().is_some_and(|v| {
            !matches!(
                v.as_str(),
                "Non Arrhythmia" | "AF" | "Takikardia" | "Bradikardia" | "UNKNOWN"
            )
        })
    {
        return Err(ApiError::bad("Invalid review fields"));
    }
    if body.clear_validation == Some(true) {
        sqlx::query("UPDATE frame_records SET confirmation=NULL,doc_classification=NULL,doc_note=NULL WHERE id=$1").bind(&id).execute(&state.pool).await?;
    } else {
        sqlx::query("UPDATE frame_records SET hidden=COALESCE($1,hidden),confirmation=COALESCE($2,confirmation),doc_classification=COALESCE($3,doc_classification),doc_note=COALESCE($4,doc_note) WHERE id=$5").bind(body.hidden).bind(body.confirmation).bind(body.doc_classification).bind(body.doc_note).bind(&id).execute(&state.pool).await?;
    }
    Ok(Json(json!({"success":true})))
}
async fn delete_frame(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    let row = sqlx::query(
        "SELECT session_id,storage_path FROM frame_records WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(&id)
    .fetch_one(&state.pool)
    .await?;
    let session: String = row.get("session_id");
    identity.session(&state.pool, &session).await?;
    let path: String = row
        .get::<Option<String>, _>("storage_path")
        .ok_or_else(|| {
            ApiError::conflict("Reconcile legacy recording before deleting individual frames")
        })?;
    let canonical = confined_path(&state.config.records_root, FilePath::new(&path)).await?;
    // Tombstones prevent fallback payloads from reappearing, including after IO failure.
    let mut transaction = state.pool.begin().await?;
    sqlx::query("INSERT INTO file_cleanup(root_kind,path) VALUES('records',$1) ON CONFLICT(path) DO UPDATE SET not_before=now()").bind(&path).execute(&mut *transaction).await?;
    sqlx::query("UPDATE frame_records SET deleted_at=now() WHERE id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    tokio::fs::remove_file(canonical).await?;
    Ok(Json(json!({"success":true})))
}
async fn delete_session(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.admin()?;
    identity.session(&state.pool, &id).await?;
    let mut transaction = state.pool.begin().await?;
    let row = sqlx::query("SELECT state,file_path,ecg_paper FROM sessions WHERE id=$1 FOR UPDATE")
        .bind(&id)
        .fetch_one(&mut *transaction)
        .await?;
    if matches!(
        row.get::<String, _>("state").as_str(),
        "requested" | "recording" | "stopping"
    ) {
        return Err(ApiError::conflict("Stop the session before deletion"));
    }
    let mut paths: Vec<String> = sqlx::query_scalar(
        "SELECT storage_path FROM frame_records WHERE session_id=$1 AND storage_path IS NOT NULL",
    )
    .bind(&id)
    .fetch_all(&mut *transaction)
    .await?;
    if let Some(path) = row.get::<Option<String>, _>("file_path") {
        paths.push(path)
    }
    for path in &paths {
        sqlx::query("INSERT INTO file_cleanup(root_kind,path) VALUES('records',$1) ON CONFLICT(path) DO UPDATE SET not_before=now()").bind(path).execute(&mut *transaction).await?;
    }
    if let Some(photo) = row
        .get::<Option<String>, _>("ecg_paper")
        .filter(|path| path.starts_with("/uploads/ecg_papers/"))
    {
        let photo = state
            .config
            .uploads_root
            .join(photo.trim_start_matches("/uploads/"));
        sqlx::query("INSERT INTO file_cleanup(root_kind,path) VALUES('uploads',$1) ON CONFLICT(path) DO UPDATE SET not_before=now()").bind(photo.to_string_lossy().as_ref()).execute(&mut *transaction).await?;
    }
    sqlx::query("UPDATE sessions SET deleted_at=now() WHERE id=$1")
        .bind(&id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE frame_records SET deleted_at=now() WHERE session_id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    for path in paths {
        match confined_path(&state.config.records_root, FilePath::new(&path)).await {
            Ok(path) => tokio::fs::remove_file(path).await?,
            Err(error) if error.0 == StatusCode::NOT_FOUND => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Json(json!({"success":true})))
}
fn image_type(bytes: &[u8]) -> ApiResult<&'static str> {
    if bytes.len() > 5 * 1024 * 1024 {
        return Err(ApiError::bad("Image exceeds 5 MiB"));
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("png")
    } else {
        Err(ApiError::bad("Only JPEG and PNG images are supported"))
    }
}
async fn write_image(state: &AppState, category: &str, bytes: &[u8]) -> ApiResult<String> {
    let extension = image_type(bytes)?;
    let directory = state.config.uploads_root.join(category);
    tokio::fs::create_dir_all(&directory).await?;
    let filename = format!("{}.{}", uuid::Uuid::new_v4().simple(), extension);
    let local = directory.join(&filename);
    crate::services::storage::reserve(state, "uploads", &local).await?;
    tokio::fs::write(local, bytes).await?;
    Ok(format!("/uploads/{category}/{filename}"))
}
pub async fn save_photo(
    state: &AppState,
    account: &str,
    value: Option<&str>,
) -> ApiResult<Option<String>> {
    let Some(value) = value else { return Ok(None) };
    if value.starts_with("/uploads/profiles/") {
        return Ok(None);
    }
    if value.is_empty() {
        return Ok(Some(String::new()));
    }
    let (prefix, data) = value
        .split_once(',')
        .ok_or_else(|| ApiError::bad("Invalid photo data"))?;
    if !matches!(prefix, "data:image/jpeg;base64" | "data:image/png;base64") {
        return Err(ApiError::bad("Invalid photo format"));
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ApiError::bad("Invalid image encoding"))?;
    let old: Option<String> = sqlx::query_scalar("SELECT profile_photo FROM accounts WHERE id=$1")
        .bind(account)
        .fetch_one(&state.pool)
        .await?;
    if let Some(old) = old.filter(|path| path.starts_with("/uploads/profiles/")) {
        crate::services::storage::reserve(
            state,
            "uploads",
            &state
                .config
                .uploads_root
                .join(old.trim_start_matches("/uploads/")),
        )
        .await?;
    }
    Ok(Some(write_image(state, "profiles", &bytes).await?))
}
async fn upload_paper(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &id).await?;
    if let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad("Invalid image upload"))?
    {
        if field.name() != Some("paper") {
            return Err(ApiError::bad("Expected paper image"));
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|_| ApiError::bad("Invalid image data"))?;
        let old: Option<String> = sqlx::query_scalar("SELECT ecg_paper FROM sessions WHERE id=$1")
            .bind(&id)
            .fetch_one(&state.pool)
            .await?;
        if let Some(old) = old.filter(|path| path.starts_with("/uploads/ecg_papers/")) {
            crate::services::storage::reserve(
                &state,
                "uploads",
                &state
                    .config
                    .uploads_root
                    .join(old.trim_start_matches("/uploads/")),
            )
            .await?;
        }
        let path = write_image(&state, "ecg_papers", &bytes).await?;
        sqlx::query("UPDATE sessions SET ecg_paper=$1 WHERE id=$2")
            .bind(&path)
            .bind(id)
            .execute(&state.pool)
            .await?;
        return Ok(Json(json!({"success":true,"path":path})));
    }
    Err(ApiError::bad("No image uploaded"))
}
async fn delete_paper(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    identity.session(&state.pool, &id).await?;
    let mut transaction = state.pool.begin().await?;
    let path: Option<String> =
        sqlx::query_scalar("SELECT ecg_paper FROM sessions WHERE id=$1 FOR UPDATE")
            .bind(&id)
            .fetch_one(&mut *transaction)
            .await?;
    sqlx::query("UPDATE sessions SET ecg_paper=NULL WHERE id=$1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    if let Some(photo) = &path {
        let local = state
            .config
            .uploads_root
            .join(photo.trim_start_matches("/uploads/"));
        sqlx::query("INSERT INTO file_cleanup(root_kind,path) VALUES('uploads',$1) ON CONFLICT(path) DO UPDATE SET not_before=now()").bind(local.to_string_lossy().as_ref()).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    if let Some(photo) = path {
        let local = state
            .config
            .uploads_root
            .join(photo.trim_start_matches("/uploads/"));
        match confined_path(&state.config.uploads_root, &local).await {
            Ok(path) => tokio::fs::remove_file(path).await?,
            Err(error) if error.0 == StatusCode::NOT_FOUND => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Json(json!({"success":true})))
}
async fn photo(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path((category, filename)): Path<(String, String)>,
) -> ApiResult<Response> {
    validate_id(filename.split('.').next().unwrap_or_default())?;
    if !matches!(category.as_str(), "profiles" | "ecg_papers")
        || !filename.ends_with(".jpg") && !filename.ends_with(".png")
    {
        return Err(ApiError::missing());
    }
    let public = format!("/uploads/{category}/{filename}");
    // Legacy references include an HTTP(S) origin. Match only the exact remaining
    // path; never fetch the origin or use a basename/suffix match.
    let statement = if category == "profiles" {
        "SELECT id FROM accounts WHERE profile_photo=$1 OR regexp_replace(profile_photo, '^https?://[^/?#]+', '')=$1 LIMIT 2"
    } else {
        "SELECT id FROM sessions WHERE deleted_at IS NULL AND (ecg_paper=$1 OR regexp_replace(ecg_paper, '^https?://[^/?#]+', '')=$1) LIMIT 2"
    };
    let references: Vec<String> = sqlx::query_scalar(statement)
        .bind(&public)
        .fetch_all(&state.pool)
        .await?;
    let resource = match references.as_slice() {
        [] => return Err(ApiError::missing()),
        [resource] => resource,
        _ => return Err(ApiError::conflict("Photo has multiple resource references")),
    };
    if category == "profiles" {
        let account = resource;
        if account != &identity.account_id
            && identity.role != Role::Admin
            && identity.patient(&state.pool, account).await.is_err()
        {
            identity.doctor(&state.pool, account).await?;
        }
    } else {
        identity.session(&state.pool, resource).await?;
    }
    let path = confined_path(
        &state.config.uploads_root,
        &state.config.uploads_root.join(&category).join(&filename),
    )
    .await?;
    let bytes = tokio::fs::read(path).await?;
    let mime = if filename.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        bytes,
    )
        .into_response())
}
#[derive(Default)]
struct UploadParts {
    csv_rate: Option<f64>,
    metadata: Option<Value>,
    samples: Option<Vec<Vec<f64>>>,
    prediction: Option<Value>,
    system: Option<Value>,
}
async fn upload(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    mut multipart: Multipart,
) -> ApiResult<Json<Value>> {
    let mut patient = None;
    let mut device = None;
    let mut parts: BTreeMap<String, UploadParts> = BTreeMap::new();
    let mut bytes_total = 0usize;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad("Invalid upload"))?
    {
        let field_name = field.name().unwrap_or_default().to_string();
        let filename = field.file_name().map(str::to_string);
        let bytes = field
            .bytes()
            .await
            .map_err(|_| ApiError::bad("Invalid upload bytes"))?;
        bytes_total += bytes.len();
        if bytes_total > 20 * 1024 * 1024 {
            return Err(ApiError::bad("Upload exceeds 20 MiB"));
        }
        match field_name.as_str() {
            "patient_id" => {
                patient = Some(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|_| ApiError::bad("Invalid patient ID"))?,
                )
            }
            "device_id" => {
                device = Some(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|_| ApiError::bad("Invalid device ID"))?,
                )
            }
            "files" => {
                let filename = filename.ok_or_else(|| ApiError::bad("Filename missing"))?;
                let stem = filename
                    .strip_suffix(".json")
                    .or_else(|| filename.strip_suffix(".csv"))
                    .ok_or_else(|| ApiError::bad("Only JSON and CSV frame files are supported"))?;
                let (key, kind) = if let Some(key) = stem.strip_suffix("_prediction") {
                    (key, "prediction")
                } else if let Some(key) = stem.strip_suffix("_system") {
                    (key, "system")
                } else {
                    (
                        stem,
                        if filename.ends_with(".csv") {
                            "csv"
                        } else {
                            "metadata"
                        },
                    )
                };
                validate_id(key)?;
                if parts.len() >= 128 && !parts.contains_key(key) {
                    return Err(ApiError::bad("Too many frames"));
                }
                let part = parts.entry(key.to_string()).or_default();
                if kind == "csv" {
                    if part.samples.is_some() {
                        return Err(ApiError::bad("Duplicate CSV frame"));
                    }
                    let csv = crate::data::csv_reader::parse_csv_frame(bytes.as_ref())?;
                    part.csv_rate = csv.sampling_rate_hz;
                    part.samples = Some(csv.samples);
                } else {
                    let value: Value = serde_json::from_slice(&bytes)
                        .map_err(|_| ApiError::bad("Invalid frame JSON"))?;
                    let slot = match kind {
                        "prediction" => &mut part.prediction,
                        "system" => &mut part.system,
                        _ => &mut part.metadata,
                    };
                    if slot.replace(value).is_some() {
                        return Err(ApiError::bad("Duplicate frame file"));
                    }
                }
            }
            _ => return Err(ApiError::bad("Unknown upload field")),
        }
    }
    let patient = identity
        .patient(
            &state.pool,
            patient
                .as_deref()
                .ok_or_else(|| ApiError::bad("patient_id required"))?,
        )
        .await?;
    let assigned: Option<String> = sqlx::query_scalar("SELECT device_id FROM patients WHERE id=$1")
        .bind(&patient)
        .fetch_one(&state.pool)
        .await?;
    let device = identity
        .device(
            &state.pool,
            device
                .as_deref()
                .or(assigned.as_deref())
                .ok_or_else(|| ApiError::bad("Assigned device required"))?,
        )
        .await?;
    let device_name: String = sqlx::query_scalar("SELECT name FROM devices WHERE id=$1")
        .bind(&device)
        .fetch_one(&state.pool)
        .await?;
    let session = crate::db::postgres::new_id("ses");
    let mut payloads = Vec::new();
    if parts.is_empty() {
        return Err(ApiError::bad("No frame files uploaded"));
    }
    let mut ordered_parts: Vec<_> = parts.into_iter().collect();
    ordered_parts.sort_by(|(left, a), (right, b)| {
        let time = |part: &UploadParts| {
            part.metadata
                .as_ref()
                .and_then(|value| value["created_at"].as_str())
                .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        };
        let number = |name: &str| {
            name.rsplit(|c: char| !c.is_ascii_digit())
                .find(|part| !part.is_empty())
                .and_then(|part| part.parse::<u64>().ok())
        };
        time(a)
            .cmp(&time(b))
            .then_with(|| number(left).cmp(&number(right)))
            .then_with(|| left.cmp(right))
    });
    for (index, (_, part)) in ordered_parts.into_iter().enumerate() {
        let mut value = part
            .metadata
            .ok_or_else(|| ApiError::bad("Every frame requires metadata JSON"))?;
        if let Some(rate) = part.csv_rate {
            if let Some(declared) = value["sampling_rate_hz"].as_f64() {
                if (declared - rate).abs() > rate * 0.005 {
                    return Err(ApiError::bad(
                        "CSV timing does not match metadata sampling rate",
                    ));
                }
            } else {
                value["sampling_rate_hz"] = json!(rate);
            }
        }
        if let Some(samples) = part.samples {
            value["ecg"] = json!({"format":"samples_by_time","samples":samples});
        }
        if let Some(prediction) = part.prediction {
            value["prediction"] = prediction;
        }
        if let Some(system) = part.system {
            value["system"] = system;
        }
        value["session_id"] = json!(session);
        value["device_id"] = json!(device_name);
        value["frame_id"] = json!((index + 1).to_string());
        if value["message_id"].is_null() {
            value["message_id"] = json!(format!("import_{}_{}", session, index + 1));
        }
        let payload: DevicePayload = serde_json::from_value(value)
            .map_err(|_| ApiError::bad("Incomplete frame metadata"))?;
        payload.validate().map_err(ApiError::bad)?;
        payloads.push(payload);
    }
    let started = chrono::DateTime::parse_from_rfc3339(&payloads[0].created_at)
        .map_err(|_| ApiError::bad("Invalid timestamp"))?
        .with_timezone(&chrono::Utc);
    sqlx::query("INSERT INTO sessions(id,device_id,patient_id,started_at,state) VALUES($1,$2,$3,$4,'importing')").bind(&session).bind(&device).bind(patient).bind(started).execute(&state.pool).await?;
    for payload in payloads {
        if let Err(error) =
            crate::services::ingestion::persist(&state, &device, payload, false).await
        {
            sqlx::query("UPDATE sessions SET state='failed' WHERE id=$1")
                .bind(&session)
                .execute(&state.pool)
                .await?;
            return Err(error);
        }
    }
    sqlx::query("UPDATE sessions SET state='ended',ended_at=started_at+next_time*interval '1 second' WHERE id=$1").bind(&session).execute(&state.pool).await?;
    Ok(Json(json!({"success":true,"session_id":session})))
}
