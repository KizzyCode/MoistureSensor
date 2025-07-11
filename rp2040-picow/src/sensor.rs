//! Moisture sensor handling

use crate::Irqs;
use embassy_rp::Peripheral;
use embassy_rp::adc::{Adc, AdcPin, Async, Channel, Config};
use embassy_rp::gpio::Pull;
use embassy_rp::peripherals::{ADC, ADC_TEMP_SENSOR};

/// ~732 Hz sample rate (the lowest possible sample rate)
const SAMPLE_RATE: u16 = u16::MAX;
/// Sample count to sample ~1.5s
const SAMPLE_COUNT: usize = 1024;

pub struct SensorReadout {
    pub sensor: f64,
    pub temperature: f64,
}

/// The moisture sensor
pub struct Sensor<D> {
    /// ADC driver
    adc: Adc<'static, Async>,
    /// ADC DMA channel
    dma: D,
    /// ADC channels (sensor, temperature)
    channels: [Channel<'static>; 2],
}
impl<D> Sensor<D>
where
    D: Peripheral + 'static,
    D::P: embassy_rp::dma::Channel,
{
    /// Creates a new sensor instance
    pub fn new<S, T>(adc: ADC, irqs: Irqs, dma: D, sensor_pin: S, temperature_pin: T) -> Self
    where
        S: Peripheral + 'static,
        S::P: AdcPin,
        T: Peripheral<P = ADC_TEMP_SENSOR> + 'static,
    {
        // Setup ADC driver and channel
        let adc = Adc::new(adc, irqs, Config::default());
        let sensor = Channel::new_pin(sensor_pin, Pull::None);
        let temperature = Channel::new_temp_sensor(temperature_pin);
        Self { adc, dma, channels: [sensor, temperature] }
    }

    /// Reads the connected sensors
    pub async fn read(&mut self) -> SensorReadout {
        // Do some supersampling
        // Note: Samples are stored interleaved, so double the capacity
        let mut samples = [0u16; SAMPLE_COUNT * 2];
        (self.adc.read_many_multichannel(&mut self.channels, &mut samples, SAMPLE_RATE, &mut self.dma))
            // Note: This should never fail under normal conditions
            .await.expect("failed to read sensor channel");

        // Process and sum interleaved samples
        let (samples, _) = samples.as_chunks();
        let sensor_total: u64 = samples.iter().map(|[sensor, _]| *sensor as u64).sum();
        let temperature_total: u64 = samples.iter().map(|[_, temperature]| *temperature as u64).sum();

        // Compute temperature
        // Note: According to chapter 4.9.5. Temperature Sensor in RP2040 datasheet
        let temperature_raw = (temperature_total as f64) / (SAMPLE_COUNT as f64);
        let temperature = 27.0 - (temperature_raw * 3.3 / 4096.0 - 0.706) / 0.001721;
        let temperature = match temperature {
            _ if temperature < 0.0 => temperature - 0.05,
            _ => temperature + 0.05,
        };

        // Compute sensor voltage
        let sensor_raw = (sensor_total as f64) / (SAMPLE_COUNT as f64);
        let sensor = (sensor_raw * 3.3) / 4096.0;
        SensorReadout { sensor, temperature }
    }
}
