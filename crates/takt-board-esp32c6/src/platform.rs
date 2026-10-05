//! Anfang und Ende eines Laufs auf dem Board (12.7): `previous_run`, und
//! was zwischen zwei Laeufen geschieht — Neustart und Tiefschlaf.
//!
//! **Der Neustart ist ein Software-Reset** des HP-Systems, wie `esp_restart`
//! in ESP-IDF: Der USB-Serial-JTAG bleibt angemeldet. Den Reset auf
//! RTC-Ebene behaelt der Host fuer die Neuanmeldung (FB-266); er setzt auch
//! das LP-System zurueck wie das Einschalten, und der naechste Lauf kennt
//! darum keinen vorigen.
//!
//! **Das Wort des geordneten Endes steht im RTC-RAM**, den die
//! Laufzeitumgebung nur beim Einschalten nullt: Es ueberlebt Software-Reset,
//! Watchdog und Tiefschlaf. Ein Lauf, der ueber `next_run` endet, schreibt
//! es; jeder Lauf loescht es zu Beginn.

use esp_hal::peripherals::LPWR;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::rtc_cntl::{SocResetReason, reset_reason};
use esp_hal::system::Cpu;
use esp_hal::time::{Duration, Instant};
use portable_atomic::{AtomicU32, Ordering};
use takt_board_support::platform::{ENDED_MARK, NO_WAKE_TIMER_US, RUNNING, deep_sleep_us};

/// Das Wort des geordneten Endes (12.7), was der vorige Lauf hinterliess.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static ENDED: AtomicU32 = AtomicU32::new(0);

/// `sys/previous_run` (12.7) aus der Reset-Ursache und dem Wort im RTC-RAM;
/// loescht das Wort, sodass jeder Lauf nur seinen Vorgaenger sieht. Einmal
/// beim Start zu rufen.
///
/// Der Watchdog der Runtime ist der MWDT. Den RTC-Watchdog nutzt nur die
/// Neuanmeldung des Hosts, als Reset des ganzen Chips wie der EN-Pin: Der
/// Lauf danach kennt keinen vorigen.
pub fn previous_run() -> u32 {
    let reason = reset_reason(Cpu::ProCpu);
    let watchdog = matches!(
        reason,
        Some(
            SocResetReason::CoreMwdt0
                | SocResetReason::CoreMwdt1
                | SocResetReason::CoreRtcWdt
                | SocResetReason::Cpu0Mwdt0
                | SocResetReason::Cpu0RtcWdt
                | SocResetReason::Cpu0Mwdt1
                | SocResetReason::SysSuperWdt
        )
    );
    let stored = match reason {
        Some(SocResetReason::SysRtcWdt) => RUNNING,
        _ => ENDED.load(Ordering::Relaxed),
    };
    ENDED.store(RUNNING, Ordering::Relaxed);
    takt_board_support::platform::previous_run(watchdog, stored)
}

/// `next_run = NOW`: der Software-Reset; der naechste Lauf meldet `ENDED`.
pub fn restart() -> ! {
    ENDED.store(ENDED_MARK, Ordering::Relaxed);
    esp_hal::system::software_reset()
}

/// `next_run = AFTER(delay)` (`Some`), `ON_WAKE` oder `ON_START` (`None`):
/// Tiefschlaf ohne RAM, bis die Weckzeit vergangen ist; der naechste Lauf
/// meldet `ENDED`. Das Board hat keinen Input, der aus dem Tiefschlaf weckt,
/// also weckt ohne Zeitgeber nur ein Reset — `ON_WAKE` und `ON_START` sind
/// hier dasselbe.
pub fn deep_sleep(duration_ns: Option<i64>) -> ! {
    ENDED.store(ENDED_MARK, Ordering::Relaxed);
    // SAFETY: Der Lauf ist zu Ende; ausser diesem Aufruf haelt niemand `LPWR`.
    let mut low = LowPower::new(unsafe { LPWR::steal() });
    let us = duration_ns.map_or(NO_WAKE_TIMER_US, deep_sleep_us);
    low.set_wakeup_deadline(Instant::now() + Duration::from_micros(us));
    low.sleep_deep(RtcSleepConfig::deep())
}
