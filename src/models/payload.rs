use crate::models::device::DevicePayload;
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecordedFrame {
    pub id: String,
    pub device_id: String,
    pub patient_id: String,
    pub session_id: String,
    pub frame_number: i64,
    pub start_time: f64,
    pub end_time: f64,
    pub payload: DevicePayload,
}
#[derive(Serialize, Deserialize)]
pub struct IntegrityEnvelope {
    pub r#type: String,
    pub content: String,
    pub sha256_checksum: String,
}
pub fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
pub fn encode_packet(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    let content = serde_json::to_string(value)?;
    serde_json::to_string(&IntegrityEnvelope {
        r#type: "ecg_packet".into(),
        sha256_checksum: digest(content.as_bytes()),
        content,
    })
}
