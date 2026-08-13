use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use crate::models::device::DevicePayload;
use tracing::{info, error};

pub async fn create_pool(database_url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await
        .expect("Gagal terhubung ke Supabase PostgreSQL")
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::Error> {
    let create_tables_query = "
        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            email TEXT UNIQUE NOT NULL,
            role TEXT NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
            profile_photo TEXT,
            status TEXT DEFAULT 'Offline'
        );

        CREATE TABLE IF NOT EXISTS doctors (
            id TEXT PRIMARY KEY,
            account_id TEXT REFERENCES accounts(id),
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS patients (
            id TEXT PRIMARY KEY,
            account_id TEXT REFERENCES accounts(id),
            primary_doctor_id TEXT REFERENCES doctors(id),
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            date_of_birth TEXT NOT NULL,
            gender TEXT NOT NULL,
            device_id TEXT
        );

        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mqtt_broker TEXT,
            mqtt_port INT,
            mqtt_topic TEXT,
            mqtt_username TEXT,
            mqtt_password TEXT
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL REFERENCES devices(id),
            patient_id TEXT REFERENCES patients(id),
            started_at TEXT NOT NULL,
            ended_at TEXT,
            file_path TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS frame_records (
            id TEXT PRIMARY KEY,
            session_id TEXT REFERENCES sessions(id),
            time_interval TEXT NOT NULL
        );
    ";

    sqlx::query(create_tables_query).execute(pool).await?;

    let _ = sqlx::query(
        "INSERT INTO devices (id, name, mqtt_broker, mqtt_port, mqtt_topic, mqtt_username, mqtt_password)
         VALUES ('dev_001', 'device01', '93d81a02c1f743b6ab4ea22d7ad9c3e0.s1.eu.hivemq.cloud', 8883, 'ecgrhythmia/device01', 'ecg-undip', 'undipjaya')
         ON CONFLICT (id) DO UPDATE SET 
            mqtt_broker = EXCLUDED.mqtt_broker, 
            mqtt_port = EXCLUDED.mqtt_port, 
            mqtt_topic = EXCLUDED.mqtt_topic, 
            mqtt_username = EXCLUDED.mqtt_username, 
            mqtt_password = EXCLUDED.mqtt_password;"
    ).execute(pool).await?;

    Ok(())
}

pub async fn generate_custom_id(pool: &PgPool, table: &str, prefix: &str) -> String {
    let expected_len = prefix.len() + 12;
    // We cannot use bound parameters for table names in Postgres, so we format securely.
    let query_str = format!(
        "SELECT id FROM {} WHERE id LIKE '{}%' AND LENGTH(id) = {} ORDER BY id DESC LIMIT 1",
        table, prefix, expected_len
    );
    
    // Instead of sqlx::query! macro, we use sqlx::query to allow dynamic SQL strings for this specific helper.
    let res: Result<(String,), _> = sqlx::query_as(&query_str).fetch_one(pool).await;

    let last_id = match res {
        Ok((id,)) => id,
        Err(_) => format!("{}000000000000", prefix),
    };

    if last_id.starts_with(prefix) && last_id.len() == expected_len {
        if let Ok(num) = last_id[prefix.len()..].parse::<i64>() {
            return format!("{}{:012}", prefix, num + 1);
        }
    }
    format!("{}000000000001", prefix)
}

pub fn start_db_worker(pool: PgPool, pacer_tx: UnboundedSender<DevicePayload>) -> UnboundedSender<DevicePayload> {
    let (tx, mut rx) = unbounded_channel::<DevicePayload>();

    tokio::spawn(async move {
        info!("[Database] Background writer task berjalan...");
        let mut device_map: HashMap<String, String> = HashMap::new();
        let mut session_map: HashMap<String, String> = HashMap::new();

        while let Some(mut payload) = rx.recv().await {
            // 1. Dapatkan atau buat device ID internal
            let dev_id = if let Some(id) = device_map.get(&payload.device_id) {
                id.clone()
            } else {
                // Try to find the device
                let dev_res = sqlx::query!("SELECT id FROM devices WHERE name = $1", payload.device_id)
                    .fetch_one(&pool)
                    .await;
                
                match dev_res {
                    Ok(record) => {
                        device_map.insert(payload.device_id.clone(), record.id.clone());
                        record.id
                    },
                    Err(_) => {
                        let new_id = generate_custom_id(&pool, "devices", "dev").await;
                        if let Err(e) = sqlx::query!("INSERT INTO devices (id, name) VALUES ($1, $2)", new_id, payload.device_id)
                            .execute(&pool)
                            .await 
                        {
                            error!("[Database] Gagal INSERT device: {}", e);
                            continue;
                        }
                        device_map.insert(payload.device_id.clone(), new_id.clone());
                        new_id
                    }
                }
            };

            // 2. Dapatkan atau buat session ID internal
            let ses_id = if let Some(id) = session_map.get(&payload.session_id) {
                id.clone()
            } else {
                let new_id = generate_custom_id(&pool, "sessions", "ses").await;
                session_map.insert(payload.session_id.clone(), new_id.clone());
                
                let initial_file_path = format!("records/{}.jsonl", new_id);
                
                let patient_res = sqlx::query!("SELECT id FROM patients WHERE device_id = $1", dev_id)
                    .fetch_one(&pool)
                    .await;
                let patient_id = patient_res.map(|r| r.id).ok();

                if let Err(e) = sqlx::query!(
                    "INSERT INTO sessions (id, device_id, patient_id, started_at, file_path) VALUES ($1, $2, $3, $4, $5)",
                    new_id, dev_id, patient_id, chrono::DateTime::parse_from_rfc3339(&payload.created_at).unwrap_or_else(|_| chrono::Utc::now().into()).with_timezone(&chrono::Utc), initial_file_path
                ).execute(&pool).await {
                    error!("[Database] Gagal INSERT sesi: {}", e);
                    continue;
                }
                new_id
            };

            payload.session_id = ses_id.clone();
            let file_path = format!("records/{}.jsonl", ses_id);

            let json_string = match serde_json::to_string(&payload) {
                Ok(val) => val,
                Err(e) => {
                    error!("[Database] Gagal serialisasi payload ke JSON: {}", e);
                    continue;
                }
            };

            if let Some(parent) = std::path::Path::new(&file_path).parent() {
                if !parent.exists() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }

            let mut file = match OpenOptions::new().create(true).append(true).open(&file_path) {
                Ok(f) => f,
                Err(e) => {
                    error!("[Database] Gagal membuka file rekaman {}: {}", file_path, e);
                    continue;
                }
            };

            if let Err(e) = writeln!(file, "{}", json_string) {
                error!("[Database] Gagal menulis baris ke file {}: {}", file_path, e);
                continue;
            }

            let _ = pacer_tx.send(payload);
        }
    });

    tx
}
