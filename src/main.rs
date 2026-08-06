mod models;
mod data;
mod network;
mod api;
mod db;
mod config;

use tracing::{info, error, Level};
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() {
    // 1. Inisialisasi Tracing/Logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Gagal mengatur global default tracing subscriber");

    info!("Memulai inisialisasi sistem medis (Mode Asinkron Axum + SQLite + SQLCipher)...");

    // 2. Muat Konfigurasi dari berkas .env
    let config = config::AppConfig::load();

    // 3. Inisialisasi Database Pool SQLite dengan enkripsi SQLCipher
    let pool = db::sqlite::create_pool(&config.db_path, &config.sqlite_key);

    // Lakukan auto-migration skema database pada saat startup
    {
        let conn = pool.get().expect("Gagal mendapatkan koneksi DB awal untuk migrasi");
        if let Err(e) = db::sqlite::run_migrations(&conn) {
            error!("Gagal menjalankan auto-migrations database: {}", e);
            panic!("Database migration failed: {}", e);
        }
        info!("Auto-migrations database SQLite berhasil diselesaikan.");
    }

    // 4. Buat daftar klien WebSocket (ClientList) asinkron yang thread-safe
    let clients = network::websocket::ClientList::default();

    // 5. Jalankan Pacer asinkron (pemotongan data EKG & forward ke WebSocket)
    let pacer_tx = network::pacer::start_pacer(clients.clone());

    // 6. Jalankan Background Database Worker untuk menulis data asinkron
    let db_tx = db::sqlite::start_db_worker(pool.clone());

    // 7. Jalankan MQTT Listener
    // Catatan: MQTT Listener berjalan di thread tersendiri
    let mqtt_pacer_tx = pacer_tx.clone();
    let mqtt_db_tx = db_tx.clone();
    let mqtt_client = network::mqtt_listener::start_mqtt_listener(
        &config.mqtt_broker,
        config.mqtt_port,
        &config.mqtt_topic,
        &config.mqtt_username,
        &config.mqtt_password,
        move |payload_str| {
            match serde_json::from_str::<models::device::DevicePayload>(&payload_str) {
                Ok(device_payload) => {
                    // Teruskan ke Pacer
                    if let Err(e) = mqtt_pacer_tx.send(device_payload.clone()) {
                        error!("[Main] Gagal mengirim data ke Pacer: {}", e);
                    }

                    // Teruskan ke Database Worker
                    if let Err(e) = mqtt_db_tx.send(device_payload) {
                        error!("[Main] Gagal mengirim data ke DB Worker: {}", e);
                    }
                }
                Err(e) => {
                    error!("[Main] Menerima JSON MQTT yang tidak valid atau tidak sesuai format: {}", e);
                }
            }
        }
    );

    // 8. Setup Router Axum untuk REST API + WebSocket
    let app_state = api::routes::AppState {
        pool: pool.clone(),
        mqtt_client: mqtt_client.clone(),
        jwt_secret: config.jwt_secret.clone(),
        api_url: format!("http://{}:{}", config.host_ip, config.rest_port),
    };

    let mut app = api::routes::create_router(app_state);
    
    // Pasang endpoint WebSocket pada root "/" dan "/ws" untuk mendukung proxy produksi
    app = app
        .route("/", axum::routing::get(network::websocket::ws_handler).with_state(clients.clone()))
        .route("/ws", axum::routing::get(network::websocket::ws_handler).with_state(clients.clone()));

    // 9. Jalankan Server HTTP & WebSocket
    let addr_ws = format!("{}:{}", config.host_ip, config.ws_port);
    let addr_rest = format!("{}:{}", config.host_ip, config.rest_port);

    if config.ws_port == config.rest_port {
        info!("Menjalankan server terpadu Axum di http://{}", addr_ws);
        let listener = tokio::net::TcpListener::bind(&addr_ws).await.unwrap();
        axum::serve(listener, app).await.unwrap();
    } else {
        let app_clone = app.clone();
        
        let ws_handle = tokio::spawn(async move {
            info!("Menjalankan server WebSocket di ws://{}", addr_ws);
            let listener = tokio::net::TcpListener::bind(&addr_ws).await.unwrap();
            axum::serve(listener, app_clone).await.unwrap();
        });

        let rest_handle = tokio::spawn(async move {
            info!("Menjalankan server REST API di http://{}", addr_rest);
            let listener = tokio::net::TcpListener::bind(&addr_rest).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });

        let _ = tokio::join!(ws_handle, rest_handle);
    }
}