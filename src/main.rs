use std::thread;

mod models;
mod data;
mod network;
mod api; // Mendaftarkan modul API yang baru kita buat

fn main() {
    let file_path = "../frame_000001_mv.csv"; 
    
    println!("Memulai inisialisasi sistem medis...");
    
    // 1. Jalankan REST API Server di thread terpisah (Port 8081)
    thread::spawn(|| {
        api::routes::start_rest_api("8081");
    });
    
    // 2. Baca file CSV EKG
    println!("Membaca data simulasi EKG dari {}...", file_path);
    match data::csv_reader::read_ecg_data(file_path) {
        Ok(ecg_data) => {
            let total_rows = ecg_data.ch1.len();
            println!("Sukses memuat {} baris data dari CSV.", total_rows);
            
            // 3. Jalankan WebSocket Server di thread utama (Port 8080)
            network::websocket::start_server(ecg_data, "8080");
        }
        Err(e) => {
            eprintln!("GAGAL: Tidak dapat membaca file CSV: {}", e);
            eprintln!("Pastikan file '{}' ada di folder yang sama dengan executable.", file_path);
        }
    }
}