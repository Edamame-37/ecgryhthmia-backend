mod support;
use serde_json::json;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    Row,
};
use std::{process::Command, str::FromStr};
use support::*;
#[tokio::test]
async fn legacy_upgrade_preserves_data_and_moves_private_credentials() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    guard_database(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("ecg_test_upgrade_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/2026102701_baseline.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("ALTER TABLE accounts ADD COLUMN password_hash TEXT; INSERT INTO accounts(id,email,role,password_hash) VALUES('legacy_auth','legacy@test.invalid','pasien','not-an-actual-hash'); INSERT INTO devices(id,name,mqtt_broker,mqtt_port,mqtt_topic,mqtt_username,mqtt_password) VALUES('dev_old','hardware_old','test.invalid',8883,'old/topic','old-user','fixture-only'); INSERT INTO patients(id,account_id,first_name,last_name,device_id) VALUES('pat_old','legacy_auth','Legacy','Fixture','dev_old'); INSERT INTO sessions(id,device_id,patient_id,started_at,file_path) VALUES('ses_old','dev_old','pat_old',now(),'records/legacy.jsonl'); INSERT INTO frame_records(id,session_id,start_time,end_time,time_interval,label) VALUES('fra_old_000001','ses_old',0,0,'incorrect legacy interval','UNKNOWN'); GRANT SELECT ON accounts,devices TO anon;").execute(&pool).await.unwrap();
    seed_legacy_ledger(&pool).await;
    ecg_backend::db::postgres::run_migrations(&pool)
        .await
        .unwrap();
    assert_legacy_history_unchanged(&pool).await;
    let account: String = sqlx::query_scalar("SELECT auth_id FROM accounts WHERE id='legacy_auth'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(account, "legacy_auth");
    let credential: String = sqlx::query_scalar(
        "SELECT mqtt_password FROM device_credentials WHERE device_id='dev_old'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(credential, "fixture-only");
    let hardware_id: Option<String> =
        sqlx::query_scalar("SELECT hardware_id FROM devices WHERE id='dev_old'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        hardware_id, None,
        "Legacy hardware mappings must not be guessed"
    );
    let private_columns:i64=sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_schema=$1 AND ((table_name='devices' AND column_name='mqtt_password') OR (table_name='accounts' AND column_name='password_hash'))").bind(&schema).fetch_one(&pool).await.unwrap();
    assert_eq!(private_columns, 0);
    let unchanged: f64 =
        sqlx::query_scalar("SELECT end_time FROM frame_records WHERE id='fra_old_000001'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        unchanged, 0.0,
        "Legacy ambiguity must not be repaired by guessing"
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconciliation_requires_explicit_mapping_backs_up_and_preserves_reviews() {
    let context = TestContext::new().await;
    let payload = payload(1);
    let source = context.state.config.records_root.join("legacy.jsonl");
    std::fs::write(&source, serde_json::to_vec(&payload).unwrap()).unwrap();
    sqlx::query(
        "UPDATE sessions SET state='ended',ended_at=now(),file_path=$1 WHERE id='ses_pat_A'",
    )
    .bind(source.to_string_lossy().as_ref())
    .execute(&context.state.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO frame_records(id,session_id,start_time,end_time,time_interval,label,hidden,confirmation,doc_note) VALUES('fra_legacy_000001','ses_pat_A',10,20,'10 - 20','UNKNOWN',true,true,'retain this review')").execute(&context.state.pool).await.unwrap();
    let directory = context.state.config.records_root.parent().unwrap();
    let mapping = directory.join("mapping.json");
    std::fs::write(
        &mapping,
        json!([{"frame_id":"fra_legacy_000001","line_number":1}]).to_string(),
    )
    .unwrap();
    let backup = directory.join("backups");
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let separator = if url.contains('?') { '&' } else { '?' };
    let invoke = |apply: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_reconcile"));
        command
            .env(
                "DATABASE_URL",
                format!("{url}{separator}options=-csearch_path%3D{}", context.schema),
            )
            .env("SUPABASE_URL", "https://auth.ecg.test")
            .env("SUPABASE_ANON_KEY", "synthetic-public")
            .env("RECORDS_ROOT", &context.state.config.records_root)
            .env("UPLOADS_ROOT", &context.state.config.uploads_root)
            .arg("--session")
            .arg("ses_pat_A")
            .arg("--manifest")
            .arg(&mapping)
            .arg("--backup")
            .arg(&backup);
        if apply {
            command.arg("--apply");
        }
        command.output().unwrap()
    };
    let output = tokio::task::block_in_place(|| invoke(false));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!backup.exists());
    let original: f64 =
        sqlx::query_scalar("SELECT start_time FROM frame_records WHERE id='fra_legacy_000001'")
            .fetch_one(&context.state.pool)
            .await
            .unwrap();
    assert_eq!(original, 10.);
    let output = tokio::task::block_in_place(|| invoke(true));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let row = sqlx::query("SELECT * FROM frame_records WHERE id='fra_legacy_000001'")
        .fetch_one(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(row.get::<f64, _>("start_time"), 0.);
    assert_eq!(row.get::<f64, _>("end_time"), 0.2);
    assert!(row.get::<bool, _>("hidden"));
    assert_eq!(
        row.get::<Option<String>, _>("doc_note").as_deref(),
        Some("retain this review")
    );
    assert!(source.exists());
    let backups: Vec<_> = std::fs::read_dir(backup).unwrap().collect();
    assert_eq!(backups.len(), 1);
    assert!(backups[0]
        .as_ref()
        .unwrap()
        .path()
        .join("metadata.json")
        .exists());
    context.cleanup().await;
}

async fn seed_legacy_ledger(pool: &sqlx::PgPool) {
    sqlx::raw_sql("CREATE TABLE _sqlx_migrations(version BIGINT PRIMARY KEY,description TEXT NOT NULL,installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),success BOOLEAN NOT NULL,checksum BYTEA NOT NULL,execution_time BIGINT NOT NULL)").execute(pool).await.unwrap();
    let manifest: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../src/db/legacy_migrations.json")).unwrap();
    for item in manifest {
        sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES($1,$2,true,decode($3,'hex'),0)").bind(item["version"].as_i64().unwrap()).bind(item["description"].as_str().unwrap()).bind(item["checksum"].as_str().unwrap()).execute(pool).await.unwrap();
    }
}
async fn assert_legacy_history_unchanged(pool: &sqlx::PgPool) {
    let manifest: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../src/db/legacy_migrations.json")).unwrap();
    let actual:Vec<serde_json::Value>=sqlx::query_scalar("SELECT jsonb_build_object('version',version,'description',description,'checksum',encode(checksum,'hex')) FROM _sqlx_migrations WHERE version BETWEEN 1 AND 3 ORDER BY version").fetch_all(pool).await.unwrap();
    assert_eq!(actual, manifest);
}
#[tokio::test]
async fn unknown_or_modified_history_is_rejected_without_rewriting_it() {
    let context = TestContext::new().await;
    sqlx::query("INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time) VALUES(999,'unknown',true,decode('00','hex'),0)").execute(&context.state.pool).await.unwrap();
    assert!(
        ecg_backend::db::postgres::run_migrations(&context.state.pool)
            .await
            .unwrap_err()
            .1
            .contains("Unrecognized migration history")
    );
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=999")
        .execute(&context.state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE _sqlx_migrations SET checksum=decode('00','hex') WHERE version=$1")
        .bind(ecg_backend::db::postgres::SCHEMA_VERSION)
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert!(
        ecg_backend::db::postgres::run_migrations(&context.state.pool)
            .await
            .unwrap_err()
            .1
            .contains("migration failed")
    );
    context.cleanup().await;
}

#[tokio::test]
async fn readiness_checks_schema_metadata_and_separates_acquisition_mapping() {
    let context = TestContext::new().await;
    let report = ecg_backend::db::postgres::schema_report(&context.state.pool)
        .await
        .unwrap();
    assert!(report.ready);
    assert!(report.acquisition_ready);

    sqlx::query("ALTER TABLE accounts ADD COLUMN password_hash TEXT")
        .execute(&context.state.pool)
        .await
        .unwrap();
    let report = ecg_backend::db::postgres::schema_report(&context.state.pool)
        .await
        .unwrap();
    assert!(!report.ready);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue.contains("password_hash")));
    sqlx::query("ALTER TABLE accounts DROP COLUMN password_hash")
        .execute(&context.state.pool)
        .await
        .unwrap();

    sqlx::query("ALTER TABLE sessions DISABLE ROW LEVEL SECURITY")
        .execute(&context.state.pool)
        .await
        .unwrap();
    let report = ecg_backend::db::postgres::schema_report(&context.state.pool)
        .await
        .unwrap();
    assert!(!report.ready);
    assert!(report
        .issues
        .iter()
        .any(|issue| issue == "RLS disabled: sessions"));
    sqlx::query("ALTER TABLE sessions ENABLE ROW LEVEL SECURITY")
        .execute(&context.state.pool)
        .await
        .unwrap();

    sqlx::query("UPDATE devices SET hardware_id=NULL WHERE id='dev_A'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    let report = ecg_backend::db::postgres::schema_report(&context.state.pool)
        .await
        .unwrap();
    assert!(report.ready);
    assert!(!report.acquisition_ready);
    assert_eq!(report.unmapped_device_count, Some(1));
    context.cleanup().await;
}
#[tokio::test]
async fn duplicate_legacy_profiles_and_histories_are_preserved_but_ambiguous_login_is_denied() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    guard_database(&url).unwrap();
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let schema = format!("ecg_test_ambiguous_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = PgConnectOptions::from_str(&url)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/2026102701_baseline.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO accounts(id,email,role) VALUES('legacy','legacy@test.invalid','pasien'); INSERT INTO devices(id,name) VALUES('legacy_device','legacy_device'); INSERT INTO patients(id,account_id,first_name,last_name) VALUES('legacy_A','legacy','First','Patient'),('legacy_B','legacy','Different','Patient'); INSERT INTO sessions(id,device_id,patient_id,started_at,ended_at) VALUES('legacy_session_A','legacy_device','legacy_A',now(),now()),('legacy_session_B','legacy_device','legacy_B',now(),now())").execute(&pool).await.unwrap();
    seed_legacy_ledger(&pool).await;
    ecg_backend::db::postgres::run_migrations(&pool)
        .await
        .unwrap();
    assert_legacy_history_unchanged(&pool).await;
    assert!(ecg_backend::db::postgres::schema_ready(&pool)
        .await
        .unwrap());
    let names: Vec<String> = sqlx::query_scalar("SELECT first_name FROM patients ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, vec!["First", "Different"]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    let mut token_claims = claims("patient");
    token_claims.sub = "legacy".into();
    assert_eq!(
        ecg_backend::api::authorization::Identity::load(&pool, &token_claims)
            .await
            .unwrap_err()
            .0
            .as_u16(),
        409
    );
    let error=sqlx::query("INSERT INTO patients(id,account_id,first_name,last_name) VALUES('legacy_C','legacy','Third','Patient')").execute(&pool).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
