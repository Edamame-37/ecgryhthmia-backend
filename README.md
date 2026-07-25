# ECG Rhythmia Backend

Backend server untuk streaming data Elektrokardiogram (EKG) real-time yang ditulis dalam bahasa Rust. Proyek ini bertugas untuk membaca dataset EKG (CSV) dan mentransmisikannya ke frontend (React) melalui protokol WebSocket dengan simulasi kecepatan real-time, serta menyediakan REST API untuk memindai dataset yang tersedia.

## Arsitektur Proyek

Struktur folder dan file pada proyek ini dirancang secara modular:

```
c:\ecgrhythmia-backend\
├── Cargo.toml          # File konfigurasi Cargo (dependensi dan meta-data proyek)
├── Cargo.lock          # Lock file untuk versi dependensi
├── frame_000001_mv.csv # Contoh file dataset EKG
└── src/
    ├── main.rs         # Titik masuk (entry point) utama aplikasi
    ├── api/
    │   ├── mod.rs
    │   └── routes.rs   # Menyediakan HTTP REST API murni untuk scan dataset
    ├── data/
    │   ├── mod.rs
    │   └── csv_reader.rs # Modul untuk membaca dan mem-parsing data CSV EKG
    ├── models/
    │   ├── mod.rs
    │   └── payload.rs  # Definisi struktur data (Payload/JSON) yang dikirim ke Frontend
    └── network/
        ├── mod.rs
        ├── mqtt_listener.rs # (Opsional) Modul untuk subscribe data dari MQTT Broker (mis. Mosquitto)
        └── websocket.rs # WebSocket Server untuk streaming data real-time ke Frontend
```

## Fitur Utama

1. **REST API (Port 8081):**
   - Berjalan di thread terpisah.
   - Endpoint `GET /api/records` digunakan untuk memindai folder dataset (`../dataset/`) dan mengembalikan daftar file EKG yang tersedia ke klien dalam format JSON. Mendukung CORS.
   - Kategori dataset yang didukung: `chapman`, `ptbxl_100hz`, `ptbxl_500hz`, `prosim_simulator`, dan `sensor_records`.

2. **WebSocket Server (Port 8080):**
   - Mentransmisikan data EKG secara berkesinambungan (continuous stream) ke klien Frontend.
   - Menggunakan format data murni milivolt (mV) dengan 3 channel (Ch1, Ch2, Ch3).
   - Melakukan chunking data (25 titik data per paket pada simulasi frekuensi sampling 250Hz) untuk meniru streaming alat medis real-time dengan sinkronisasi *delay* yang akurat.
   - Data akan di-loop terus menerus (looping simulasi) saat mencapai akhir file.

3. **Pembaca CSV yang Efisien:**
   - Mengekstrak file CSV statis menjadi *struct* internal `RawECGData`.
   - Menangani error parsing *float* jika ada data yang kosong/NaN (default ke `0.0`).

## Persyaratan (Requirements)

- **Rust & Cargo** (Edisi 2021)
- File CSV EKG yang valid berada di lokasi yang dikonfigurasi (misal: `../frame_000001_mv.csv` atau di dalam folder `../dataset/`). File CSV minimal harus memiliki 4 kolom berurutan: Waktu, Ch1, Ch2, Ch3.

## Cara Menjalankan

1. Pastikan Anda berada di root direktori proyek.
2. Jalankan aplikasi menggunakan `cargo`:

```bash
cargo run
```

Saat berhasil dijalankan, Anda akan melihat output di terminal yang mengindikasikan:
- Inisialisasi sistem medis.
- REST API Server HTTP berjalan di `http://127.0.0.1:8081/api/records`.
- Jumlah baris data yang berhasil dimuat dari CSV.
- WebSocket Server berjalan di `ws://127.0.0.1:8080`.

## Dependensi

- `tungstenite = "0.20"` - Untuk implementasi server WebSocket.
- `serde = { version = "1.0", features = ["derive"] }` - Untuk serialisasi & deserialisasi data (seperti ke JSON).
- `serde_json = "1.0"` - Digunakan bersama serde untuk parsing payload JSON.
- `csv = "1.3"` - Untuk membaca dan mem-parsing data CSV dengan cepat dan efisien.

*(Catatan: Library `rumqttc` disertakan pada `Cargo.toml` sebagai referensi jika akan mengaktifkan klien MQTT untuk integrasi perangkat hardware masa depan).*
