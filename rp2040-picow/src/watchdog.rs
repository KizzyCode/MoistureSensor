//! Provides reset-after functionality

use crate::debug_println;
use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m::peripheral::SCB;
use embassy_executor::Spawner;
use embassy_rp::pac::WATCHDOG;
use embassy_rp::peripherals::WATCHDOG;
use embassy_time::{Duration, Instant, Timer};

/// Lifecycle hints that persist across resets
#[derive(Debug, Clone, Copy)]
pub struct Lifecycle;
impl Lifecycle {
    /// The watchdog has been started, but not much more happened
    pub const WATCHDOG: u32 = 367300213;
    /// The radio peripheral has been initialized
    pub const RADIOINIT: u32 = 968074460;
    /// The main application logic has been entered
    pub const APPINIT: u32 = 3422455895;
    /// The main application logic has finished
    pub const DEEPSLEEP: u32 = 156439317;

    /// The scratch checksum XOR constant
    const CHECKSUM_XOR: u32 = 0x2144DF9C;

    /// Persists the current lifecycle
    pub fn store(lifecycle: u32) {
        WATCHDOG.scratch0().write_value(lifecycle);
        WATCHDOG.scratch1().write_value(lifecycle ^ Self::CHECKSUM_XOR);
    }

    /// Loads the last-persisted lifecycle
    pub fn load() -> Option<u32> {
        let lifecycle = WATCHDOG.scratch0().read();
        let checksum = WATCHDOG.scratch1().read();
        (lifecycle == (checksum ^ Self::CHECKSUM_XOR)).then_some(lifecycle)
    }
}

/// Watchdog wrapper
pub struct Watchdog {
    /// Underlying watchdog peripheral
    watchdog: Option<WATCHDOG>,
    /// Watchdog deadline in seconds
    deadline_secs: AtomicU32,
}
impl Watchdog {
    /// The watchdog timeout (currently ~8s)
    pub const TIMEOUT: Duration = Duration::from_micros(0xFFFFFF / 2);

    /// Creates a new watchdog instance from the peripheral
    pub const fn new(peripheral: WATCHDOG) -> Self {
        Self { watchdog: Some(peripheral), deadline_secs: AtomicU32::new(0) }
    }

    /// Starts the watchdog and setups the controller with the given initial timeout
    pub fn start(&'static mut self, timeout: Duration, spawner: &Spawner) -> WatchdogController {
        // Consume watchdog peripheral and create rich type
        let peripheral = self.watchdog.take().expect("watchdog has already been consumed");
        let mut watchdog = embassy_rp::watchdog::Watchdog::new(peripheral);

        // Setup the control plane and configure the initial timeout
        // Note: The initial timeout is important to ensure that the task does not exit immediately
        let controller = WatchdogController { deadline_secs: &self.deadline_secs };
        controller.set_timeout(timeout);

        // Start watchdog
        watchdog.pause_on_debug(true);
        watchdog.start(Self::TIMEOUT);

        // Initialize controlplane, set initial timeout and start task
        spawner.must_spawn(watchdog_task(&self.deadline_secs, watchdog));
        controller
    }
}

/// A controller for a started watchdog
#[derive(Debug, Clone, Copy)]
pub struct WatchdogController {
    /// Watchdog deadline in seconds
    deadline_secs: &'static AtomicU32,
}
impl WatchdogController {
    /// Sets a new watchdog timeout
    pub fn set_timeout(&self, timeout: Duration) {
        // The instant starts with `0` at boot; so at a second-scale this should never overflow
        let deadline = Instant::now() + timeout;
        let deadline_secs = u32::try_from(deadline.as_secs()).expect("timeout is too large");
        self.deadline_secs.store(deadline_secs, Ordering::SeqCst);
    }

    /// Feeds the watchdog for the given duration and performs a graceful reset via [`SCB::sys_reset`] afterwards
    pub async fn reset_after(self, timeout: Duration) -> ! {
        // TODO: Turn off unneeded peripherals?
        //scb.set_sleepdeep() maybe?

        // Wait the given duration
        // Note: Because the feeding interval is 75% of the watchdog interval, there is enough time to reset after the
        //  timeout expires.
        self.set_timeout(timeout);
        Timer::after(timeout).await;

        // Perform reset
        debug_println!("[info] performing graceful reset");
        SCB::sys_reset();
    }
}

/// [`Watchdog`] task
#[embassy_executor::task]
async fn watchdog_task(deadline_secs: &'static AtomicU32, mut watchdog: embassy_rp::watchdog::Watchdog) {
    /// The watchdog feeding interval with a sufficient safety margin
    pub const FEED_INTERVAL: Duration = Duration::from_micros((Watchdog::TIMEOUT.as_micros() / 100) * 75);

    /// The current instant in secnds
    #[inline]
    fn now_secs() -> u32 {
        // The instant starts with `0` at boot, so this should not overflow
        let now = Instant::now().as_secs();
        u32::try_from(now).expect("instant timestamp is too large")
    }

    loop {
        // Load and check the deadline from the controller
        if now_secs() <= deadline_secs.load(Ordering::SeqCst) {
            // Feed watchdog if the deadline has not expired yet
            watchdog.feed();
            debug_println!("[info] fed watchdog");
        } else {
            // Log a warning if the watchdog deadline has expired
            debug_println!("[warn] watchdog deadline expired");
        }

        // Always yield some time to allow other tasks to run
        Timer::after(FEED_INTERVAL).await;
    }
}
