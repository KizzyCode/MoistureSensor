//! Wifi magic

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use cyw43::{Control, JoinOptions, PowerManagementMode, SpiBusCyw43, State};
use cyw43_firmware::{CYW43_43439A0, CYW43_43439A0_CLM};
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use embassy_executor::Spawner;
use embassy_net::{Stack, StackResources};
use embassy_rp::Peripheral;
use embassy_rp::gpio::{Level, Output, Pin};
use embassy_rp::interrupt::typelevel::Binding;
use embassy_rp::pac::ROSC;
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{Instance, InterruptHandler, Pio, PioPin};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Timer;
use embedded_hal::digital::{ErrorType, OutputPin};

/// [`cyw43::Runner`] for [`Pio0Dma0Spi`]
type Cyw43Runner = cyw43::Runner<'static, &'static SharedOutput, &'static mut Pio0Dma0Spi>;

/// [`embassy_net::Runner`] for [`cyw43::NetDriver`]
type NetworkRunner = embassy_net::Runner<'static, cyw43::NetDriver<'static>>;

/// Shared wrapper to share an output pin across contexts
struct SharedOutput(RefCell<Output<'static>>);
impl ErrorType for &'static SharedOutput {
    type Error = <Output<'static> as ErrorType>::Error;
}
impl OutputPin for &'static SharedOutput {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(self.0.borrow_mut().set_low())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(self.0.borrow_mut().set_high())
    }
}

