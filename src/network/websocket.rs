/**
 * @fileoverview Modul Network: WebSocket Server (Rust)
 * Mengurus transmisi streaming data EKG real-time ke Frontend React.
 * 
 * UPDATE: 
 * 1. Penghapusan total multiplier kalibrasi (Data sudah murni mV).
 * 2. Penambahan sinkronisasi frekuensi (FS Delay) untuk streaming real-time.
 */

use std::net::TcpListener;
use std::thread;
use std::time::Duration;
use tungstenite::{accept, Message};
use crate::models::payload::{ECGDataPayload, RawECGData, ServerMessage};

pub fn start_server(data: RawECGData, port: &str) {
    let address = format!("127.0.0.1:{}", port);
    let server = TcpListener::bind(&address).expect("Gagal mem-bind port WebSocket");
    
    println!("=================================================");
    println!("SERVER EKG RUST BERJALAN DI ws://{}", address);
    println!("Mode: Real-Time Streaming (Murni Milivolt)");
    println!("Menunggu koneksi dari Frontend React...");
    println!("=================================================");

    for stream in server.incoming() {
        match stream {
            Ok(stream) => {
                let data_clone = data.clone();
                // Spawn thread mandiri agar server bisa melayani banyak tab browser/klien
                thread::spawn(move || {
                    handle_client(stream, data_clone);
                });
            }
            Err(e) => {
                eprintln!("[Network] Error menerima koneksi: {}", e);
            }
        }
    }
}

fn handle_client(stream: std::net::TcpStream, data: RawECGData) {
    let mut websocket = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[Network] Gagal melakukan handshake WebSocket: {}", e);
            return;
        }
    };
    
    println!("[Network] Frontend Terhubung! Memulai transmisi sinyal real-time...");

    // ============================================================
    // KONFIGURASI SINKRONISASI FREKUENSI (FS)
    // ============================================================
    // Asumsi dataset/CSV yang dikirim menggunakan FS 250 Hz
    let fs = 250.0; 
    
    // Alih-alih menembakkan 1 titik demi 1 titik (overhead terlalu tinggi),
    // kita mengirim paket berisi "chunk" (potongan) data. 
    // 25 titik pada 250Hz = setara dengan durasi 100 milidetik per paket.
    let chunk_size = 25; 
    let sleep_duration = Duration::from_millis((1000.0 * chunk_size as f64 / fs) as u64);
    
    let total_samples = data.time.len();

    // Loop tanpa batas untuk mensimulasikan alat EKG medis yang menyala terus-menerus
    loop {
        let mut i = 0;
        
        while i < total_samples {
            let end = std::cmp::min(i + chunk_size, total_samples);

            // Mengekstrak sebagian data (Tanpa ada lagi perkalian v_ref/gain)
            let chunk_data = RawECGData {
                time: data.time[i..end].to_vec(),
                ch1: data.ch1[i..end].to_vec(), 
                ch2: data.ch2[i..end].to_vec(),
                ch3: data.ch3[i..end].to_vec(),
            };

            let payload = ECGDataPayload {
                raw: chunk_data,
                classification_result: "NORM".to_string(), // Teks statis, jika inferensi dipindah ke UI
                confidence: "0.99".to_string(),
                anomaly_indices: vec![],
            };

            let msg = ServerMessage {
                r#type: "live_data".to_string(), // Menandakan aliran kontinu ke React Hook
                measurement_id: Some("MEAS-LIVE-01".to_string()),
                device_id: Some("UNDIP-ECG-01".to_string()),
                timestamp: Some("Live-Stream".to_string()),
                sha256_checksum: Some("bypass".to_string()), 
                data_payload: Some(payload),
                data: None, // <--- TAMBAHKAN BARIS INI
                message: None,
            };

            // Serialisasi ke JSON
            let json_string = match serde_json::to_string(&msg) {
                Ok(json) => json,
                Err(e) => {
                    eprintln!("[Network] Gagal membentuk JSON: {}", e);
                    break;
                }
            };
            
            // Transmisi ke WebSocket Client (React)
            if let Err(e) = websocket.send(Message::Text(json_string)) {
                eprintln!("[Network] Koneksi ditutup oleh Frontend: {}", e);
                return; // Keluar dari fungsi jika tab ditutup
            }

            // Jeda Waktu agar kecepatan streaming cocok dengan 25 mm/s (Real-time Pacing)
            thread::sleep(sleep_duration);
            
            i = end;
        }
        
        // Jeda sebentar sebelum mengulang dari awal file CSV (Looping Simulasi)
        thread::sleep(Duration::from_millis(500));
    }
}