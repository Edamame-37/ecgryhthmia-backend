use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let url = "postgresql://postgres:bapakkauperangbarengidf@aws-0-ap-southeast-1.pooler.supabase.com:5432/postgres"; // Assuming Session Pooler, but I can use env var or hardcode it since the user put it in .env
    
    // I can just read the URL from .env
    let env_content = std::fs::read_to_string("../.env").unwrap();
    let db_url = env_content.lines().find(|l| l.starts_with("DATABASE_URL=")).unwrap().split('=').nth(1).unwrap().trim_matches('"');
    
    let pool = PgPoolOptions::new().max_connections(2).connect(db_url).await?;
    
    let queries = [
        "CREATE TABLE IF NOT EXISTS frame_records (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            time_interval TEXT NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        )",
    ];
    
    for q in queries.iter() {
        pool.execute(*q).await?;
    }
    println!("Tabel frame_records berhasil dibuat!");
    Ok(())
}
