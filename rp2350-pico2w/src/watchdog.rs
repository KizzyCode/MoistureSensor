//! Provides reset-after functionality

use crate::debug_println;
use core::sync::atomic::{AtomicU32, Ordering};
use core::u32;
use cortex_m::asm;
use cortex_m::peripheral::{NVIC, SCB};
use embassy_executor::Spawner;
use embassy_rp::pac::clocks::vals::{ClkRefCtrlSrc, ClkSysCtrlSrc};
use embassy_rp::pac::{CLOCKS, Interrupt, POWMAN, WATCHDOG};
use embassy_rp::peripherals::{RTC, WATCHDOG};
use embassy_rp::{Peri, interrupt};
use embassy_time::{Duration, Instant, Timer};

/// Helper macro to write registers with special requirements
macro_rules! write_reg {
    (powman: $register:expr => |$name:ident| $write:expr) => {{
        $register.modify(|$name| {
            // Provide the safety password
            $name.0 = 0x5AFE0000 | ($name.0 & 0x0000FFFF);
            $write
        })
    }};
    (other: $register:expr => |$name:ident| $modify:expr) => {{
        // Modify as-is
        $register.modify(|$name| $modify)
    }};
}

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
    pub const LIGHTSLEEP: u32 = 156439317;

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
    watchdog: Option<Peri<'static, WATCHDOG>>,
    /// Watchdog deadline in seconds
    deadline_secs: AtomicU32,
}
impl Watchdog {
    /// The watchdog timeout (currently ~16s)
    pub const TIMEOUT: Duration = Duration::from_micros(0xFFFFFF);
    /// The watchdog feeding interval with a sufficient safety margin
    pub const FEED_INTERVAL: Duration = Duration::from_micros((Watchdog::TIMEOUT.as_micros() / 100) * 75);

    /// Creates a new watchdog instance from the peripheral
    pub const fn new(peripheral: Peri<'static, WATCHDOG>) -> Self {
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

    /// Feeds the watchdog and performs a light-sleep for the given duration, then performs a graceful reset
    pub async fn reset_after(self, _rtc: Peri<'static, RTC>, mut scb: SCB, timeout: Duration) -> ! {
        unsafe {
            // Disable all interrupts so WFI doesn't trigger unexpectedly
            (*NVIC::PTR).icer[0].write(u32::MAX);
            (*NVIC::PTR).icer[1].write(u32::MAX);
        };

        // Stop the AON timer so we can configure it
        write_reg!(powman: POWMAN.badpasswd() => |w| w.set_badpasswd(true));
        write_reg!(powman: POWMAN.timer() => |w| w.set_use_xosc(true));

        // Configure sys and ref clocks to use XOSC, and disable all clocks besides the AON during sleep
        write_reg!(other: CLOCKS.clk_ref_ctrl() => |w| w.set_src(ClkRefCtrlSrc::XOSC_CLKSRC));
        write_reg!(other: CLOCKS.clk_sys_ctrl() => |w| w.set_src(ClkSysCtrlSrc::CLK_REF));
        write_reg!(other: CLOCKS.sleep_en0() => |w| w.set_clk_ref_powman(true));
        write_reg!(other: CLOCKS.sleep_en1() => |w| w.0 = 0);

        // Setup powman so we can schedule the alarm
        write_reg!(powman: POWMAN.badpasswd() => |w| w.set_badpasswd(true));
        write_reg!(powman: POWMAN.timer() => |w| {
            // Disable timer and alarm, and switch to XOSC
            w.set_alarm_enab(false);
            w.set_use_xosc(true);
            w.set_alarm(true);
            w.set_run(false);
        });

        // Configure alarm and restart clock
        let alert_ms = timeout.as_millis();
        write_reg!(powman: POWMAN.alarm_time_15to0() => |w| w.set_alarm_time_15to0((alert_ms >> 0) as u16));
        write_reg!(powman: POWMAN.alarm_time_31to16() => |w| w.set_alarm_time_31to16((alert_ms >> 16) as u16));
        write_reg!(powman: POWMAN.alarm_time_47to32() => |w| w.set_alarm_time_47to32((alert_ms >> 32) as u16));
        write_reg!(powman: POWMAN.alarm_time_63to48() => |w| w.set_alarm_time_63to48((alert_ms >> 48) as u16));
        write_reg!(powman: POWMAN.timer() => |w| {
            // Resume the AON timer, and reset it to 0
            w.set_run(true);
            w.clear();
        });

        // Enable the alarm interrupt and ensure the badpasswd-register is clear (sanity check)
        write_reg!(powman: POWMAN.timer() => |w| w.set_alarm_enab(true));
        write_reg!(powman: POWMAN.inte() => |w| w.set_timer(true));
        assert!(!POWMAN.badpasswd().read().badpasswd(), "failed to set powman register");
        unsafe { NVIC::unmask(Interrupt::POWMAN_IRQ_TIMER) };

        // Start deepsleep
        scb.set_sleepdeep();
        asm::wfi();

        // Perform a graceful reboot afterwards
        debug_println!("[info] performing graceful reset");
        WATCHDOG.ctrl().write(|w| w.set_trigger(true));
        loop {
            // Wait for the reset
            // Note: Use nop to avoid rust-lang/rust#28728
            asm::nop();
        }
    }
}

/// [`Watchdog`] task
#[embassy_executor::task]
async fn watchdog_task(deadline_secs: &'static AtomicU32, mut watchdog: embassy_rp::watchdog::Watchdog) {
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
        Timer::after(Watchdog::FEED_INTERVAL).await;
    }
}

/// AON clock interrupt handler
#[interrupt]
#[allow(non_snake_case)]
fn POWMAN_IRQ_TIMER() {
    // Disable the interrupt only when the alarm is done
    debug_println!("[info] aom clock interrupt fired");
    if POWMAN.timer().read().alarm() {
        // Disable interrupt to exit the IRQ deadloop
        NVIC::mask(Interrupt::POWMAN_IRQ_TIMER);
        debug_println!("[info] disabled aom clock interrupt");
    }
}
