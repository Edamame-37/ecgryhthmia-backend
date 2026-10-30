use crate::api::{
    error::{ApiError, ApiResult},
    recordings::confined_path,
    routes::AppState,
};
use sqlx::Row;
use std::path::Path;
/// Reserve cleanup before publishing files. Committed references keep their files.
pub async fn reserve(state: &AppState, kind: &str, path: &Path) -> ApiResult<()> {
    sqlx::query("INSERT INTO file_cleanup(root_kind,path,not_before) VALUES($1,$2,now()+interval '1 hour') ON CONFLICT(path) DO NOTHING").bind(kind).bind(path.to_string_lossy().as_ref()).execute(&state.pool).await?;
    Ok(())
}
pub async fn process(state: &AppState) -> ApiResult<()> {
    for _ in 0..100 {
        let mut transaction = state.pool.begin().await?;
        let Some(row)=sqlx::query("SELECT id,root_kind,path FROM file_cleanup WHERE not_before<=now() ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *transaction).await? else {break};
        let id: i64 = row.get("id");
        let kind: String = row.get("root_kind");
        let path: String = row.get("path");
        let linked: bool = if kind == "records" {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM frame_records WHERE storage_path=$1 AND deleted_at IS NULL) OR EXISTS(SELECT 1 FROM sessions WHERE file_path=$1 AND deleted_at IS NULL)").bind(&path).fetch_one(&mut *transaction).await?
        } else {
            let public = format!(
                "/uploads/{}",
                Path::new(&path)
                    .strip_prefix(&state.config.uploads_root)
                    .map_err(|_| ApiError::forbidden())?
                    .to_string_lossy()
                    .replace('\\', "/")
            );
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM accounts WHERE profile_photo=$1) OR EXISTS(SELECT 1 FROM sessions WHERE ecg_paper=$1 AND deleted_at IS NULL)").bind(public).fetch_one(&mut *transaction).await?
        };
        let root = if kind == "records" {
            &state.config.records_root
        } else {
            &state.config.uploads_root
        };
        let result = if linked {
            Ok(())
        } else {
            match confined_path(root, Path::new(&path)).await {
                Ok(path) => tokio::fs::remove_file(path).await.map_err(ApiError::from),
                Err(error) if error.0 == axum::http::StatusCode::NOT_FOUND => Ok(()),
                Err(error) => Err(error),
            }
        };
        if result.is_ok() {
            sqlx::query("DELETE FROM file_cleanup WHERE id=$1")
                .bind(id)
                .execute(&mut *transaction)
                .await?;
        } else {
            sqlx::query("UPDATE file_cleanup SET attempts=attempts+1,not_before=now()+interval '1 minute' WHERE id=$1").bind(id).execute(&mut *transaction).await?;
            tracing::error!(cleanup_id = id, "File cleanup will be retried");
        }
        transaction.commit().await?;
    }
    Ok(())
}
