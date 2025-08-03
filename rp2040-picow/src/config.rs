//! Configuration provider

use crate::debug_println;
use embassy_time::Duration;

/// Application config
#[derive(Debug, Clone, Copy)]
#[allow(non_snake_case)]
pub struct AppConfig {
    /// WIFI SSID
    pub WIFI_SSID: &'static str,
    /// WIFI password
    pub WIFI_PASS: &'static str,
    /// MQTT server address and port
    pub MQTT_ADDR: &'static str,
    /// MQTT username
    pub MQTT_USER: &'static str,
    /// MQTT password
    pub MQTT_PASS: &'static str,
    /// MQTT topic prefix
    pub MQTT_PRFX: &'static str,
    /// The sleep duration between to measurement cycles
    pub SENSOR_SLEEP_SECS: Duration,
    /// The alert blinking duration if an error occurs
    pub SENSOR_ALERT_SECS: Duration,
}
impl AppConfig {
    /// Loads the config from the flash memory
    pub fn load() -> Self {
        /// Default duration
        const DEFAULT_DURATION: Duration = Duration::from_secs(30);

        /// Userdata section in flash
        #[unsafe(link_section = ".userdata")]
        static USERDATA: [u8; 4096] = [0; 4096];

        // Read config
        let mut wifi_ssid = None;
        let mut wifi_pass = None;
        let mut mqtt_addr = None;
        let mut mqtt_user = None;
        let mut mqtt_pass = None;
        let mut mqtt_prfx = None;
        let mut sensor_sleep_secs = None;
        let mut sensor_alert_secs = None;
        'read_lines: for line in USERDATA.split(|byte| *byte == b'\n') {
            // Parse line as INI line
            let Ok(line) = str::from_utf8(line) else {
                // We are not in the INI section anymore
                break 'read_lines;
            };
            let Some((key, value)) = line.split_once('=') else {
                // Not an INI key-value pair
                continue 'read_lines;
            };

            // Parse the value
            match key.trim() {
                // Select correct slot
                "WIFI_SSID" => Self::read_str(value, &mut wifi_ssid),
                "WIFI_PASS" => Self::read_str(value, &mut wifi_pass),
                "MQTT_ADDR" => Self::read_str(value, &mut mqtt_addr),
                "MQTT_USER" => Self::read_str(value, &mut mqtt_user),
                "MQTT_PASS" => Self::read_str(value, &mut mqtt_pass),
                "MQTT_PRFX" => Self::read_str(value, &mut mqtt_prfx),
                "SENSOR_SLEEP_SECS" => Self::read_secs(value, &mut sensor_sleep_secs),
                "SENSOR_ALERT_SECS" => Self::read_secs(value, &mut sensor_alert_secs),
                // Unknown INI line; skip it
                _ => continue 'read_lines,
            };
        }

        // Validate that the config contains no empty values anymore
        Self {
            WIFI_SSID: Self::unwrap_or_default("WIFI_SSID", wifi_ssid, "DEFAULT_WIFI_SSID"),
            WIFI_PASS: Self::unwrap_or_default("WIFI_PASS", wifi_pass, "DEFAULT_WIFI_PASS"),
            MQTT_ADDR: Self::unwrap_or_default("MQTT_ADDR", mqtt_addr, "DEFAULT_MQTT_ADDR"),
            MQTT_USER: Self::unwrap_or_default("MQTT_USER", mqtt_user, ""),
            MQTT_PASS: Self::unwrap_or_default("MQTT_PASS", mqtt_pass, ""),
            MQTT_PRFX: Self::unwrap_or_default("MQTT_PRFX", mqtt_prfx, ""),
            SENSOR_SLEEP_SECS: Self::unwrap_or_default("SENSOR_SLEEP_SECS", sensor_sleep_secs, DEFAULT_DURATION),
            SENSOR_ALERT_SECS: Self::unwrap_or_default("SENSOR_ALERT_SECS", sensor_alert_secs, DEFAULT_DURATION),
        }
    }

    /// Reads a string value into the given target slot if the slot is empty
    fn read_str(value: &'static str, target: &mut Option<&'static str>) {
        if target.is_none() {
            // Set value
            let value = value.trim();
            *target = Some(value);
        }
    }

    /// Reads a second duration into the given target slot if the slot is empty
    fn read_secs(value: &'static str, target: &mut Option<Duration>) {
        if target.is_none() {
            let Ok(value) = value.parse() else {
                // Log warning and ignore
                debug_println!("[warn] invalid config value: {}", value);
                return;
            };

            // Set value
            let value = Duration::from_secs(value);
            *target = Some(value);
        }
    }

    /// Unwraps the given value or logs a warning and falls back to the default
    fn unwrap_or_default<T>(name: &str, value: Option<T>, default: T) -> T {
        if let Some(value) = value {
            // Value has been set
            value
        } else {
            // Log error and use default
            debug_println!("[warn] using default config value: {}", name);
            default
        }
    }
}
