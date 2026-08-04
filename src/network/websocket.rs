/**
 * @fileoverview Modul Network: WebSocket Server (Rust)
 * Mengurus transmisi streaming data EKG real-time ke Frontend React.
 * 
 * UPDATE: 
 * Mengubah dari mode simulasi CSV (Pacing) ke mode Sinkronisasi MQTT Live.
 * Menerima daftar channel sender dan mendaftarkan klien baru.
 */

use std::net::TcpListener;
use std::thread;
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;
use tungstenite::{accept, Message};

pub type ClientList = Arc<Mutex<Vec<Sender<String>>>>;

pub fn start_server(clients: ClientList, port: &str) {
    let address = format!("127.0.0.1:{}", port);
    let server = TcpListener::bind(&address).expect("Gagal mem-bind port WebSocket");
    
    println!("=================================================");
    println!("SERVER EKG RUST BERJALAN DI ws://{}", address);
    println!("Mode: Real-Time Live MQTT (Forwarding)");
    println!("Menunggu koneksi dari Frontend React...");
    println!("=================================================");

    for stream in server.incoming() {
        match stream {
            Ok(stream) => {
                let clients_clone = clients.clone();
                // Spawn thread mandiri agar server bisa melayani banyak tab browser/klien
                thread::spawn(move || {
                    handle_client(stream, clients_clone);
                });
            }
            Err(e) => {
                eprintln!("[Network] Error menerima koneksi: {}", e);
            }
        }
    }
}

fn handle_client(stream: std::net::TcpStream, clients: ClientList) {
    let mut websocket = match accept(stream) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[Network] Gagal melakukan handshake WebSocket: {}", e);
            return;
        }
    };
    
    println!("[Network] Frontend Terhubung! Menyiapkan penerimaan data dari MQTT...");

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    
    {
        // Daftarkan sender channel ini ke dalam daftar klien global
        let mut clients_lock = clients.lock().unwrap();
        clients_lock.push(tx);
    }
    
    // Loop penerimaan pesan dari channel MQTT
    // Selama ada data dari MQTT (via rx), kirimkan ke klien WebSocket
    for msg in rx {
        if let Err(e) = websocket.send(Message::Text(msg)) {
            eprintln!("[Network] Koneksi ditutup oleh Frontend ({}). Menghapus klien...", e);
            break;
        }
    }
}