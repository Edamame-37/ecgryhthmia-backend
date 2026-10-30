use crate::api::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgConnection, PgPoolOptions},
    PgPool, Row,
};
pub const SCHEMA_VERSION: i64 = 2026102710;
const LEGACY_MANIFEST: &str = include_str!("legacy_migrations.json");
const PRIVATE_TABLES: &[&str] = &[
    "accounts",
    "doctors",
    "patients",
    "devices",
    "sessions",
    "frame_records",
    "device_credentials",
    "pairing_codes",
    "impersonation_sessions",
    "audit_events",
    "command_receipts",
    "file_cleanup",
];
const REQUIRED_COLUMNS: &[&str] = &[
    "accounts.auth_id",
    "accounts.enabled",
    "accounts.tokens_valid_after",
    "devices.hardware_id",
    "devices.config_revision",
    "device_credentials.device_id",
    "device_credentials.mqtt_broker",
    "device_credentials.mqtt_topic",
    "sessions.state",
    "sessions.next_frame",
    "sessions.next_time",
    "sessions.deleted_at",
    "frame_records.storage_path",
    "frame_records.checksum",
    "frame_records.sampling_rate_hz",
    "frame_records.sample_count",
    "frame_records.source_message_id",
    "frame_records.deleted_at",
];

#[derive(Debug, Serialize)]
pub struct SchemaReport {
    pub current_version: i64,
    pub expected_version: i64,
    pub ready: bool,
    pub acquisition_ready: bool,
    pub unmapped_device_count: Option<i64>,
    pub issues: Vec<String>,
}
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
    Ok(schema_report(pool).await?.ready)
}

/// Metadata-only checks: a migration ledger alone does not establish schema integrity.
pub async fn schema_report(pool: &PgPool) -> ApiResult<SchemaReport> {
    let current_version = schema_version(pool).await?;
    let mut issues = Vec::new();
    if current_version != SCHEMA_VERSION {
        issues.push(format!(
            "Schema version is {current_version}; expected {SCHEMA_VERSION}"
        ));
    }
    check_applied_history(pool, &mut issues).await?;
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT table_name||'.'||column_name FROM information_schema.columns WHERE table_schema=current_schema() AND table_name=ANY($1)",
    ).bind(PRIVATE_TABLES).fetch_all(pool).await?;
    for column in REQUIRED_COLUMNS {
        if !columns.iter().any(|present| present == column) {
            issues.push(format!("Required column missing: {column}"));
        }
    }
    if columns
        .iter()
        .any(|column| column == "accounts.password_hash")
    {
        issues.push("Forbidden legacy column present: accounts.password_hash".into());
    }
    let tables=sqlx::query(
        "SELECT c.relname,c.relrowsecurity,EXISTS(SELECT 1 FROM aclexplode(COALESCE(c.relacl,acldefault('r',c.relowner))) a WHERE a.grantee=0) AS public_access,EXISTS(SELECT 1 FROM pg_roles r WHERE r.rolname IN ('anon','authenticated') AND has_table_privilege(r.oid,c.oid,'SELECT,INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')) AS browser_access FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=current_schema() AND c.relkind='r' AND c.relname=ANY($1)",
    ).bind(PRIVATE_TABLES).fetch_all(pool).await?;
    for table in PRIVATE_TABLES {
        let Some(row) = tables
            .iter()
            .find(|row| row.get::<String, _>("relname") == *table)
        else {
            issues.push(format!("Private table missing: {table}"));
            continue;
        };
        if !row.get::<bool, _>("relrowsecurity") {
            issues.push(format!("RLS disabled: {table}"));
        }
        if row.get::<bool, _>("public_access") || row.get::<bool, _>("browser_access") {
            issues.push(format!(
                "Private table has browser/PUBLIC privileges: {table}"
            ));
        }
    }
    let unmapped_device_count = if columns.iter().any(|column| column == "devices.hardware_id") {
        Some(
            sqlx::query_scalar("SELECT count(*) FROM devices WHERE hardware_id IS NULL")
                .fetch_one(pool)
                .await?,
        )
    } else {
        None
    };
    let ready = issues.is_empty();
    Ok(SchemaReport {
        current_version,
        expected_version: SCHEMA_VERSION,
        ready,
        acquisition_ready: ready && unmapped_device_count == Some(0),
        unmapped_device_count,
        issues,
    })
}

async fn check_applied_history(pool: &PgPool, issues: &mut Vec<String>) -> ApiResult<()> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !exists {
        issues.push("Migration ledger missing".into());
        return Ok(());
    }
    let rows=sqlx::query("SELECT version,description,success,encode(checksum,'hex') AS checksum FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool).await?;
    let migrator = sqlx::migrate!("./migrations");
    let legacy: Vec<LegacyMigration> =
        serde_json::from_str(LEGACY_MANIFEST).expect("valid checked-in migration manifest");
    for migration in migrator.iter() {
        let checksum: String = migration
            .checksum
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let valid = rows.iter().any(|row| {
            row.get::<i64, _>("version") == migration.version
                && row.get::<bool, _>("success")
                && row.get::<String, _>("description") == migration.description
                && row.get::<String, _>("checksum") == checksum
        });
        if !valid {
            issues.push(format!(
                "Migration missing or changed: {}",
                migration.version
            ));
        }
    }
    for row in rows {
        let version: i64 = row.get("version");
        if migrator
            .iter()
            .any(|migration| migration.version == version)
        {
            continue;
        }
        if !legacy.iter().any(|entry| {
            entry.version == version
                && entry.description == row.get::<String, _>("description")
                && entry.checksum == row.get::<String, _>("checksum")
                && row.get::<bool, _>("success")
        }) {
            issues.push(format!("Unrecognized migration history: {version}"));
        }
    }
    Ok(())
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
