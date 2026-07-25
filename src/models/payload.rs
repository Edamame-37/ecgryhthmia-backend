/**
 * @fileoverview Modul Models: Bentuk Data (Payload)
 * Merupakan cetak biru dari JSON yang akan dikirim via WebSocket ke Frontend.
 * Struktur ini 100% sejajar (sinkron) dengan `ecgTypes.ts` di React.
 */

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawECGData {
    pub time: Vec<f64>,
    pub ch1: Vec<f64>, // Lead I (Murni milivolt)
    pub ch2: Vec<f64>, // Lead II (Murni milivolt)
    pub ch3: Vec<f64>, // Lead III / aVR / Kalibrasi
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ECGDataPayload {
    pub raw: RawECGData,
    pub classification_result: String,
    pub confidence: String,
    pub anomaly_indices: Vec<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerMessage {
    // Menggunakan r#type karena "type" adalah kata kunci bawaan (reserved keyword) di Rust
    pub r#type: String, 

    // Atribut 'skip_serializing_if' akan membuang field ini dari JSON jika nilainya None.
    // Ini menghemat ukuran teks (bandwidth) WebSocket secara drastis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measurement_id: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256_checksum: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_payload: Option<ECGDataPayload>,
    
    // Menggunakan serde_json::Value untuk mendukung data dinamis / array campuran
    // (setara dengan tipe data `any[]` pada TypeScript)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Vec<Value>>, 
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}