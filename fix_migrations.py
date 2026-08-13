import re

with open("src/db/postgres.rs", "r", encoding="utf-8") as f:
    content = f.read()

# Replace run_migrations function
old_run_migrations = re.search(r'pub async fn run_migrations.*?Ok\(\(\)\)\n}', content, re.DOTALL)
if old_run_migrations:
    new_run_migrations = """pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    let queries = [
        "CREATE TABLE IF NOT EXISTS accounts (id TEXT PRIMARY KEY, email TEXT UNIQUE NOT NULL, role TEXT NOT NULL, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP, profile_photo TEXT, status TEXT DEFAULT 'Offline')",
        "CREATE TABLE IF NOT EXISTS doctors (id TEXT PRIMARY KEY, account_id TEXT REFERENCES accounts(id), first_name TEXT NOT NULL, last_name TEXT NOT NULL, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)",
        "CREATE TABLE IF NOT EXISTS patients (id TEXT PRIMARY KEY, account_id TEXT REFERENCES accounts(id), first_name TEXT NOT NULL, last_name TEXT NOT NULL, date_of_birth TEXT NOT NULL, gender TEXT, primary_doctor_id TEXT REFERENCES doctors(id), device_id TEXT, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)",
        "CREATE TABLE IF NOT EXISTS devices (id TEXT PRIMARY KEY, name TEXT NOT NULL UNIQUE, mqtt_broker TEXT, mqtt_port INTEGER, mqtt_topic TEXT, mqtt_username TEXT, mqtt_password TEXT, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)",
        "CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, device_id TEXT NOT NULL REFERENCES devices(id), patient_id TEXT NOT NULL REFERENCES patients(id), started_at TIMESTAMP WITH TIME ZONE NOT NULL, ended_at TIMESTAMP WITH TIME ZONE, file_path TEXT, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)",
        "CREATE TABLE IF NOT EXISTS frame_records (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, time_interval TEXT NOT NULL, created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)",
        "CREATE TABLE IF NOT EXISTS annotations (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), start_time DOUBLE PRECISION NOT NULL, end_time DOUBLE PRECISION NOT NULL, label TEXT NOT NULL, notes TEXT, created_by TEXT REFERENCES accounts(id), created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP)"
    ];

    for q in queries.iter() {
        sqlx::query(*q).execute(pool).await?;
    }
    
    Ok(())
}"""
    content = content.replace(old_run_migrations.group(0), new_run_migrations)

with open("src/db/postgres.rs", "w", encoding="utf-8") as f:
    f.write(content)
