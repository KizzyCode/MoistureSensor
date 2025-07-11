#![no_std]
#![no_main]

mod config;
mod debug;
mod mqtt;
mod panic;
mod sensor;
mod watchdog;
mod wifi;

use crate::config::Config;
use crate::debug::{StatusLed, StatusLedMode};
use crate::mqtt::{MqttBuffer, MqttStack};
use crate::sensor::Sensor;
use crate::watchdog::{Lifecycle, Watchdog};
use crate::wifi::{Cyw43, Cyw43Config, Cyw43Session};
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::InterruptHandler;
use embassy_time::Duration;
use static_cell::StaticCell;

/// The application timeout
pub const APP_TIMEOUT: Duration = Duration::from_secs(45);

bind_interrupts!(struct Irqs {
    // PIO0 interrupt handler
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    /// Static watchdog peripheral
    static WATCHDOG: StaticCell<Watchdog> = StaticCell::new();
    /// Static CYW43 peripheral handle
    static CYW43: StaticCell<Cyw43> = StaticCell::new();
    /// Static radio control session
    static RADIO: StaticCell<Cyw43Session> = StaticCell::new();
    /// Static status LED control session
    static LED: StaticCell<StatusLed> = StaticCell::new();

    // Get peripherals and grab reset info before doing anything else
    let hw = embassy_rp::init(Default::default());
    let lifecycle_before_reset = Lifecycle::load();
    debug_println!("[info] lifecycle before reset: {:?}", lifecycle_before_reset);

    // Setup watchdog
    let watchdog = WATCHDOG.init(Watchdog::new(hw.WATCHDOG));
    let watchdog = watchdog.start(APP_TIMEOUT, &spawner);
    Lifecycle::store(Lifecycle::WATCHDOG);
    debug_println!("[info] watchdog initialized");

    // Load device config
    let config = Config::load();
    debug_println!("[info] loaded config: {:?}", config);

    // Setup radio and init network stack
    let radio =
        CYW43.init(Cyw43Config::new(hw.PIO0, Irqs, hw.DMA_CH0).set_pins(hw.PIN_23, hw.PIN_25, hw.PIN_24, hw.PIN_29));
    let (radio, network) = radio.boot(&spawner).await;
    Lifecycle::store(Lifecycle::RADIOINIT);
    debug_println!("[info] initialized radio");

    // Setup radio and LED control sessions
    let radio = RADIO.init(radio);
    let led = LED.init(StatusLed::new(radio));
    let led = led.start(&spawner);

    // We now have everything set up to divert to the after-panic handler if appropriate
    let true = matches!(lifecycle_before_reset, Some(Lifecycle::DEEPSLEEP)) else {
        // Apparently the previous app has not stopped gracefully
        panic::after_panic(&config, &watchdog, &led).await;
    };

    //
    // Enter main application logic
    //
    Lifecycle::store(Lifecycle::APPINIT);
    led.set(StatusLedMode::On);

    // Try to join network
    radio.join(&config).await;
    debug_println!("[info] joined wifi: {}", config.WIFI_SSID);

    // Wait for link
    network.wait_link_up().await;
    debug_println!("[info] got network link");

    // Wait for DHCP
    network.wait_config_up().await;
    debug_println!("[info] got dhcp config");

    // Init MQTT stack
    let mut mqtt = MqttStack::new(network);
    let mqtt = mqtt.init(&config);

    // Connect to MQTT server
    let mqtt = mqtt.connect().await;
    debug_println!("[info] connected to mqtt server");

    // Establish MQTT session
    let mut mqtt = mqtt.login().await;
    debug_println!("[info] established mqtt session");

    // Read sensor and chip temperature
    // Note: The ADC draws some current, so ensure it is dropped immediately
    let ((sensor_voltage, sensor_raw), (sys_temp, _)) = {
        let mut sensor = Sensor::new(hw.ADC, hw.PIN_28, hw.ADC_TEMP_SENSOR);
        (sensor.read_pin(), sensor.read_temperature())
    };

    // Scope the MQTT buffers due to stack size
    {
        // Publish raw sensor value
        let sensor_raw_str = MqttBuffer::from_display(sensor_raw);
        mqtt.publish("raw", &sensor_raw_str).await;
        debug_println!("[info] published mqtt raw sensor value: {}", sensor_raw);
    }
    {
        // Publish sensor voltage
        let sensor_voltage_str = MqttBuffer::from_display(sensor_voltage);
        mqtt.publish("voltage", &sensor_voltage_str).await;
        debug_println!("[info] published sensor voltage: {}", sensor_voltage);
    }
    {
        // Publish chip temperature
        let sys_temp_str = MqttBuffer::from_display(sys_temp);
        mqtt.publish("temperature", &sys_temp_str).await;
        debug_println!("[info] published system temperature: {}", sys_temp);
    }

    // Disconnect
    mqtt.disconnect().await;
    debug_println!("[info] disconnected from mqtt server");

    // Shutdown radio (also turns LED off)
    radio.shutdown().await;
    debug_println!("[info] stopped radio");

    //
    // Sleep and perform reset
    //
    Lifecycle::store(Lifecycle::DEEPSLEEP);
    debug_println!("[info] entering sleep");
    watchdog.reset_after(config.SENSOR_SLEEP_SECS).await;
}
