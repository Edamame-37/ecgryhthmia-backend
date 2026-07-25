/**
 * @fileoverview Modul Network: MQTT Listener (Rust)
 * Bertugas berlangganan (subscribe) data EKG dari MQTT Broker (Mosquitto)
 * dan meneruskannya ke handler WebSocket.
 */
/* 
use rumqttc::{Client, MqttOptions, QoS, Event, Packet};
use std::thread;
use std::time::Duration;

pub fn start_mqtt_listener<F>(broker_host: &str, broker_port: u16, topic: &str, on_message: F)
where
    F: Fn(String) + Send + 'static,
{
    let host = broker_host.to_string();
    let topic_name = topic.to_string();

    // Spawn thread khusus agar listener MQTT tidak mengganggu server WebSocket
    thread::spawn(move || {
        let mut mqttoptions = MqttOptions::new("rust_ecg_bridge", &host, broker_port);
        mqttoptions.set_keep_alive(Duration::from_secs(5));

        let (client, mut connection) = Client::new(mqttoptions, 10);
        
        // Subscribe ke topik data EKG dari ESP32
        if let Err(e) = client.subscribe(&topic_name, QoS::AtMostOnce) {
            eprintln!("[MQTT] Gagal subscribe ke topik '{}': {}", topic_name, e);
            return;
        }

        println!("[MQTT] Terhubung ke Broker {}:{} | Topik: {}", host, broker_port, topic_name);

        // Loop mendengarkan pesan yang masuk
        for notification in connection.iter() {
            match notification {
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    if let Ok(payload_str) = String::from_utf8(publish.payload.to_vec()) {
                        // Teruskan pesan JSON murni ke callback WebSocket
                        on_message(payload_str);
                    }
                }
                Err(e) => {
                    eprintln!("[MQTT] Koneksi terputus/error: {}. Mencoba menghubungkan ulang...", e);
                    thread::sleep(Duration::from_secs(1));
                }
                _ => {}
            }
        }
    });
}
*/