#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await?;
    println!("Recorded sessions: {count}");
    Ok(())
}
