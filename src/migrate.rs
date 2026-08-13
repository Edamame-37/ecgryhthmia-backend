use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let url = "postgresql://postgres:bapakkauperangbarengidf@db.xzjxkplsgzcvdcjdhpcp.supabase.co:6543/postgres";
    let pool = PgPoolOptions::new().max_connections(2).connect(url).await?;
    
    let query = "
        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            email TEXT UNIQUE NOT NULL,
            role TEXT NOT NULL DEFAULT 'viewer',
            first_name TEXT,
            last_name TEXT,
            profile_photo TEXT,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS patients (
            id TEXT PRIMARY KEY,
            account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            date_of_birth TEXT NOT NULL,
            device_id TEXT,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            mqtt_broker TEXT,
            mqtt_port INTEGER,
            mqtt_topic TEXT,
            mqtt_username TEXT,
            mqtt_password TEXT,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            patient_id TEXT NOT NULL REFERENCES patients(id) ON DELETE CASCADE,
            started_at TIMESTAMP WITH TIME ZONE NOT NULL,
            ended_at TIMESTAMP WITH TIME ZONE,
            file_path TEXT,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
        CREATE TABLE IF NOT EXISTS annotations (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            start_time DOUBLE PRECISION NOT NULL,
            end_time DOUBLE PRECISION NOT NULL,
            label TEXT NOT NULL,
            notes TEXT,
            created_by TEXT REFERENCES accounts(id) ON DELETE SET NULL,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP
        );
    ";
    
    sqlx::query(query).execute(&pool).await?;
    println!("Migrasi database berhasil!");
    Ok(())
}
