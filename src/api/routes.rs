/**
 * @fileoverview Modul API: REST Routes (Zero-Dependency)
 * Menyediakan endpoint HTTP murni untuk memindai folder dataset 
 * dan mengembalikannya sebagai JSON ke Frontend React.
 * Mengadopsi fungsionalitas `get_available_records()` dari Python lama.
 */

use std::net::{TcpListener, TcpStream};
use std::io::{Read, Write};
use std::thread;
use std::fs;
use serde::Serialize;

#[derive(Serialize)]
pub struct AvailableRecords {
    pub chapman: Vec<String>,
    pub ptbxl_100hz: Vec<String>,
    pub ptbxl_500hz: Vec<String>,
    pub prosim_simulator: Vec<String>,
    pub sensor_records: Vec<String>,
}

pub fn start_rest_api(port: &str) {
    let address = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&address).expect("Gagal mem-bind port REST API");
    
    println!("[REST API] Server HTTP berjalan di http://{}/api/records", address);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Thread terpisah agar request HTTP tidak mengganggu streaming WebSocket
                thread::spawn(|| {
                    handle_http_client(stream);
                });
            }
            Err(e) => eprintln!("[REST API] Error menerima koneksi: {}", e),
        }
    }
}

fn handle_http_client(mut stream: TcpStream) {
    let mut buffer = [0; 1024];
    if let Ok(bytes_read) = stream.read(&mut buffer) {
        if bytes_read == 0 { return; }
        
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);
        
        // CORS Headers -> Sangat wajib agar Axios/Fetch dari React (port 5173) tidak diblokir
        let cors_headers = "Access-Control-Allow-Origin: *\r\n\
                            Access-Control-Allow-Methods: GET, OPTIONS\r\n\
                            Access-Control-Allow-Headers: Content-Type\r\n";

        // Menangani Preflight Request dari Browser (OPTIONS)
        if request.starts_with("OPTIONS") {
            let response = format!("HTTP/1.1 204 No Content\r\n{}\r\n", cors_headers);
            let _ = stream.write_all(response.as_bytes());
            return;
        }

        // Menangani Endpoint Utama (GET /api/records)
        if request.starts_with("GET /api/records") {
            let records = scan_directories();
            let json_response = serde_json::to_string(&records).unwrap_or_else(|_| "{}".to_string());
            
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                Content-Type: application/json\r\n\
                {}\
                Content-Length: {}\r\n\
                \r\n\
                {}",
                cors_headers,
                json_response.len(),
                json_response
            );
            
            let _ = stream.write_all(response.as_bytes());
        } else {
            // Jika rute tidak ditemukan
            let response = format!("HTTP/1.1 404 Not Found\r\n{}\r\n", cors_headers);
            let _ = stream.write_all(response.as_bytes());
        }
    }
}

fn scan_directories() -> AvailableRecords {
    let mut records = AvailableRecords {
        chapman: Vec::new(),
        ptbxl_100hz: Vec::new(),
        ptbxl_500hz: Vec::new(),
        prosim_simulator: Vec::new(),
        sensor_records: Vec::new(),
    };

    // PENTING: Karena Backend Rust kita sekarang memakan CSV (bukan .mat atau .hea),
    // scanner ini dikonfigurasi untuk mencari ekstensi .csv yang sudah Anda konversi.
    let base_dir = "../dataset"; // Menunjuk ke root dataset Anda
    
    // --- 1. Scan Chapman ---
    let chapman_path = format!("{}/chapman/sample", base_dir);
    if let Ok(entries) = fs::read_dir(&chapman_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".csv") {
                records.chapman.push(name.replace(".csv", ""));
            }
        }
    }

    // --- 2. Scan PTB-XL (500Hz) ---
    let ptbxl_path = format!("{}/ptbxl/sample_500hz", base_dir);
    if let Ok(entries) = fs::read_dir(&ptbxl_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".csv") {
                records.ptbxl_500hz.push(name.replace(".csv", ""));
            }
        }
    }

    // --- 3. Scan ProSim Simulator ---
    let prosim_path = format!("{}/Kalibrasi Prosim", base_dir);
    if let Ok(entries) = fs::read_dir(&prosim_path) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    records.prosim_simulator.push(entry.file_name().to_string_lossy().to_string());
                }
            }
        }
    }

    // Mengurutkan secara alfabetis agar tampilan di UI React rapi
    records.chapman.sort();
    records.ptbxl_500hz.sort();
    records.prosim_simulator.sort();

    records
}   