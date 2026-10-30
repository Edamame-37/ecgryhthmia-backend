use super::error::{ApiError, ApiResult};
use jsonwebtoken::{jwk::JwkSet, DecodingKey};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
type CachedKeys = Option<(Instant, JwkSet)>;
#[derive(Clone)]
pub struct JwksCache {
    client: reqwest::Client,
    url: String,
    keys: Arc<Mutex<CachedKeys>>,
}
impl JwksCache {
    pub fn new(url: String) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .expect("HTTP client"),
            url,
            keys: Default::default(),
        }
    }
    pub async fn key(&self, kid: &str) -> ApiResult<DecodingKey> {
        let mut cache = self.keys.lock().await;
        if cache
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(60))
        {
            let response = self
                .client
                .get(&self.url)
                .send()
                .await
                .map_err(|_| ApiError::unauthorized())?
                .error_for_status()
                .map_err(|_| ApiError::unauthorized())?;
            if response
                .content_length()
                .is_some_and(|size| size > 256 * 1024)
            {
                return Err(ApiError::unauthorized());
            }
            let bytes = response
                .bytes()
                .await
                .map_err(|_| ApiError::unauthorized())?;
            if bytes.len() > 256 * 1024 {
                return Err(ApiError::unauthorized());
            }
            let set: JwkSet =
                serde_json::from_slice(&bytes).map_err(|_| ApiError::unauthorized())?;
            *cache = Some((Instant::now(), set));
        }
        let key = cache
            .as_ref()
            .and_then(|(_, set)| set.find(kid))
            .ok_or_else(ApiError::unauthorized)?;
        DecodingKey::from_jwk(key).map_err(|_| ApiError::unauthorized())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};
    use serde_json::json;
    #[tokio::test]
    async fn removed_signing_key_is_not_served_after_ttl() {
        let first = json!({"keys":[{"kty":"oct","kid":"old","k":"bG9jYWwtdGVzdC1rZXk"}]});
        let current = Arc::new(Mutex::new(first));
        let served = current.clone();
        let app = Router::new().route(
            "/keys",
            get(move || {
                let served = served.clone();
                async move { Json(served.lock().await.clone()) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/keys", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cache = JwksCache::new(url);
        assert!(cache.key("old").await.is_ok());
        *current.lock().await = json!({"keys":[]});
        assert!(cache.key("old").await.is_ok());
        cache.keys.lock().await.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(61);
        assert!(cache.key("old").await.is_err());
        server.abort();
    }
}