/// Wrapper to implement [`SpiBusCyw43`] for a mutable reference over [`PioSpi`] with [`PIO0`]/[`DMA_CH0`]
struct Pio0Dma0Spi(PioSpi<'static, PIO0, 0, DMA_CH0>);
impl SpiBusCyw43 for &mut Pio0Dma0Spi {
    async fn cmd_write(&mut self, write: &[u32]) -> u32 {
        SpiBusCyw43::cmd_write(&mut self.0, write).await
    }
    async fn cmd_read(&mut self, write: u32, read: &mut [u32]) -> u32 {
        SpiBusCyw43::cmd_read(&mut self.0, write, read).await
    }
    async fn wait_for_event(&mut self) {
        SpiBusCyw43::wait_for_event(&mut self.0).await
    }
}

// CYW43 peripheral config
pub struct Cyw43Config;
impl Cyw43Config {
    /// Creates a config from the given PIO and associated IRQ handlers and DMA channel
    pub fn new<IRQS>(pio: PIO0, irqs: IRQS, dma: DMA_CH0) -> Cyw43ConfigWithPio<IRQS>
    where
        IRQS: Binding<<<PIO0 as Peripheral>::P as Instance>::Interrupt, InterruptHandler<<PIO0 as Peripheral>::P>>,
    {
        Cyw43ConfigWithPio { pio, irqs, dma }
    }
}

/// CYW43 peripheral config
pub struct Cyw43ConfigWithPio<IRQS> {
    /// The PIO peripheral
    pio: PIO0,
    /// The IRQ handlers
    irqs: IRQS,
    /// The
    dma: DMA_CH0,
}
impl<IRQS> Cyw43ConfigWithPio<IRQS> {
    /// Sets the power-select line and the chip-select, data and clock SPI lines
    pub fn set_pins<P, S, D, C>(self, powerselect: P, select: S, data: D, clock: C) -> Cyw43
    where
        IRQS: Binding<<<PIO0 as Peripheral>::P as Instance>::Interrupt, InterruptHandler<<PIO0 as Peripheral>::P>>,
        P: Peripheral + 'static,
        P::P: Pin,
        S: Peripheral + 'static,
        S::P: Pin,
        D: PioPin,
        C: PioPin,
    {
        // Setup GPIO control pins
        let powerselect = Output::new(powerselect, Level::Low);
        let select = Output::new(select, Level::High);

        // Setup PIO SPI bus
        let mut pio = Pio::new(self.pio, self.irqs);
        let spi = PioSpi::new(&mut pio.common, pio.sm0, RM2_CLOCK_DIVIDER, pio.irq0, select, data, clock, self.dma);

        // Create peripheral
        Cyw43 {
            powerselect: SharedOutput(RefCell::new(powerselect)),
            spi: Pio0Dma0Spi(spi),
            state: State::new(),
            stack: StackResources::new(),
            stop: AtomicBool::new(false),
        }
    }
}

/// Fully configured CYW43 peripheral
pub struct Cyw43 {
    /// The power-select line
    powerselect: SharedOutput,
    /// The PIO-driven SPI bus
    spi: Pio0Dma0Spi,
    /// Radio state
    state: State,
    /// Network stack pool
    stack: StackResources<5>,
    /// Stop signal
    stop: AtomicBool,
}
impl Cyw43 {
    /// Starts the CYW43 chip and initializes the firmware and network stack
    pub async fn boot(&'static mut self, spawner: &Spawner) -> (Cyw43Session, Stack<'static>) {
        // Start the CYW43 peripheral
        let (netdevice, mut radio, runner) =
            cyw43::new(&mut self.state, &self.powerselect, &mut self.spi, CYW43_43439A0).await;
        spawner.must_spawn(cyw43_session_task(&self.stop, runner));

        // Load firmware and set power management
        radio.init(CYW43_43439A0_CLM).await;
        radio.set_power_management(PowerManagementMode::PowerSave).await;

        // Prepare network stack and generate random seed
        let netconfig = embassy_net::Config::dhcpv4(Default::default());

        // Generate network stack seed
        let mut random_seed = 0;
        for shl in 0..64 {
            // Collect 64 sufficiently random bits
            let bit = ROSC.randombit().read().randombit();
            random_seed |= (bit as u64) << shl;
        }

        // Start network stack
        let (stack, runner) = embassy_net::new(netdevice, netconfig, &mut self.stack, random_seed);
        spawner.must_spawn(cyw43_network_task(&self.stop, runner));

        // Create session handle and return session and stack
        let radio = Mutex::new(radio);
        let session = Cyw43Session { powerselect: &self.powerselect, signal: &self.stop, radio };
        (session, stack)
    }
}

/// A [`Cyw43`] session
pub struct Cyw43Session {
    /// The power-select line
    powerselect: &'static SharedOutput,
    /// Signal flags
    signal: &'static AtomicBool,
    /// CYW43 session controller
    radio: Mutex<ThreadModeRawMutex, Control<'static>>,
}
impl Cyw43Session {
    /// Joins the given wifi network
    pub async fn join(&self, config: &crate::Config) {
        let options = JoinOptions::new(config.WIFI_PASS.as_bytes());
        self.radio.lock().await.join(config.WIFI_SSID, options).await.expect("failed to join wifi network")
    }

    /// Performs a shutdown of the CYW43 chip
    ///
    /// # Important
    /// As this function shuts down all entire radio chip, the associated network device also becomes unusable. Using
    /// the device and associated network stack after shutdown may result in weird errors or unexpected side effects.
    pub async fn shutdown(&self) {
        // Disconnect from WiFi
        self.radio.lock().await.leave().await;
        Timer::after_millis(500).await;

        // Stop the worker task
        self.signal.store(true, Ordering::SeqCst);
        Timer::after_millis(500).await;

        // Send power-off signal to chip (in practice, this can never fail)
        let mut powerselect = self.powerselect;
        powerselect.set_low().expect("failed to set powerselect line to low");
    }

    /// Checks if the session has been shutdown
    #[must_use]
    pub fn is_shutdown(&self) -> bool {
        self.signal.load(Ordering::SeqCst)
    }

    /// Sets the status LED
    pub async fn set_led(&self, high: bool) {
        self.radio.lock().await.gpio_set(0, high).await;
    }
}
impl Drop for Cyw43Session {
    fn drop(&mut self) {
        // Panic in debug mode as the session should always be shut down instead of just dropped
        debug_assert!(self.signal.load(Ordering::SeqCst), "did not shutdown CYW43 session");
    }
}

/// [`Cyw43Session`] task
#[embassy_executor::task]
async fn cyw43_session_task(stop: &'static AtomicBool, runner: Cyw43Runner) {
    if !stop.load(Ordering::SeqCst) {
        // Poll if we are not stopped
        runner.run().await
    }
}

/// [`Cyw43Session`] network task
#[embassy_executor::task]
async fn cyw43_network_task(stop: &'static AtomicBool, mut runner: NetworkRunner) {
    if !stop.load(Ordering::SeqCst) {
        // Poll if we are not stopped
        runner.run().await
    }
}
