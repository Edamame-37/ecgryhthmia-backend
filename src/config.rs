use dotenvy::dotenv;
use std::env;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub host_ip: String,
    pub rest_port: String,
    pub ws_port: String,
    pub mqtt_broker: String,
    pub mqtt_port: u16,
    pub mqtt_topic: String,
    pub mqtt_username: String,
    pub mqtt_password: String,
    pub jwt_secret: String,
    pub sqlite_key: String,
    pub db_path: String,
    pub default_admin_email: String,
    pub default_admin_password: String,
}

impl AppConfig {
    pub fn load() -> Self {
        dotenv().ok();

                let host_ip = env::var("HOST_IP")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let rest_port = env::var("REST_PORT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "8081".to_string());
        let ws_port = env::var("WS_PORT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "8080".to_string());
        
        let mqtt_broker = env::var("MQTT_BROKER")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: MQTT_BROKER belum diset di .env!");
        
        let mqtt_port = env::var("MQTT_PORT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "8883".to_string())
            .parse::<u16>()
            .unwrap_or(8883);
            
                let mqtt_topic = env::var("MQTT_TOPIC")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: MQTT_TOPIC belum diset di .env!");
            
        let mqtt_username = env::var("MQTT_USERNAME")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: MQTT_USERNAME belum diset di .env!");
            
        let mqtt_password = env::var("MQTT_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: MQTT_PASSWORD belum diset di .env!");
        
        let jwt_secret = env::var("JWT_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: JWT_SECRET belum diset di .env!");
        
        let sqlite_key = env::var("SQLITE_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .expect("[Config] ERROR: SQLITE_KEY belum diset di .env!");

                let db_path = env::var("DB_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "database.db".to_string());

        let default_admin_email = env::var("DEFAULT_ADMIN_EMAIL")
            .expect("[Config] ERROR: DEFAULT_ADMIN_EMAIL belum diset di .env!");
            
        let default_admin_password = env::var("DEFAULT_ADMIN_PASSWORD")
            .expect("[Config] ERROR: DEFAULT_ADMIN_PASSWORD belum diset di .env!");

        AppConfig {
            host_ip,
            rest_port,
            ws_port,
            mqtt_broker,
            mqtt_port,
            mqtt_topic,
            mqtt_username,
            mqtt_password,
            jwt_secret,
            sqlite_key,
            db_path,
            default_admin_email,
            default_admin_password,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load() {
        std::env::set_var("HOST_IP", "127.0.0.9");
        std::env::set_var("REST_PORT", "9999");
        std::env::set_var("WS_PORT", "9998");
        std::env::set_var("MQTT_BROKER", "broker.test");
        std::env::set_var("MQTT_PORT", "1883");
        std::env::set_var("MQTT_TOPIC", "test/topic");
        std::env::set_var("MQTT_USERNAME", "testuser");
        std::env::set_var("MQTT_PASSWORD", "testpass");
        std::env::set_var("JWT_SECRET", "testsecret");
        std::env::set_var("SQLITE_KEY", "testkey");
        std::env::set_var("DB_PATH", "testdb.db");
        std::env::set_var("DEFAULT_ADMIN_EMAIL", "admin@test.com");
        std::env::set_var("DEFAULT_ADMIN_PASSWORD", "admin123");

        let config = AppConfig::load();
        assert_eq!(config.host_ip, "127.0.0.9");
        assert_eq!(config.rest_port, "9999");
        assert_eq!(config.ws_port, "9998");
        assert_eq!(config.mqtt_broker, "broker.test");
        assert_eq!(config.mqtt_port, 1883);
        assert_eq!(config.mqtt_topic, "test/topic");
        assert_eq!(config.mqtt_username, "testuser");
        assert_eq!(config.mqtt_password, "testpass");
        assert_eq!(config.jwt_secret, "testsecret");
        assert_eq!(config.sqlite_key, "testkey");
        assert_eq!(config.db_path, "testdb.db");
        assert_eq!(config.default_admin_email, "admin@test.com");
        assert_eq!(config.default_admin_password, "admin123");
    }
}
