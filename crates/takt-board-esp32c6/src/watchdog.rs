//! Der Watchdog der Schleife (12.3): der MWDT der Timergruppe 0.
//!
//! `esp-hal` haelt beim Start alle Watchdogs an. Den RTC-Watchdog behaelt
//! die Neuanmeldung des Hosts (FB-272); der MWDT setzt nach seiner Frist
//! das HP-System zurueck, und der naechste Start meldet `WATCHDOG` (12.7).
//! Im Tiefschlaf ist er mit dem HP-System aus.

use esp_hal::peripherals::TIMG0;
use esp_hal::time::Duration;
use esp_hal::timer::timg::{MwdtStage, Wdt};
use takt_rt_core::Watchdog;

/// Der MWDT0 als Watchdog der Schleife.
#[derive(Clone, Copy, Debug)]
pub struct Mwdt;

/// Gibt die Register des MWDT fuer einen Zugriff frei (ESP32-C6 TRM, `TIMG_WDTWPROTECT`).
const WKEY: u32 = 0x50D8_3AA1;

impl Mwdt {
    /// Startet den Watchdog mit der Frist `timeout_ns`, oder gibt einem
    /// laufenden eine neue, und bestaetigt ihn.
    pub fn arm(timeout_ns: i64) -> Mwdt {
        let mut wdt = Wdt::<TIMG0<'static>>::new();
        let us = u64::try_from(timeout_ns).unwrap_or(0).div_ceil(1_000).max(1);
        wdt.set_timeout(MwdtStage::Stage0, Duration::from_micros(us));
        wdt.enable();
        wdt.feed();
        Mwdt
    }

    /// Setzt die Frist zurueck; ohne gestarteten Watchdog wirkungslos.
    #[esp_hal::ram]
    pub fn feed() {
        let regs = TIMG0::regs();
        regs.wdtwprotect().write(|w| unsafe { w.wdt_wkey().bits(WKEY) });
        regs.wdtfeed().write(|w| unsafe { w.bits(1) });
        regs.wdtwprotect().write(|w| unsafe { w.wdt_wkey().bits(0) });
    }
}

impl Watchdog for Mwdt {
    fn kick(&mut self) {
        Mwdt::feed();
    }
}
