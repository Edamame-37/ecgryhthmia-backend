use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{process::ExitCode, str::FromStr};

#[tokio::main]
async fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL")?;
    let options =
        PgConnectOptions::from_str(&url)?.options([("default_transaction_read_only", "on")]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    let report = ecg_backend::db::postgres::schema_report(&pool)
        .await
        .map_err(|error| std::io::Error::other(error.1))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    pool.close().await;
    Ok(ExitCode::from(if !report.ready {
        1
    } else if !report.acquisition_ready {
        2
    } else {
        0
    }))
}
