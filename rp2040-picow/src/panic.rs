//! Panic handler and after-panic signalizer

use crate::config::AppConfig;
use crate::debug::{StatusLedMode, StatusLedSession};
use crate::debug_println;
use crate::watchdog::{Lifecycle, WatchdogController};
use core::panic::PanicInfo;
use cortex_m::asm;
use cortex_m::peripheral::SCB;
use embassy_time::Timer;

/// Graceful after-panic handler to signalize the panic to the user
pub async fn after_panic(config: &AppConfig, led: &StatusLedSession, watchdog: &WatchdogController) -> ! {
    // After this handler, we want to enter the normal cycle again
    Lifecycle::store(Lifecycle::LIGHTSLEEP);
    debug_println!("[info] executing after-panic task");

    // Blink the LED to signal the panic
    led.set(StatusLedMode::Blink);
    Timer::after(config.SENSOR_ALERT_SECS).await;

    // Perform reset
    debug_println!("[info] performing graceful post-panic reset");
    watchdog.reset();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Disable all interrupts
    cortex_m::interrupt::disable();
    debug_println!("{}", info);

    // Check the lifecycle and reset accordingly
    match Lifecycle::load() {
        Some(Lifecycle::WATCHDOG) => {
            // We cannot really handle these resets gracefully
            asm::bkpt();
            asm::udf();
        }
        _ => {
            // Do a graceful reset
            SCB::sys_reset();
        }
    }
}
