//! Panic handler and after-panic signalizer

use crate::debug::{StatusLedMode, StatusLedSession};
use crate::debug_println;
use crate::watchdog::Lifecycle;
use core::panic::PanicInfo;
use cortex_m::asm;
use cortex_m::peripheral::SCB;
use embassy_time::{Duration, Timer};

/// Graceful after-panic handler to signalize the panic to the user
pub async fn after_panic(led: &StatusLedSession) -> ! {
    /// The post-panic signal duration
    const PANIC_DURATION: Duration = Duration::from_secs(5);

    // After this handler, we want to enter the normal cycle again
    Lifecycle::store(Lifecycle::LIGHTSLEEP);
    debug_println!("[info] executing after-panic task");

    // Blink the LED to signal the panic; use 1/3 of the app-timeout as reference
    led.set(StatusLedMode::Blink);
    Timer::after(PANIC_DURATION).await;

    // Perform reset
    debug_println!("[info] performing graceful post-panic reset");
    SCB::sys_reset();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Disable all interrupts
    cortex_m::interrupt::disable();
    debug_println!("{}", info);

    // Crash and wait until the watchdog kills us
    // Note: If the watchdog is not yet running, we crash so early that a normal reset wouldn't help either.
    asm::bkpt();
    asm::udf();
}
