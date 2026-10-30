mod support;
use ecg_backend::services::{
    ingestion::persist,
    recording::{acknowledge, Command, DeviceAck},
};
use support::*;
#[tokio::test]
async fn wrong_listener_stale_out_of_order_and_stopped_frames_are_rejected() {
    let context = TestContext::new().await;
    assert!(persist(&context.state, "dev_B", payload(1), true)
        .await
        .is_err());
    assert!(persist(&context.state, "dev_A", payload(2), true)
        .await
        .is_err());
    let mut stale = payload(1);
    stale.created_at = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
    assert!(persist(&context.state, "dev_A", stale, true).await.is_err());
    let mut spoof = payload(1);
    spoof.device_id = "device_pat_B".into();
    assert!(persist(&context.state, "dev_A", spoof, true).await.is_err());
    persist(&context.state, "dev_A", payload(1), true)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET state='stopping' WHERE id='ses_pat_A'")
        .execute(&context.state.pool)
        .await
        .unwrap();
    assert!(acknowledge(
        &context.state,
        "dev_A",
        DeviceAck {
            session_id: "ses_pat_A".into(),
            command: Command::Stop,
            last_frame_number: Some(2)
        }
    )
    .await
    .is_err());
    acknowledge(
        &context.state,
        "dev_A",
        DeviceAck {
            session_id: "ses_pat_A".into(),
            command: Command::Stop,
            last_frame_number: Some(1),
        },
    )
    .await
    .unwrap();
    assert!(persist(&context.state, "dev_A", payload(2), true)
        .await
        .is_err());
    context.cleanup().await;
}
#[test]
fn csv_channel_mapping_is_exact_and_bad_values_are_rejected() {
    use ecg_backend::data::csv_reader::parse_csv_samples;
    let samples =
        parse_csv_samples("Time,Lead III,Lead I,Lead II\n0,3,1,2\n0.004,6,4,5".as_bytes()).unwrap();
    assert_eq!(samples, vec![vec![1., 2., 3.], vec![4., 5., 6.]]);
    for text in [
        "I,II,III\n1,NaN,3",
        "I,I,III\n1,2,3",
        "I,III\n1,2",
        "Time,I,II,III\n0,1,2,3\n0,1,2,3",
        "Time,I,II,III\nNaN,1,2,3",
    ] {
        assert!(parse_csv_samples(text.as_bytes()).is_err(), "{text}");
    }
}
#[tokio::test]
async fn session_channels_do_not_cross_and_dropped_subscribers_are_reclaimed() {
    let hub = ecg_backend::network::websocket::StreamHub::default();
    let mut a = hub.subscribe("A").await.unwrap();
    let mut b = hub.subscribe("B").await.unwrap();
    hub.publish("A", "only-a".into()).await;
    assert_eq!(a.recv().await.unwrap(), "only-a");
    assert!(b.try_recv().is_err());
    for index in 0..1200 {
        drop(hub.subscribe(&format!("temporary-{index}")).await.unwrap());
    }
}
