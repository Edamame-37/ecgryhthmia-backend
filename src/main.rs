use std::thread;

mod models;
mod data;
mod network;
mod api;
mod db;

fn main() {
    println!("Memulai inisialisasi sistem medis (Mode Live Pacing MQTT + SQLite)...");

    // 2. Buat daftar klien WebSocket (ClientList) yang thread-safe
    let clients = network::websocket::ClientList::default();
    
    // 3. Jalankan Pacer yang akan menerima kargo JSON raksasa, lalu memotong-motongnya ke ClientList
    let pacer_tx = network::pacer::start_pacer(clients.clone());

    // 4. Jalankan Background Database Worker untuk SQLite
    // File database akan otomatis terbuat di direktori utama
    let db_path = "database.db";
    let db_tx = db::sqlite::start_db_worker(db_path);

    // 5. Jalankan MQTT Listener
    // Catatan: Pastikan broker MQTT lokal (seperti Mosquitto) berjalan di localhost port 1883
    let mqtt_client = network::mqtt_listener::start_mqtt_listener(
        "93d81a02c1f743b6ab4ea22d7ad9c3e0.s1.eu.hivemq.cloud", 
        8883, 
        "ecgrhythmia/device01", 
        move |payload_str| {
            // Lakukan parsing JSON besar dari string mentah menjadi struktur DevicePayload
            match serde_json::from_str::<models::device::DevicePayload>(&payload_str) {
                Ok(device_payload) => {
                    // 5a. Teruskan struct ke antrean Pacer (Untuk UI Websocket)
                    if let Err(e) = pacer_tx.send(device_payload.clone()) {
                        eprintln!("[Main] Gagal mengirim data ke Pacer: {}", e);
                    }

                    // 5b. Teruskan struct yang sama ke antrean Database (Untuk Storage)
                    if let Err(e) = db_tx.send(device_payload) {
                        eprintln!("[Main] Gagal mengirim data ke DB Worker: {}", e);
                    }
                }
                Err(e) => {
                    eprintln!("[Main] Menerima JSON MQTT yang tidak valid atau format tidak sesuai: {}", e);
                }
            }
        }
    );

    // 1. Jalankan REST API Server di thread terpisah (Port 8081)
    let rest_mqtt_client = mqtt_client.clone();
    thread::spawn(move || {
        api::routes::start_rest_api("8081", rest_mqtt_client);
    });
    
    // 6. Jalankan WebSocket Server di thread utama (Port 8080)
    network::websocket::start_server(clients, "8080");
}