use std::thread;
use std::sync::mpsc::{channel, Sender, Receiver};
use std::collections::HashMap;
use rusqlite::{params, Connection};
use std::fs::OpenOptions;
use std::io::Write;
use crate::models::device::DevicePayload;

pub fn open_encrypted_db(db_path: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(db_path)?;
    dotenvy::dotenv().ok();
    let db_key = std::env::var("SQLITE_KEY").unwrap_or_else(|_| "ecgrhythmia-super-secret-key-2026".to_string());
    conn.execute_batch(&format!("PRAGMA key = '{}';", db_key))?;
    Ok(conn)
}

pub fn start_db_worker(db_path: &str) -> Sender<DevicePayload> {
    let (tx, rx): (Sender<DevicePayload>, Receiver<DevicePayload>) = channel();
    let db_path_str = db_path.to_string();

    thread::spawn(move || {
        println!("[Database] Menghubungkan ke SQLite lokal: {}...", db_path_str);
        
        let conn = match Connection::open(&db_path_str) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[Database] Gagal membuka file SQLite: {}", e);
                return;
            }
        };

        // Load DB Key from .env or use fallback
        dotenvy::dotenv().ok();
        let db_key = std::env::var("SQLITE_KEY").unwrap_or_else(|_| "ecgrhythmia-super-secret-key-2026".to_string());
        
        // Aktifkan enkripsi SQLCipher
        if let Err(e) = conn.execute_batch(&format!("PRAGMA key = '{}';", db_key)) {
            eprintln!("[Database] Gagal mengatur kunci enkripsi: {}", e);
            return;
        }

        // Auto-Migration: Membuat tabel jika belum ada
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
                confirmation INTEGER,
                doc_classification TEXT,
                FOREIGN KEY(device_id) REFERENCES devices(id),
                FOREIGN KEY(patient_id) REFERENCES patients(id)
            );
        ";

        if let Err(e) = conn.execute_batch(create_tables_query) {
            eprintln!("[Database] Gagal membuat tabel-tabel: {}", e);
            return;
        }

        // Jalankan migrasi kolom bila tabel lama sudah ada namun belum memiliki kolom baru ini
        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN confirmation INTEGER;", params![]);
        let _ = conn.execute("ALTER TABLE sessions ADD COLUMN doc_classification TEXT;", params![]);
        
        let _ = conn.execute("ALTER TABLE devices ADD COLUMN mac TEXT;", params![]);
        let _ = conn.execute("ALTER TABLE devices ADD COLUMN battery INTEGER;", params![]);
        let _ = conn.execute("ALTER TABLE devices ADD COLUMN status TEXT;", params![]);
        let _ = conn.execute("ALTER TABLE devices ADD COLUMN assigned_to TEXT;", params![]);
        let _ = conn.execute("ALTER TABLE accounts ADD COLUMN status TEXT DEFAULT 'Offline';", params![]);
        
        // Pre-register configured devices so they appear online immediately
        let _ = conn.execute(
            "INSERT OR IGNORE INTO devices (id, name, mac, battery, status, assigned_to) VALUES ('dev_001', 'device01', '00:1A:2B:3C:4D:5E', 100, 'Active', 'pat000000000001');",
            params![]
        );

        println!("[Database] Seluruh tabel siap. Menunggu data masuk...");

        let mut device_map: HashMap<String, String> = HashMap::new();
        let mut session_map: HashMap<String, String> = HashMap::new();

        // Background loop untuk menerima dan menyimpan data
        for payload in rx {
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
                            "INSERT INTO devices (id, name, mac, battery, status, assigned_to) VALUES (?1, ?2, 'Unknown', 100, 'Active', 'Unassigned')",
                            params![new_id, payload.device_id]
                        ) {
                            eprintln!("[Database] Gagal INSERT device: {}", e);
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
                if let Err(e) = conn.execute(
                    "INSERT INTO sessions (id, device_id, patient_id, started_at, file_path) VALUES (?1, ?2, NULL, ?3, ?4)",
                    params![new_id, dev_id, payload.created_at, initial_file_path]
                ) {
                    eprintln!("[Database] Gagal INSERT sesi: {}", e);
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
                    eprintln!("[Database] Gagal serialisasi payload ke JSON: {}", e);
                    continue;
                }
            };

            let mut file = match OpenOptions::new().create(true).append(true).open(&file_path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("[Database] Gagal membuka file rekaman: {}", e);
                    continue;
                }
            };

            if let Err(e) = writeln!(file, "{}", json_string) {
                eprintln!("[Database] Gagal menulis baris ke file {}: {}", file_path, e);
                continue;
            }
        }
    });

    tx
}

pub fn generate_custom_id(conn: &Connection, table: &str, prefix: &str) -> String {
    let query = format!("SELECT id FROM {} ORDER BY id DESC LIMIT 1", table);
    let last_id: String = conn.query_row(&query, [], |row| row.get(0)).unwrap_or_else(|_| format!("{}000000000000", prefix));
    
    if last_id.len() > prefix.len() {
        if let Ok(num) = last_id[prefix.len()..].parse::<i64>() {
            return format!("{}{:012}", prefix, num + 1);
        }
    }
    format!("{}000000000001", prefix)
}
