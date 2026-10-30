use super::websocket::StreamHub;
use crate::{
    api::error::{ApiError, ApiResult},
    models::payload::{encode_packet, RecordedFrame},
};
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{mpsc, Mutex, OwnedSemaphorePermit, Semaphore};
type Delivery = (RecordedFrame, OwnedSemaphorePermit);
#[derive(Clone)]
pub struct Pacer {
    queues: Arc<Mutex<HashMap<String, mpsc::Sender<Delivery>>>>,
    capacity: Arc<Semaphore>,
    hub: StreamHub,
}
impl Pacer {
    pub fn new(hub: StreamHub) -> Self {
        Self {
            queues: Default::default(),
            capacity: Arc::new(Semaphore::new(32)),
            hub,
        }
    }
    pub async fn enqueue(&self, frame: RecordedFrame) -> ApiResult<()> {
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| {
            ApiError::conflict("Streaming capacity reached; frame retained in history")
        })?;
        let mut queues = self.queues.lock().await;
        queues.retain(|_, sender| !sender.is_closed());
        let sender = if let Some(sender) = queues.get(&frame.device_id) {
            sender.clone()
        } else {
            if queues.len() >= 256 {
                return Err(ApiError::conflict("Streaming capacity reached"));
            }
            let (sender, mut receiver) = mpsc::channel::<Delivery>(4);
            let hub = self.hub.clone();
            tokio::spawn(async move {
                while let Ok(Some((frame, _permit))) =
                    tokio::time::timeout(Duration::from_secs(60), receiver.recv()).await
                {
                    let rate = frame.payload.sampling_rate_hz;
                    let chunk_size = (rate * 0.1).round().max(1.0) as usize;
                    let origin = tokio::time::Instant::now();
                    for (offset, chunk) in frame.payload.ecg.samples.chunks(chunk_size).enumerate()
                    {
                        let sample_offset = offset * chunk_size;
                        let raw = json!({
                         "time":(0..chunk.len()).map(|i|frame.start_time+(sample_offset+i) as f64/rate).collect::<Vec<_>>(),
                         "ch1":chunk.iter().map(|v|v[0]).collect::<Vec<_>>(),
                         "ch2":chunk.iter().map(|v|v[1]).collect::<Vec<_>>(),
                         "ch3":chunk.iter().map(|v|v[2]).collect::<Vec<_>>()
                        });
                        let prediction = &frame.payload.prediction;
                        let packet = json!({"type":"live_data","measurement_id":frame.id,"device_id":frame.device_id,"patient_id":frame.patient_id,
       "session_id":frame.session_id,"timestamp":frame.payload.created_at,"frame_number":frame.frame_number,
       "sampling_rate_hz":rate,"duration_s":frame.payload.duration_s,"sample_count":frame.payload.ecg.samples.len(),
       "sample_offset":sample_offset,"start_time":frame.start_time,
       "data_payload":{"raw":raw,"classification_result":prediction.as_ref().map(|p|p.label.as_str()).unwrap_or("UNKNOWN"),
        "prediction_details":prediction,"validation":frame.payload.validation,"system":frame.payload.system,
        "network":frame.payload.network,"stress_test":frame.payload.stress_test,"anomaly_indices":[]}});
                        if let Ok(encoded) = encode_packet(&packet) {
                            hub.publish(&frame.session_id, encoded).await;
                        }
                        tokio::time::sleep_until(
                            origin
                                + Duration::from_secs_f64(
                                    (sample_offset + chunk.len()) as f64 / rate,
                                ),
                        )
                        .await;
                    }
                }
            });
            queues.insert(frame.device_id.clone(), sender.clone());
            sender
        };
        drop(queues);
        // A slow device cannot grow an unbounded queue or stall unrelated devices.
        sender
            .try_send((frame, permit))
            .map_err(|_| ApiError::conflict("Streaming backlog; frame persisted for history"))
    }
}
