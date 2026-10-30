use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct DeviceValidation {
    pub status: String,
    pub warnings: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceEcg {
    pub format: String,
    pub samples: Vec<Vec<f64>>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DevicePrediction {
    pub status: String,
    pub label: String,
    pub confidence_percent: f64,
    pub probabilities: Option<serde_json::Value>,
    pub threshold: Option<f64>,
    pub latency_ms: Option<f64>,
    pub runtime: Option<String>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceSystem {
    pub cpu_usage_percent: Option<f64>,
    pub memory_usage_percent: Option<f64>,
    pub memory_usage_mb: Option<i64>,
    pub cpu_temperature_c: Option<f64>,
    pub uptime_s: Option<i64>,
    pub battery_percent: Option<f64>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceStressTest {
    pub enabled: Option<bool>,
    pub frame_counter: Option<i64>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeviceNetwork {
    pub mqtt_publish_latency_ms: Option<f64>,
    pub wifi_rssi_dbm: Option<i64>,
    pub mqtt_connected: Option<bool>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DevicePayload {
    pub message_id: String,
    pub device_id: String,
    pub session_id: String,
    pub frame_id: String,
    pub created_at: String,
    pub sampling_rate_hz: f64,
    pub duration_s: f64,
    #[serde(default)]
    pub validation: DeviceValidation,
    pub ecg: DeviceEcg,
    #[serde(default)]
    pub prediction: Option<DevicePrediction>,
    pub system: Option<DeviceSystem>,
    pub stress_test: Option<DeviceStressTest>,
    pub network: Option<DeviceNetwork>,
}
impl DevicePayload {
    pub fn validate(&self) -> Result<(), String> {
        if !self.sampling_rate_hz.is_finite() || !(100.0..=1000.0).contains(&self.sampling_rate_hz)
        {
            return Err("Invalid sampling rate".into());
        }
        if self.ecg.format != "samples_by_time"
            || self.ecg.samples.is_empty()
            || self.ecg.samples.len() > 30000
        {
            return Err("Invalid ECG frame shape".into());
        }
        if self.ecg.samples.iter().any(|sample| {
            sample.len() != 3
                || sample
                    .iter()
                    .any(|value| !value.is_finite() || value.abs() > 100.0)
        }) {
            return Err("ECG samples require three finite mV channels".into());
        }
        let duration = self.ecg.samples.len() as f64 / self.sampling_rate_hz;
        if !self.duration_s.is_finite()
            || (self.duration_s - duration).abs() > 0.5 / self.sampling_rate_hz
            || duration > 30.0
        {
            return Err("Sample count does not match duration".into());
        }
        for id in [
            &self.device_id,
            &self.session_id,
            &self.message_id,
            &self.frame_id,
        ] {
            if id.is_empty()
                || id.len() > 128
                || !id.bytes().all(|character| {
                    character.is_ascii_alphanumeric() || character == b'_' || character == b'-'
                })
            {
                return Err("Invalid frame identity".into());
            }
        }
        if chrono::DateTime::parse_from_rfc3339(&self.created_at).is_err() {
            return Err("Invalid frame timestamp".into());
        }
        if self.validation.status.len() > 100
            || self.validation.warnings.len() > 16
            || self
                .validation
                .warnings
                .iter()
                .any(|value| value.len() > 200)
        {
            return Err("Invalid validation metadata".into());
        }
        if let Some(prediction) = &self.prediction {
            if prediction
                .runtime
                .as_ref()
                .is_some_and(|value| value.len() > 100)
                || prediction.label.is_empty()
                || prediction.label.len() > 100
                || !prediction.confidence_percent.is_finite()
                || !(0.0..=100.0).contains(&prediction.confidence_percent)
            {
                return Err("Invalid prediction".into());
            }
            if let Some(probabilities) = &prediction.probabilities {
                if !probabilities.as_object().is_some_and(|values| {
                    !values.is_empty()
                        && values.len() <= 16
                        && values.keys().all(|key| key.len() <= 100)
                        && values.values().all(|value| {
                            value.as_f64().is_some_and(|number| {
                                number.is_finite() && (0.0..=100.0).contains(&number)
                            })
                        })
                }) {
                    return Err("Invalid prediction probabilities".into());
                }
            }
        }
        if let Some(system) = &self.system {
            for value in [
                system.cpu_usage_percent,
                system.memory_usage_percent,
                system.battery_percent,
            ]
            .into_iter()
            .flatten()
            {
                if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                    return Err("Invalid device telemetry".into());
                }
            }
        }
        Ok(())
    }
}
