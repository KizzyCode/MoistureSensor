//! Provides reset-after functionality

use crate::debug_println;
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};
use core::u32;
use cortex_m::asm;
use cortex_m::peripheral::{NVIC, SCB};
use critical_section::Mutex;
use embassy_executor::Spawner;
use embassy_rp::pac::clocks::vals::{ClkRefCtrlSrc, ClkRtcCtrlAuxsrc, ClkSysCtrlSrc};
use embassy_rp::pac::psm::regs::Wdsel;
use embassy_rp::pac::rosc::vals::Enable;
use embassy_rp::pac::watchdog::regs::Load;
use embassy_rp::pac::{CLOCKS, Interrupt, PLL_SYS, PLL_USB, PSM, ROSC, WATCHDOG, XIP_CTRL};
use embassy_rp::peripherals::{RTC, WATCHDOG};
use embassy_rp::rtc::{DateTime, DateTimeFilter, DayOfWeek, Rtc};
use embassy_rp::{Peri, interrupt};
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
    /// The watchdog timeout (currently ~8s)
    pub const TIMEOUT: Duration = Duration::from_micros(0xFFFFFF / 2);
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

        // Configure the watchdog so it resets everything, including ROSC/XOSC
        // Note: This is an additional safety measurement as we do some funny stuff with our clocks during sleep
        PSM.wdsel().write_value(Wdsel(0x0001ffff));

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
    pub fn reset_after(self, rtc: Peri<'static, RTC>, mut scb: SCB, timeout: Duration) -> ! {
        /// The watchdog feed interval in seconds
        const FEED_INTERVAL_SECS: u8 = Watchdog::FEED_INTERVAL.as_secs() as u8;
        /// The default datetime
        const DEFAULT_DATETIME: DateTime =
            DateTime { year: 1970, month: 1, day: 1, day_of_week: DayOfWeek::Thursday, hour: 0, minute: 0, second: 0 };
        /// Watchdog hardware timeout counter
        // Note: The maximum setting is 0xffffff which corresponds to 0xffffff / 2 ticks before triggering a watchdog
        //  reset (see errata RP2040-E1).
        const TIMEOUT_COUNTER: u32 = 0xFFFFFF;

        // Configure the watchdog so it resets everything incl ROSC/XOSC, and disable all interrupts except RTC
        PSM.wdsel().write_value(Wdsel(0x0001ffff));
        unsafe { (*NVIC::PTR).icer[0].write(u32::MAX) };
        unsafe { NVIC::unmask(Interrupt::RTC_IRQ) };

        // Configure clocks, so that everything is either disabled or uses XOSC as source
        CLOCKS.clk_adc_ctrl().modify(|w| w.set_enable(false));
        CLOCKS.clk_usb_ctrl().modify(|w| w.set_enable(false));
        CLOCKS.clk_peri_ctrl().modify(|w| w.set_enable(false));
        CLOCKS.clk_ref_ctrl().modify(|w| w.set_src(ClkRefCtrlSrc::XOSC_CLKSRC));
        CLOCKS.clk_sys_ctrl().modify(|w| w.set_src(ClkSysCtrlSrc::CLK_REF));
        CLOCKS.clk_rtc_ctrl().modify(|w| w.set_auxsrc(ClkRtcCtrlAuxsrc::XOSC_CLKSRC));
        // Note: 12 MHz / 256 => 46875 Hz, which is the RTC reference frequency
        CLOCKS.clk_rtc_div().modify(|w| w.set_int(256));

        // Disable PLLs
        PLL_SYS.pwr().modify(|w| {
            w.set_pd(true);
            w.set_vcopd(true);
        });
        PLL_USB.pwr().modify(|w| {
            w.set_pd(true);
            w.set_vcopd(true);
        });

        // Power down ring oscillator, XIP cache, and configure sleepdeep bits
        ROSC.ctrl().modify(|w| w.set_enable(Enable::DISABLE));
        XIP_CTRL.ctrl().modify(|w| w.set_power_down(true));
        scb.set_sleepdeep();

        // Create and setup RTC handle
        let mut rtc = Rtc::new(rtc);
        rtc.set_datetime(DEFAULT_DATETIME).expect("failed to set initial datetime");
        critical_section::with(|cs| {
            // Initialize shared RTC
            *Self::rtc().borrow_ref_mut(cs) = Some(rtc);
        });

        // Loop until the timeout is expired
        let steps = timeout.as_micros() / Watchdog::FEED_INTERVAL.as_micros();
        debug_println!("[info] sleeping for n intervals: {}", steps);
        for _ in 0..steps {
            // Feed watchdog manually as we don't have an owned high level instance
            // Note: This is sound, since `Self` only exists if the watchdog has been started already
            WATCHDOG.load().write_value(Load(TIMEOUT_COUNTER));
            debug_println!("[info] fed watchdog from lightsleep");

            // Schedule RTC alert
            critical_section::with(|cs| {
                // Borrow RTC
                let mut rtc_slot = Self::rtc().borrow_ref_mut(cs);
                let rtc = rtc_slot.as_mut().expect("no rtc setup");

                // Schedule next alert
                let now = rtc.now().expect("failed to get current time");
                let filter = DateTimeFilter::default().second((now.second + FEED_INTERVAL_SECS) % 60);
                rtc.schedule_alarm(filter);
            });

            // Wait for interrupt
            asm::wfi();
        }

        // Perform a graceful reset
        debug_println!("[info] performing graceful reset");
        self.reset();
    }

    /// Performs an immediate, graceful full reset via the watchdog
    pub fn reset(&self) -> ! {
        // Configure the watchdog so it resets everything, including ROSC/XOSC
        // Note: This is an additional safety measurement as we do some funny stuff with our clocks during sleep
        PSM.wdsel().write_value(Wdsel(0x0001ffff));

        // Perform reset via watchdog (this also resets the clocks)
        // Note: This should be sound as the existence of `self` implies the watchdog is running
        WATCHDOG.ctrl().write(|w| w.set_trigger(true));
        loop {
            // Wait for the reset
            // Note: Use nop to avoid rust-lang/rust#28728
            asm::nop();
        }
    }

    /// Shared slot for the RTC to provide it to the interrupt handler too
    fn rtc() -> &'static Mutex<RefCell<Option<Rtc<'static, RTC>>>> {
        static SHARED_RTC: Mutex<RefCell<Option<Rtc<'static, RTC>>>> = Mutex::new(RefCell::new(None));
        &SHARED_RTC
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

/// RTC interrupt handler
#[interrupt]
#[allow(non_snake_case)]
fn RTC_IRQ() {
    // The RTC alert fired
    debug_println!("[info] rtc interrupt fired");
    NVIC::unpend(Interrupt::RTC_IRQ);

    // Reset RTC if possible
    critical_section::with(|cs| {
        // Try to access the shared RTC peripheral
        let mut rtc_slot = WatchdogController::rtc().borrow_ref_mut(cs);
        if let Some(rtc) = rtc_slot.as_mut() {
            // Reset RTC interrupt
            rtc.clear_interrupt();
            rtc.disable_alarm();
        }
    });
}
