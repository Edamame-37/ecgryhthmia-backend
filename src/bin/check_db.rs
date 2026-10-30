#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let url = std::env::var("DATABASE_URL")?;
    let pool = sqlx::PgPool::connect(&url).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM devices")
        .fetch_one(&pool)
        .await?;
    println!("Registered devices: {count}");
    Ok(())
}
