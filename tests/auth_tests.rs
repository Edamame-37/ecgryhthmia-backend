mod support;
use aws_lc_rs::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING},
};
use axum::{routing::get, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use support::*;

#[tokio::test]
async fn dynamic_es256_valid_expired_tampered_and_invalid_claims() {
    let random = SystemRandom::new();
    let der = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &random).unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, der.as_ref()).unwrap();
    let point = pair.public_key().as_ref();
    let set = json!({"keys":[{"kty":"EC","crv":"P-256","alg":"ES256","use":"sig","kid":"test-key","x":URL_SAFE_NO_PAD.encode(&point[1..33]),"y":URL_SAFE_NO_PAD.encode(&point[33..65])}]});
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let app = Router::new().route(
        "/auth/v1/.well-known/jwks.json",
        get(move || {
            let set = set.clone();
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Json(set)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let directory = tempfile::tempdir().unwrap();
    let mut config = test_config(String::new(), directory.path());
    config.supabase_url = format!("http://{address}");
    let verifier = ecg_backend::api::auth::JwtVerifier::new(Arc::new(config));
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some("test-key".into());
    let key = jsonwebtoken::EncodingKey::from_ec_der(der.as_ref());
    let mut claims = claims("patient");
    claims.iss = format!("http://{address}/auth/v1");
    let valid = jsonwebtoken::encode(&header, &claims, &key).unwrap();
    assert_eq!(verifier.verify(&valid).await.unwrap().sub, "auth-patient");
    assert!(verifier.verify(&valid).await.is_ok());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    for field in ["expired", "issuer", "audience", "future", "old", "empty"] {
        let mut invalid = claims.clone();
        match field {
            "expired" => {
                invalid.iat -= 120;
                invalid.exp = invalid.iat + 30;
            }
            "issuer" => invalid.iss = "https://untrusted.invalid".into(),
            "audience" => invalid.aud = "anon".into(),
            "future" => invalid.iat += 120,
            "old" => invalid.iat -= 3601,
            "empty" => invalid.sub.clear(),
            _ => unreachable!(),
        }
        assert!(
            verifier
                .verify(&jsonwebtoken::encode(&header, &invalid, &key).unwrap())
                .await
                .is_err(),
            "{field}"
        );
    }
    let mut parts: Vec<_> = valid.split('.').map(str::to_string).collect();
    let mut payload: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&parts[1]).unwrap()).unwrap();
    payload["sub"] = json!("forged");
    parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    assert!(verifier.verify(&parts.join(".")).await.is_err());
    header.kid = Some("removed-key".into());
    assert!(verifier
        .verify(&jsonwebtoken::encode(&header, &claims, &key).unwrap())
        .await
        .is_err());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "unknown keys cannot amplify JWKS fetches"
    );
    server.abort();
}
