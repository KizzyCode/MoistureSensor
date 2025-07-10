//! Moisture sensor handling

use embassy_rp::Peripheral;
use embassy_rp::adc::{Adc, AdcPin, Blocking, Channel, Config};
use embassy_rp::gpio::Pull;
use embassy_rp::peripherals::{ADC, ADC_TEMP_SENSOR};

/// The moisture sensor
pub struct Sensor {
    /// ADC driver
    adc: Adc<'static, Blocking>,
    /// Sensor ADC channel
    channel: Channel<'static>,
    /// Temperature ADC channel
    temperature: Channel<'static>,
}
impl Sensor {
    /// Creates a new sensor instance
    pub fn new<S, T>(adc: ADC, sensor_pin: S, temperature_pin: T) -> Self
    where
        S: Peripheral + 'static,
        S::P: AdcPin,
        T: Peripheral<P = ADC_TEMP_SENSOR> + 'static,
    {
        // Setup ADC driver and channel
        let adc = Adc::new_blocking(adc, Config::default());
        let channel = Channel::new_pin(sensor_pin, Pull::None);
        let temperature = Channel::new_temp_sensor(temperature_pin);
        Self { adc, channel, temperature }
    }

    /// Gets estimated voltage and raw readout of the sensor pin
    pub fn read_pin(&mut self) -> (f32, u16) {
        // Note: This should never fail under normal conditions
        let raw = self.adc.blocking_read(&mut self.channel).expect("failed to read sensor channel");
        ((raw as f32 * 3.3) / 65536.0, raw)
    }

    /// Gets estimated temperature in degrees celsius, and the raw readout of the temperature channel
    pub fn read_temperature(&mut self) -> (f32, u16) {
        // Note: This should never fail under normal conditions
        let raw = self.adc.blocking_read(&mut self.temperature).expect("failed to read temperature channel");

        // Compute temperature
        // Note: According to chapter 4.9.5. Temperature Sensor in RP2040 datasheet
        let temp_raw = 27.0 - (raw as f32 * 3.3 / 4096.0 - 0.706) / 0.001721;
        let rounded_temp_x10 = match temp_raw {
            _ if temp_raw < 0.0 => ((temp_raw * 10.0) - 0.5) as i16,
            _ => ((temp_raw * 10.0) + 0.5) as i16,
        };

        // Return temperature and raw value
        ((rounded_temp_x10 as f32) / 10.0, raw)
    }
}
