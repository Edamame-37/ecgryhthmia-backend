use super::error::{ApiError, ApiResult};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
#[derive(Clone, Default)]
pub struct RateLimits(Arc<Mutex<HashMap<String, (Instant, u32)>>>);
impl RateLimits {
    pub async fn check(&self, key: &str, limit: u32) -> ApiResult<()> {
        let mut entries = self.0.lock().await;
        entries.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
        if entries.len() >= 4096 && !entries.contains_key(key) {
            return Err(ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Rate limit exceeded".into(),
            ));
        }
        let (at, count) = entries
            .entry(key.to_string())
            .or_insert((Instant::now(), 0));
        if at.elapsed() >= Duration::from_secs(60) {
            *at = Instant::now();
            *count = 0
        }
        if *count >= limit {
            return Err(ApiError(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "Rate limit exceeded".into(),
            ));
        }
        *count += 1;
        Ok(())
    }
}
