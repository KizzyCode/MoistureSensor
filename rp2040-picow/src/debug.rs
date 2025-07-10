//! User communication for status update

use crate::wifi::Cyw43Session;
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};

/// Prints a line via semi-hosting for debug builds only
#[macro_export]
macro_rules! debug_println {
    ($s:expr) => {{
        if cfg!(debug_assertions) {
            // Print via semihosting
            // Note: This crashes if no debugger is attached
            cortex_m_semihosting::hprintln!($s);
        }
    }};
    ($s:expr, $($tt:tt)*) => {{
        if cfg!(debug_assertions) {
            // Print via semihosting
            // Note: This crashes if no debugger is attached
            cortex_m_semihosting::hprintln!($s, $($tt)*);
        }
    }};
}

/// Status LED mode
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum StatusLedMode {
    /// LED off
    Off,
    /// LED on
    On,
    /// Fast blinking (4/s)
    Blink,
}

/// The status LED handler
pub struct StatusLed {
    /// Status LED mode
    mode: AtomicU8,
    /// Radio peripheral (the LED is controlled via the radio lol)
    radio: &'static Cyw43Session,
}
impl StatusLed {
    /// Creates a new status LED handler
    pub const fn new(radio: &'static Cyw43Session) -> Self {
        let mode = AtomicU8::new(StatusLedMode::Off as u8);
        Self { mode, radio }
    }

    /// Starts the status LED task
    pub fn start(&'static self, spawner: &Spawner) -> StatusLedSession {
        spawner.must_spawn(status_led_task(&self.mode, self.radio));
        StatusLedSession { mode: &self.mode }
    }
}

/// A [`StatusLed`] session
#[derive(Clone, Copy)]
pub struct StatusLedSession {
    /// LED mode
    mode: &'static AtomicU8,
}
impl StatusLedSession {
    /// Sets the status LED to the given mode
    pub fn set(&self, mode: StatusLedMode) {
        self.mode.store(mode as u8, Ordering::SeqCst);
    }
}

/// [`Cyw43Session`] network task
#[embassy_executor::task]
async fn status_led_task(mode: &'static AtomicU8, radio: &'static Cyw43Session) {
    /// Toggle interval for LED blinking
    const BLINK_INTERVAL: Duration = Duration::from_millis(125);

    // Init the LED to a known state
    let mut state = false;
    let mut last_toggle = Instant::now();
    radio.set_led(state).await;

    // Toggle state if appropriate
    while !radio.is_shutdown() {
        // Determine whether we should toggle the LED state
        let wants_toggle = match mode.load(Ordering::SeqCst) {
            mode if mode == StatusLedMode::Off as u8 => state != false,
            mode if mode == StatusLedMode::On as u8 => state != true,
            mode if mode == StatusLedMode::Blink as u8 => Instant::now() > last_toggle + BLINK_INTERVAL,
            mode => unreachable!("invalid status led mode: {mode}"),
        };

        // Update state if appropriate
        if wants_toggle {
            state = !state;
            last_toggle = Instant::now();
            radio.set_led(state).await;
        }

        // Sleep some time
        Timer::after(BLINK_INTERVAL).await;
    }
}
