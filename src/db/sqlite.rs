use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use rusqlite::{params, Connection};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use crate::models::device::DevicePayload;
use tracing::{info, error};

pub type DbPool = Pool<SqliteConnectionManager>;

#[derive(Debug)]
pub struct SqlcipherCustomizer {
    pub key: String,
}

impl r2d2::CustomizeConnection<rusqlite::Connection, rusqlite::Error> for SqlcipherCustomizer {
    fn on_acquire(&self, conn: &mut rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(&format!("PRAGMA key = '{}';", self.key))?;
        Ok(())
    }
}

pub fn create_pool(db_path: &str, db_key: &str) -> DbPool {
    let manager = SqliteConnectionManager::file(db_path);
    let customizer = SqlcipherCustomizer { key: db_key.to_string() };
    Pool::builder()
        .connection_customizer(Box::new(customizer))
        .build(manager)
        .expect("Gagal membuat connection pool SQLite")
}

pub fn run_migrations(conn: &Connection, admin_email: &str, admin_password: &str) -> Result<(), rusqlite::Error> {
    let create_tables_query = "
        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            email TEXT UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            created_at TEXT NOT NULL,
            profile_photo TEXT,
            status TEXT DEFAULT 'Offline'
        );

        CREATE TABLE IF NOT EXISTS doctors (
            id TEXT PRIMARY KEY,
            account_id TEXT,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            FOREIGN KEY(account_id) REFERENCES accounts(id)
        );

        CREATE TABLE IF NOT EXISTS patients (
            id TEXT PRIMARY KEY,
            account_id TEXT,
            primary_doctor_id TEXT,
            first_name TEXT NOT NULL,
            last_name TEXT NOT NULL,
            date_of_birth TEXT NOT NULL,
            gender TEXT NOT NULL,
            FOREIGN KEY(account_id) REFERENCES accounts(id),
            FOREIGN KEY(primary_doctor_id) REFERENCES doctors(id)
        );

        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mac TEXT,
            battery INTEGER,
            status TEXT,
            assigned_to TEXT
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            device_id TEXT NOT NULL,
            patient_id TEXT,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            file_path TEXT NOT NULL,
            FOREIGN KEY(device_id) REFERENCES devices(id),
            FOREIGN KEY(patient_id) REFERENCES patients(id)
        );

        CREATE TABLE IF NOT EXISTS frame_records (
            id TEXT PRIMARY KEY,
            session_id TEXT,
            time_interval TEXT NOT NULL,
            confirmation INTEGER,
            doc_classification TEXT,
            FOREIGN KEY(session_id) REFERENCES sessions(id)
        );
    ";

    conn.execute_batch(create_tables_query)?;

    let _ = conn.execute("ALTER TABLE sessions DROP COLUMN confirmation;", params![]);
    let _ = conn.execute("ALTER TABLE sessions DROP COLUMN doc_classification;", params![]);
    let _ = conn.execute("ALTER TABLE accounts ADD COLUMN status TEXT DEFAULT 'Offline';", params![]);
    
    // Migrasi device_id ke patients
    let _ = conn.execute("ALTER TABLE patients ADD COLUMN device_id TEXT;", params![]);
    // Update existing assignments from devices table before dropping it!
    let _ = conn.execute("UPDATE patients SET device_id = (SELECT id FROM devices WHERE assigned_to = patients.id)", params![]);
    
    // Drop unused columns
    let _ = conn.execute("ALTER TABLE devices DROP COLUMN mac;", params![]);
    let _ = conn.execute("ALTER TABLE devices DROP COLUMN battery;", params![]);
    let _ = conn.execute("ALTER TABLE devices DROP COLUMN status;", params![]);
    let _ = conn.execute("ALTER TABLE devices DROP COLUMN assigned_to;", params![]);
    
    let _ = conn.execute(
        "INSERT OR IGNORE INTO devices (id, name) VALUES ('dev_001', 'device01');",
        params![]
    );

    if let Ok(admin_hash) = bcrypt::hash(admin_password, bcrypt::DEFAULT_COST) {
        let _ = conn.execute(
            "INSERT OR IGNORE INTO accounts (id, email, password_hash, role, created_at, status) VALUES ('acc_admin', ?1, ?2, 'admin', datetime('now'), 'Offline');",
            params![admin_email, admin_hash]
        );
    }

    Ok(())
}

