use ecg_backend::{
    api::routes::{create_router, AppState},
    config::AppConfig,
    db::postgres,
    network::mqtt_listener,
    services::{ingestion, recording},
};
use std::error::Error;
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let config = AppConfig::load().map_err(|error| error.1)?;
    let pool = postgres::create_pool(&config.database_url)
        .await
        .map_err(|error| error.1)?;
    let migrate_only = std::env::args().any(|argument| argument == "--migrate-only");
    if config.apply_migrations || migrate_only {
        postgres::run_migrations(&pool)
            .await
            .map_err(|error| error.1)?;
    }
    if migrate_only {
        tracing::info!("Migrations completed");
        return Ok(());
    }
    let report = postgres::schema_report(&pool)
        .await
        .map_err(|error| error.1)?;
    if !report.ready {
        return Err(format!(
            "Database schema is not ready: {}. Review docs/schema-upgrade.md and run the read-only check_db preflight",
            report.issues.join("; ")
        )
        .into());
    }
    tokio::fs::create_dir_all(&config.records_root).await?;
    tokio::fs::create_dir_all(&config.uploads_root).await?;
    let rest_address = format!("{}:{}", config.host_ip, config.rest_port);
    let ws_address = format!("{}:{}", config.host_ip, config.ws_port);
    let state = AppState::new(pool, config);
    let sender = ingestion::start(state.clone());
    let unmapped = report.unmapped_device_count.unwrap_or_default();
    if unmapped > 0 {
        tracing::warn!(
            count = unmapped,
            "Device listeners disabled until hardware identities are reviewed"
        );
    }
    let device_ids: Vec<String> = sqlx::query_scalar("SELECT c.device_id FROM device_credentials c JOIN devices d ON d.id=c.device_id WHERE d.hardware_id IS NOT NULL")
        .fetch_all(&state.pool)
        .await?;
    for id in device_ids {
        if let Err(error) = mqtt_listener::connect_device(state.clone(), &id, sender.clone()).await
        {
            tracing::error!(device_id=id,error=?error,"Device listener could not start")
        }
    }
    let housekeeping = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(error) = ecg_backend::services::storage::process(&housekeeping).await {
                tracing::error!(?error, "Storage reconciliation failed");
            }
            if let Err(error) = recording::expire_commands(&housekeeping).await {
                tracing::error!(?error, "Command timeout reconciliation failed")
            }
        }
    });
    let app = create_router(state);
    let rest_listener = tokio::net::TcpListener::bind(&rest_address).await?;
    tracing::info!(%rest_address,"REST listener ready");
    if ws_address == rest_address {
        axum::serve(rest_listener, app)
            .with_graceful_shutdown(shutdown())
            .await?;
    } else {
        let ws_listener = tokio::net::TcpListener::bind(&ws_address).await?;
        tracing::info!(%ws_address,"WebSocket listener ready");
        tokio::try_join!(
            axum::serve(rest_listener, app.clone()).with_graceful_shutdown(shutdown()),
            axum::serve(ws_listener, app).with_graceful_shutdown(shutdown())
        )?;
    }
    Ok(())
}
async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
