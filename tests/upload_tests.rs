mod support;
use axum::{body::Body, http::Request};
use serde_json::json;
use support::*;
use tower::ServiceExt;
async fn upload(context: &TestContext, files: Vec<(&str, String)>) -> axum::response::Response {
    let boundary = "ecg-synthetic-boundary";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"patient_id\"\r\n\r\npat_A\r\n"
    );
    for (filename, contents) in files {
        body.push_str(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n{contents}\r\n"));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    let request = Request::builder()
        .method("POST")
        .uri("/api/sessions/upload")
        .header("Authorization", format!("Bearer {}", token("patient")))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    context.app.clone().oneshot(request).await.unwrap()
}
#[tokio::test]
async fn multipart_import_orders_frames_and_uses_sample_duration_and_unknown_prediction() {
    let context = TestContext::new().await;
    let now = chrono::Utc::now().to_rfc3339();
    let metadata = |id: &str| {
        json!({"message_id":id,"created_at":now,"sampling_rate_hz":125.0,"duration_s":0.2})
            .to_string()
    };
    let csv = std::iter::once("Time,Lead III,Lead I,Lead II".to_owned())
        .chain((0..25).map(|index| format!("{},0.1,0.5,0.6", index as f64 / 125.)))
        .collect::<Vec<_>>()
        .join("\n");
    let response = upload(
        &context,
        vec![
            ("frame10.json", metadata("source10")),
            ("frame10.csv", csv.clone()),
            ("frame2.csv", csv),
            ("frame2.json", metadata("source2")),
        ],
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let id = json_response(response).await["session_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let frames = json_response(
        context
            .request(
                "GET",
                &format!("/api/records/{id}"),
                Some("patient"),
                json!({}),
            )
            .await,
    )
    .await;
    assert_eq!(frames.as_array().unwrap().len(), 2);
    assert_eq!(frames[0]["source_message_id"], "source2");
    assert_eq!(frames[1]["source_message_id"], "source10");
    assert_eq!(frames[0]["start_time"], 0.);
    assert_eq!(frames[0]["end_time"], 0.2);
    assert_eq!(frames[1]["start_time"], 0.2);
    assert_eq!(frames[1]["end_time"], 0.4);
    assert_eq!(frames[0]["label"], "UNKNOWN");
    assert_eq!(frames[0]["ecg"]["samples"][0], json!([0.5, 0.6, 0.1]));
    context.cleanup().await;
}
#[tokio::test]
async fn malformed_upload_does_not_create_a_partial_success_session() {
    let context = TestContext::new().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(
        upload(
            &context,
            vec![("frame1.csv", "Lead I,Lead III\n1,3".into())]
        )
        .await
        .status()
        .as_u16(),
        400
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&context.state.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    context.cleanup().await;
}
