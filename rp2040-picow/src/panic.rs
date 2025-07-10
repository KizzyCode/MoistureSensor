//! Panic handler and after-panic signalizer

use crate::config::Config;
use crate::debug::{StatusLedMode, StatusLedSession};
use crate::debug_println;
use crate::watchdog::{Lifecycle, WatchdogController};
use core::panic::PanicInfo;
use cortex_m::asm;
use cortex_m::peripheral::SCB;

/// Graceful after-panic handler to signalize the panic to the user
pub async fn after_panic(config: &Config, watchdog: &WatchdogController, led: &StatusLedSession) -> ! {
    // After this handler, we want to enter the normal cycle again
    Lifecycle::store(Lifecycle::DEEPSLEEP);
    debug_println!("[info] executing after-panic task");

    // Blink the LED to signal the panic and reboot into normal operation again
    led.set(StatusLedMode::Blink);
    watchdog.reset_after(config.SENSOR_ALERT_SECS).await;
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
