# ECG Rhythmia - Sinkronisasi & Integrasi Frontend

Dokumentasi ini berfokus pada integrasi sisi **Frontend (React)** untuk memvisualisasikan data Elektrokardiogram (EKG) secara *real-time*, serta bagaimana frontend melakukan sinkronisasi dengan backend.

## 💻 Integrasi Frontend (React)

Aplikasi frontend (React/TypeScript) bertanggung jawab untuk dua fungsi utama: memvisualisasikan *streaming* data EKG yang dikirim oleh backend dan memuat daftar dataset (records) yang tersedia.

### 1. Komunikasi WebSocket (Streaming Real-Time)
- **Koneksi:** Frontend terhubung ke server WebSocket backend pada alamat `ws://127.0.0.1:8080`.
- **Format Data:** Data diterima dalam format JSON. Struktur data (Payload) dari backend dirancang agar 100% sejajar dengan antarmuka TypeScript di sisi frontend (misal: `ecgTypes.ts`), khususnya pada objek `RawECGData` (berisi properti array `time`, `ch1`, `ch2`, `ch3`).
- **Render Visual:** Data yang diterima sudah dalam bentuk **murni milivolt (mV)** sehingga frontend tidak perlu lagi melakukan perhitungan kalibrasi multiplier/gain (*zero-overhead render*). Komponen grafik pada React cukup me-render nilai mentah ini secara langsung ke dalam bentuk gelombang EKG.

### 2. Pengambilan Data Dataset (REST API)
- **Koneksi HTTP:** Menggunakan pustaka *fetch* bawaan peramban atau Axios, frontend melakukan *request* HTTP `GET` ke REST API backend di `http://127.0.0.1:8081/api/records`.
- **Fungsi:** Berguna untuk memuat dan menampilkan daftar ketersediaan file CSV dataset (seperti dari folder Chapman, PTB-XL, atau data simulasi Prosim) pada menu navigasi (sidebar/dropdown) di aplikasi React.
- **CORS Terintegrasi:** REST API sisi server telah dikonfigurasi untuk mengizinkan *Cross-Origin Resource Sharing (CORS)* untuk semua origin (*Access-Control-Allow-Origin: \**), sehingga *request* langsung dari *dev server* frontend (misalnya `localhost:5173` atau `localhost:3000`) tidak akan diblokir oleh peramban (*browser*).

---

## 📂 Struktur Folder Frontend (`arrhythmia-detection-dashboard`)

Proyek antarmuka ini dibangun menggunakan **React**, **TypeScript**, **Vite**, dan **Tailwind CSS**. Arsitektur internal aplikasi (*Clean Architecture*) disusun agar kode lebih modular dan mudah dipelihara.

```text
c:\arrhythmia-detection-dashboard\
├── package.json               # Konfigurasi proyek, dependensi npm, dan script build/dev
├── vite.config.ts             # Konfigurasi Vite (bundler frontend)
├── tailwind.config.js         # Konfigurasi desain, warna, dan utilitas Tailwind CSS
├── postcss.config.js          # Pengaturan PostCSS untuk Tailwind
├── tsconfig.json              # Konfigurasi root TypeScript
├── index.html                 # Halaman utama aplikasi (Entry HTML)
├── public/                    # Aset statis yang tidak diproses oleh bundler
└── src/                       # Kode sumber (*source code*) utama React
    ├── main.tsx               # Titik awal masuk (Entry point) React (mounting ke index.html)
    ├── App.tsx                # Komponen root aplikasi
    ├── App.css / index.css    # Gaya global Tailwind
    ├── application/           # Lapisan Application (Use cases, custom hooks, state management)
    ├── core/                  # Lapisan Core (Tipe data, interface TypeScript, konfigurasi)
    ├── data/                  # Lapisan Data (Akses API eksternal, klien WebSocket)
    ├── presentation/          # Lapisan Presentation (Komponen UI React, layout, halaman)
    ├── workers/               # (Opsional) Web Workers untuk komputasi asinkron (multithreading UI)
    └── assets/                # Aset proyek lokal (gambar, ikon SVG, dsb)
```

## ⚙️ Cara Setup & Menjalankan Backend (Rust)

Backend aplikasi ini dibangun dengan **Rust**, menggunakan **WebSocket** dan REST API, serta **SQLite** sebagai basis data utamanya.

