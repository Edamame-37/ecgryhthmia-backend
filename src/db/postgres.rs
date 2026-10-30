use crate::api::error::{ApiError, ApiResult};
use serde::Deserialize;
use sqlx::{
    postgres::{PgConnection, PgPoolOptions},
    PgPool, Row,
};
pub const SCHEMA_VERSION: i64 = 2026102708;
const LEGACY_MANIFEST: &str = include_str!("legacy_migrations.json");
#[derive(Deserialize)]
struct LegacyMigration {
    version: i64,
    description: String,
    checksum: String,
}
pub async fn create_pool(url: &str) -> ApiResult<PgPool> {
    Ok(PgPoolOptions::new().max_connections(5).acquire_timeout(std::time::Duration::from_secs(5))
 .after_connect(|connection,_|Box::pin(async move {
  sqlx::query("SELECT set_config('statement_timeout','15000',false),set_config('lock_timeout','5000',false),set_config('idle_in_transaction_session_timeout','30000',false)").execute(&mut *connection).await?;Ok(())
 })).connect(url).await?)
}
pub async fn schema_version(pool: &PgPool) -> ApiResult<i64> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !exists {
        return Ok(0);
    }
    Ok(
        sqlx::query_scalar("SELECT COALESCE(max(version),0) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await?,
    )
}
pub async fn schema_ready(pool: &PgPool) -> ApiResult<bool> {
    Ok(schema_version(pool).await? == SCHEMA_VERSION)
}
async fn validate_history(
    connection: &mut PgConnection,
    migrator: &sqlx::migrate::Migrator,
) -> ApiResult<bool> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *connection)
        .await?;
    if !exists {
        return Ok(false);
    }
    let legacy: Vec<LegacyMigration> =
        serde_json::from_str(LEGACY_MANIFEST).expect("valid checked-in migration manifest");
    let rows=sqlx::query("SELECT version,description,success,encode(checksum,'hex') AS checksum FROM _sqlx_migrations ORDER BY version").fetch_all(&mut *connection).await?;
    let mut has_legacy = false;
    for row in rows {
        let version: i64 = row.get("version");
        if migrator
            .iter()
            .any(|migration| migration.version == version)
        {
            continue;
        }
        let verified = legacy.iter().any(|migration| {
            migration.version == version
                && migration.description == row.get::<String, _>("description")
                && migration.checksum == row.get::<String, _>("checksum")
                && row.get::<bool, _>("success")
        });
        if !verified {
            return Err(ApiError::conflict(format!("Unrecognized migration history at version {version}; restore its source or review the database before upgrading")));
        }
        has_legacy = true;
    }
    Ok(has_legacy)
}
pub async fn run_migrations(pool: &PgPool) -> ApiResult<()> {
    let mut connection = pool.acquire().await?;
    let mut migrator = sqlx::migrate!("./migrations");
    let result=async {
  let has_legacy=validate_history(&mut connection,&migrator).await?;
  // Only the pinned historical migrations may be absent from this source tree.
  // SQLx still validates checksums for every current migration.
  migrator.set_ignore_missing(has_legacy);
  sqlx::query("SELECT set_config('statement_timeout','0',false),set_config('idle_in_transaction_session_timeout','0',false)").execute(&mut *connection).await?;
  migrator.run(&mut *connection).await.map_err(|error| {
   tracing::error!(%error,"Migration failed");
   ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE,format!("Database migration failed: {error}"))
  })
 }.await;
    // Do not return a connection with operator-only timeout settings to the pool.
    let closed = connection.close().await;
    result?;
    closed?;
    Ok(())
}
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}
