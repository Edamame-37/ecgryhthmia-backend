use crate::{
    api::{
        error::{ApiError, ApiResult},
        routes::AppState,
    },
    services::ingestion::Inbound,
};
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS, TlsConfiguration, Transport};
use sqlx::Row;
#[derive(Clone)]
pub struct DeviceConnection {
    pub client: AsyncClient,
    pub revision: i64,
}
pub async fn connect_device(
    state: AppState,
    id: &str,
    sender: tokio::sync::mpsc::Sender<Inbound>,
) -> ApiResult<()> {
    let row=sqlx::query("SELECT d.id,d.name,d.config_revision,c.mqtt_broker,c.mqtt_port,c.mqtt_topic,c.mqtt_username,c.mqtt_password FROM devices d JOIN device_credentials c ON c.device_id=d.id WHERE d.id=$1").bind(id).fetch_one(&state.pool).await?;
    let host: String = row.get("mqtt_broker");
    let port: i32 = row.get("mqtt_port");
    let topic: String = row.get("mqtt_topic");
    validate_broker(&state, &host, port, &topic)?;
    let device_id = id.to_string();
    let mut options = MqttOptions::new(
        format!("ecg-{}", uuid::Uuid::new_v4().simple()),
        host,
        port as u16,
    );
    options.set_keep_alive(std::time::Duration::from_secs(30));
    options.set_max_packet_size(2 * 1024 * 1024, 2 * 1024 * 1024);
    options.set_credentials(
        row.get::<String, _>("mqtt_username"),
        row.get::<String, _>("mqtt_password"),
    );
    options.set_transport(Transport::Tls(TlsConfiguration::Native));
    let (client, mut events) = AsyncClient::new(options, 16);
    let mut tasks = state.mqtt_tasks.lock().await;
    if let Some(previous) = tasks.remove(id) {
        previous.abort();
        let _ = previous.await;
    }
    state.mqtt_clients.write().await.insert(
        device_id.clone(),
        DeviceConnection {
            client: client.clone(),
            revision: row.get("config_revision"),
        },
    );
    let task = tokio::spawn(async move {
        loop {
            match events.poll().await {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    if client.subscribe(&topic, QoS::AtLeastOnce).await.is_err() {
                        break;
                    }
                    if client
                        .subscribe(format!("{topic}/ack"), QoS::AtLeastOnce)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Event::Incoming(Packet::Publish(packet))) => {
                    let message = if packet.topic == topic {
                        serde_json::from_slice(&packet.payload)
                            .ok()
                            .map(|payload| Inbound::Frame {
                                device_id: device_id.clone(),
                                payload: Box::new(payload),
                            })
                    } else if packet.topic == format!("{topic}/ack") {
                        serde_json::from_slice(&packet.payload)
                            .ok()
                            .map(|ack| Inbound::Ack {
                                device_id: device_id.clone(),
                                ack,
                            })
                    } else {
                        None
                    };
                    if let Some(message) = message {
                        if sender.send(message).await.is_err() {
                            break;
                        }
                    } else {
                        tracing::warn!(device_id, "Invalid MQTT message rejected")
                    }
                }
                Err(error) => {
                    tracing::warn!(%error,device_id,"MQTT disconnected");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                _ => {}
            }
        }
    });
    tasks.insert(id.to_owned(), task);
    Ok(())
}
pub fn validate_broker(_state: &AppState, host: &str, port: i32, topic: &str) -> ApiResult<()> {
    let allowed = std::env::var("MQTT_ALLOWED_BROKERS").unwrap_or_default();
    if port != 8883
        || !allowed
            .split(',')
            .any(|item| !item.is_empty() && item.trim() == host)
        || host.contains('/')
        || topic.is_empty()
        || topic.len() > 200
        || topic.contains(['#', '+'])
    {
        return Err(ApiError::bad(
            "MQTT broker must be allowlisted and use TLS port 8883; wildcard topics are prohibited",
        ));
    }
    Ok(())
}