pub fn start_db_worker(pool: DbPool) -> UnboundedSender<DevicePayload> {
    let (tx, mut rx) = unbounded_channel::<DevicePayload>();

    tokio::spawn(async move {
        info!("[Database] Background writer task berjalan...");
        let mut device_map: HashMap<String, String> = HashMap::new();
        let mut session_map: HashMap<String, String> = HashMap::new();

        while let Some(payload) = rx.recv().await {
            let conn = match pool.get() {
                Ok(c) => c,
                Err(e) => {
                    error!("[Database] Gagal mendapatkan koneksi dari pool: {}", e);
                    continue;
                }
            };

            // 1. Dapatkan atau buat device ID internal (dev...)
            let dev_id = if let Some(id) = device_map.get(&payload.device_id) {
                id.clone()
            } else {
                let db_dev_id: Result<String, _> = conn.query_row(
                    "SELECT id FROM devices WHERE name = ?1",
                    params![payload.device_id],
                    |row| row.get(0)
                );
                match db_dev_id {
                    Ok(id) => {
                        device_map.insert(payload.device_id.clone(), id.clone());
                        id
                    },
                    Err(_) => {
                        let new_id = generate_custom_id(&conn, "devices", "dev");
                        if let Err(e) = conn.execute(
                            "INSERT INTO devices (id, name) VALUES (?1, ?2)",
                            params![new_id, payload.device_id]
                        ) {
                            error!("[Database] Gagal INSERT device: {}", e);
                            continue;
                        }
                        device_map.insert(payload.device_id.clone(), new_id.clone());
                        new_id
                    }
                }
            };

            // 2. Dapatkan atau buat session ID internal (ses...)
            let ses_id = if let Some(id) = session_map.get(&payload.session_id) {
                id.clone()
            } else {
                let new_id = generate_custom_id(&conn, "sessions", "ses");
                session_map.insert(payload.session_id.clone(), new_id.clone());
                
                let initial_file_path = format!("records/{}.jsonl", new_id);
                let patient_id: Option<String> = conn.query_row(
                    "SELECT id FROM patients WHERE device_id = ?1",
                    params![dev_id],
                    |row| row.get(0)
                ).ok();

                if let Err(e) = conn.execute(
                    "INSERT INTO sessions (id, device_id, patient_id, started_at, file_path) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![new_id, dev_id, patient_id, payload.created_at, initial_file_path]
                ) {
                    error!("[Database] Gagal INSERT sesi: {}", e);
                    continue;
                }
                new_id
            };

            // 3. Tentukan path file berdasarkan internal session_id
            let file_path = format!("records/{}.jsonl", ses_id);

            // 4. Tulis raw JSON line ke dalam file secara berurutan (append)
            let json_string = match serde_json::to_string(&payload) {
                Ok(val) => val,
                Err(e) => {
                    error!("[Database] Gagal serialisasi payload ke JSON: {}", e);
                    continue;
                }
            };

            // Memastikan folder records ada
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
        }
    });

    tx
}

pub fn generate_custom_id(conn: &Connection, table: &str, prefix: &str) -> String {
    let expected_len = prefix.len() + 12;
    let query = format!(
        "SELECT id FROM {} WHERE id LIKE '{}%' AND LENGTH(id) = {} ORDER BY id DESC LIMIT 1", 
        table, prefix, expected_len
    );
    let last_id: String = conn.query_row(&query, [], |row| row.get(0)).unwrap_or_else(|_| format!("{}000000000000", prefix));
    
    if last_id.starts_with(prefix) && last_id.len() == expected_len {
        if let Ok(num) = last_id[prefix.len()..].parse::<i64>() {
            return format!("{}{:012}", prefix, num + 1);
        }
    }
    format!("{}000000000001", prefix)
}
