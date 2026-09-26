//! Die Plattformschnittstelle des Boards (12.7): Reset-Ursache, Zaehler der
//! Starts, Neustart und Tiefschlaf.
//!
//! **Der Neustart ist ein Software-Reset** des HP-Systems, wie `esp_restart`
//! in ESP-IDF: Der naechste Start liest `CoreSw` und meldet `SOFTWARE`, und
//! der USB-Serial-JTAG bleibt angemeldet. Den Reset auf RTC-Ebene behaelt
//! der Host fuer die Neuanmeldung (FB-266); er setzt auch das LP-System
//! zurueck wie das Einschalten und heisst darum `POWER_ON`.
//!
//! **Der Zaehler steht im RTC-RAM**, den die Laufzeitumgebung nur beim
//! Einschalten nullt: Er ueberlebt Software-Reset, Watchdog und Tiefschlaf,
//! wie 12.7 es fuer `reset_count` verlangt.

use esp_hal::peripherals::LPWR;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::rtc_cntl::{SocResetReason, reset_reason};
use esp_hal::system::Cpu;
use esp_hal::time::{Duration, Instant};
use portable_atomic::{AtomicU32, Ordering};
use takt_board_support::platform::{ORDERLY_END, boot_reason as reason, deep_sleep_us};

/// Ohne Zeitgeber verlangt `esp-hal` trotzdem eine Weckquelle; der
/// Zeitgeber steht dann auf dreissig Jahren. Der Vergleicher fasst 48 Bit
/// Takte des langsamen RTC-Takts, rund 65 Jahre.
const WITHOUT_TIMER_US: u64 = 30 * 365 * 86_400 * 1_000_000;

/// `reset_count` (12.7), was der vorige Lauf hinterliess.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static COUNT: AtomicU32 = AtomicU32::new(0);

/// `sys/boot_reason` aus der Reset-Ursache (12.7).
///
/// Der Watchdog der Runtime ist der MWDT. Den RTC-Watchdog nutzt nur die
/// Neuanmeldung des Hosts, als Reset des ganzen Chips wie der EN-Pin —
/// also `POWER_ON`.
pub fn boot_reason() -> i32 {
    match reset_reason(Cpu::ProCpu) {
        Some(SocResetReason::CoreDeepSleep) => reason::DEEP_SLEEP_WAKE,
        Some(SocResetReason::CoreSw | SocResetReason::Cpu0Sw) => reason::SOFTWARE,
        Some(SocResetReason::SysRtcWdt) => reason::POWER_ON,
        Some(
            SocResetReason::CoreMwdt0
            | SocResetReason::CoreMwdt1
            | SocResetReason::CoreRtcWdt
            | SocResetReason::Cpu0Mwdt0
            | SocResetReason::Cpu0RtcWdt
            | SocResetReason::Cpu0Mwdt1
            | SocResetReason::SysSuperWdt,
        ) => reason::WATCHDOG,
        _ => reason::POWER_ON,
    }
}

/// `sys/reset_count` zu `boot_reason` (12.7); zaehlt diesen Start. Einmal
/// beim Start zu rufen.
pub fn reset_count(boot_reason: i32) -> u32 {
    let count = takt_board_support::platform::reset_count(boot_reason, COUNT.load(Ordering::Relaxed));
    COUNT.store(count, Ordering::Relaxed);
    count
}

/// `reboot = RESTART`: der Software-Reset; der naechste Start meldet
/// `SOFTWARE` und zaehlt von vorn.
pub fn restart() -> ! {
    COUNT.store(ORDERLY_END, Ordering::Relaxed);
    esp_hal::system::software_reset()
}

/// `reboot = DEEP_SLEEP_FOR(duration)` oder `DEEP_SLEEP` (`None`): Tiefschlaf
/// ohne RAM, bis die Weckzeit vergangen ist; der naechste Start meldet
/// `DEEP_SLEEP_WAKE`. Das Board hat keinen Input, der aus dem Tiefschlaf
/// weckt, also weckt ohne Zeitgeber nur ein Reset.
pub fn deep_sleep(duration_ns: Option<i64>) -> ! {
    COUNT.store(ORDERLY_END, Ordering::Relaxed);
    // SAFETY: Der Lauf ist zu Ende; ausser diesem Aufruf haelt niemand `LPWR`.
    let mut low = LowPower::new(unsafe { LPWR::steal() });
    let us = duration_ns.map_or(WITHOUT_TIMER_US, deep_sleep_us);
    low.set_wakeup_deadline(Instant::now() + Duration::from_micros(us));
    low.sleep_deep(RtcSleepConfig::deep())
}
