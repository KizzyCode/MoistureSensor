#![no_std]
#![no_main]

mod config;
mod debug;
mod mqtt;
mod panic;
mod sensor;
mod watchdog;
mod wifi;

use crate::config::AppConfig;
use crate::debug::{StatusLed, StatusLedMode};
use crate::mqtt::{MqttBuffer, MqttStack};
use crate::sensor::Sensor;
use crate::watchdog::{Lifecycle, Watchdog};
use crate::wifi::{Cyw43, Cyw43Config, Cyw43Session};
use cortex_m::Peripherals;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::clocks::ClockConfig;
use embassy_rp::config::Config;
use embassy_rp::peripherals::PIO0;
use embassy_time::Duration;
use static_cell::StaticCell;

/// The application timeout
const APP_TIMEOUT: Duration = Duration::from_secs(45);

// Bind required interrupt handlers
bind_interrupts!(struct Irqs {
    // PIO0 interrupt handler
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    // ADC channel interrupt handler
    ADC_IRQ_FIFO => embassy_rp::adc::InterruptHandler;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    /// The system frequency in Hz
    const SYSTEM_FREQ_HZ: u32 = 30_000_000;

    /// Static watchdog peripheral
    static WATCHDOG: StaticCell<Watchdog> = StaticCell::new();
    /// Static CYW43 peripheral handle
    static CYW43: StaticCell<Cyw43> = StaticCell::new();
    /// Static radio control session
    static RADIO: StaticCell<Cyw43Session> = StaticCell::new();
    /// Static status LED control session
    static LED: StaticCell<StatusLed> = StaticCell::new();

    // Setup device
    let mut hw_config = Config::default();
    hw_config.clocks = ClockConfig::system_freq(SYSTEM_FREQ_HZ).expect("failed to build clock config");
    let hw = embassy_rp::init(hw_config);

    // Get peripherals and take reset info before doing anything else
    let peripherals = Peripherals::take().expect("failed to take peripherals");
    let lifecycle_before_reset = Lifecycle::load();
    debug_println!("[info] lifecycle before reset: {:?}", lifecycle_before_reset);

    // Setup watchdog
    let watchdog = WATCHDOG.init(Watchdog::new(hw.WATCHDOG));
    let watchdog = watchdog.start(APP_TIMEOUT, &spawner);
    Lifecycle::store(Lifecycle::WATCHDOG);
    debug_println!("[info] watchdog initialized");

    // Load device config
    let config = AppConfig::load();
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
    let true = matches!(lifecycle_before_reset, Some(Lifecycle::LIGHTSLEEP)) else {
        // Apparently the previous app has not stopped gracefully
        panic::after_panic(&led).await;
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
    // Note: The ADC draws some current, so ensure it is dropped asap
    let mut sensor = Sensor::new(hw.ADC, Irqs, hw.DMA_CH1, hw.PIN_27, hw.PIN_28, hw.ADC_TEMP_SENSOR);
    let readings = sensor.read().await;
    drop(sensor);
    debug_println!("[info] read sensor values");

    // Scope the MQTT buffers due to stack size
    {
        // Publish sensor voltage
        let sensor = MqttBuffer::from_display(readings.sensor);
        mqtt.publish("voltage", &sensor).await;
        debug_println!("[info] published sensor voltage: {}", readings.sensor);
    }
    {
        // Publish chip temperature
        let temperature_str = MqttBuffer::from_display(readings.temperature);
        mqtt.publish("temperature", &temperature_str).await;
        debug_println!("[info] published system temperature: {}", readings.temperature);
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
    Lifecycle::store(Lifecycle::LIGHTSLEEP);
    debug_println!("[info] entering sleep");
    watchdog.reset_after(hw.RTC, peripherals.SCB, config.SENSOR_SLEEP_SECS).await;
}