### Persyaratan (Prerequisites)
- **Rust & Cargo**: Instal Rust melalui [rustup.rs](https://rustup.rs/).
- **SQLite**: Database SQLite tersemat (embedded), biasanya tidak memerlukan instalasi server terpisah.

### Langkah-langkah Instalasi
1. **Navigasi ke Direktori**:
   ```bash
   cd c:\ecgrhythmia-backend
   ```
2. **Konfigurasi Environment** (Opsional):
   Jika diperlukan, atur port dan *path* di file `.env` yang berada di direktori root.
3. **Build & Run**:
   ```bash
   cargo run
   ```
   *Cargo akan secara otomatis mengunduh semua dependensi (crates), mengkompilasi aplikasi, dan menjalankannya.* 
   Secara default, REST API berjalan di port `8081` dan WebSocket berjalan di port `8080`.

---

## 🗄️ Pemakaian Database (SQLite)

Aplikasi ini menggunakan file `database.db` (SQLite) yang otomatis tersimpan di direktori utama backend.

- **Fungsi Utama**: Menyimpan data persisten yang mencakup **Profil Pasien**, **Riwayat Pemeriksaan (Records)**, dan **Data Medis** lainnya.
- **Inisialisasi Otomatis**: Saat backend pertama kali dijalankan, sistem akan otomatis membuat skema tabel (jika belum ada) dan dapat memuat data statis (*seeding*) agar API langsung siap diuji.
- **Manajemen Mandiri**: Karena seluruh data berpusat di satu file `database.db`, Anda dapat dengan mudah melakukan *backup*, menghapus data (*reset*), atau melihat tabel menggunakan *tools* GUI eksternal seperti **DB Browser for SQLite**.

---

## 🌐 Sinkronisasi dengan PWA (Frontend)

Backend didesain agar dapat tersinkronisasi mulus dengan aplikasi React (yang telah dikonfigurasi sebagai *Progressive Web App* / PWA).

1. **Sinkronisasi Data Profil & Riwayat (REST API)**: 
   Setiap kali pengguna melakukan pembaruan profil atau pengaturan perangkat di PWA, frontend mengirimkan *request* HTTP (seperti `POST` atau `PUT`) ke `http://127.0.0.1:8081/api/...`. Backend SQLite akan langsung menyimpan perubahan ini secara permanen.
   
2. **Komunikasi Real-Time (WebSocket)**:
   PWA mengandalkan koneksi persisten ke `ws://127.0.0.1:8080` untuk menerima aliran (*streaming*) grafik detak jantung EKG tanpa *overhead* (hambatan) koneksi ulang HTTP biasa.
   
3. **Mekanisme Fallback (Mode Offline PWA)**:
   Jika backend terputus atau dimatikan, antarmuka PWA dilengkapi dengan *Local Storage Fallback*. PWA tetap dapat dioperasikan secara fungsional (untuk berpindah halaman, melihat riwayat *cache*, atau menyimpan profil tiruan) berkat fitur *Service Worker* dan penyimpanan lokal, menjamin UX (Pengalaman Pengguna) yang tidak terputus.

---

## ⚙️ Cara Setup & Menjalankan Frontend

### Persyaratan (Prerequisites)
- **Node.js** (Rekomendasi versi LTS 18.x atau ke atas).
- Manajer paket seperti **npm** (biasanya terpasang otomatis bersama Node.js).

### Langkah-langkah Menjalankan

1. **Buka Terminal** baru, lalu arahkan navigasi ke direktori proyek frontend:
   ```bash
   cd c:\arrhythmia-detection-dashboard
   ```
2. **Instalasi Dependensi**. Jalankan perintah ini (hanya perlu dilakukan pertama kali atau jika ada penambahan pustaka baru):
   ```bash
   npm install
   ```
3. **Jalankan *Development Server***:
   ```bash
   npm run dev
   ```
4. **Buka Aplikasi di Browser**. 
   Secara *default*, Vite akan menjalankan aplikasi di `http://localhost:5173` (perhatikan log di terminal Anda untuk tautan spesifik). Buka tautan tersebut menggunakan peramban web favorit Anda.

### Perintah Tambahan (NPM Scripts)
- `npm run build`: Melakukan proses kompilasi TypeScript dan mem-*build* aplikasi agar siap di-*deploy* ke tahap produksi (berada di folder `dist/`).
- `npm run lint`: Memeriksa potensi kesalahan/standar kode dengan cepat (memanfaatkan `oxlint`).
- `npm run preview`: Membuka server lokal (*preview*) untuk melihat dan menguji *build* versi produksi yang telah dikompilasi sebelumnya.

---

## 🔄 Mekanisme Streaming WebSocket (Backend Internal)

Dalam sistem ini, backend (Rust) memegang kendali penuh atas mekanisme pengaturan ritme pengiriman aliran data EKG (dari file CSV ke WebSocket) agar persis menyerupai alat fisik medis *real-time*.

1. **Chunking Data (Pemaketan):** 
   Alih-alih mengirim titik koordinat satu per satu yang akan membuat jaringan kewalahan (karena *overhead* WebSocket), backend memotong (chunk) aliran data dalam bentuk *batch*.
   
2. **Frekuensi Sampling (250Hz):** 
   Sistem diatur pada asumsi frekuensi *sampling rate* dasar 250Hz. Backend mengelompokkan secara spesifik **25 sampel data** menjadi satu *chunk* paket transmisi.
   
3. **Delay Real-Time Presisi:** 
   Dalam *sampling rate* 250Hz, 25 sampel merepresentasikan durasi waktu tepat **100 milidetik (ms)**. Oleh karena itu, *thread* pengiriman backend akan menerapkan sinkronisasi jeda waktu otomatis (*sleep_duration*) selama 100ms setiap kali selesai mengirimkan satu *chunk* paket ke frontend.
   
4. **Aliran Tanpa Henti (Seamless Looping):** 
   Skema ini menjamin kelancaran *streaming real-time* yang sangat konsisten, setara dengan kecepatan sapuan standar perekaman di atas kertas termal EKG (25 mm/s). Ketika pointer pembacaan backend telah mencapai titik data terakhir pada file rekaman CSV, sistem akan otomatis mereset siklus dari titik nol (*looping*), mensimulasikan aliran detak jantung pasien yang terus menyala.
