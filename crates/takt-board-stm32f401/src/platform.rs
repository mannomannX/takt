//! Die Plattformschnittstelle des Boards (12.7): Reset-Ursache und
//! Tiefschlaf. Den Neustart macht [`crate::reboot`].
//!
//! **Tiefschlaf ist Standby** (RM0368 5.3.7): Kern und RAM sind aus, der
//! Wakeup-Timer der RTC weckt, und das Wecken ist ein Reset mit `SBF` in
//! `PWR_CSR`. Die RTC laeuft am Uhrenquarz (LSE, 32 768 Hz); schwingt er
//! nicht an, am internen RC (LSI, nominal 32 kHz, laut RM0368 17 bis
//! 47 kHz) — dann weckt das Board so ungenau, wie der RC geht.
//!
//! **Laenger, als der Timer zaehlt** (2^17 s), schlaeft das Board in
//! Abschnitten. Den Rest tragen Backup-Register der RTC, die den Standby
//! ueberleben, und [`continue_deep_sleep`] schlaeft ihn beim Start weiter,
//! bevor das Programm laeuft.

use stm32f4::stm32f401::{PWR, RCC, RTC, pwr, rcc, rtc};
use takt_board_support::platform::{RtcWakeup, boot_reason as reason, rtc_wakeup};

use crate::{CORE_HZ, cycles};

/// Kennung eines Tiefschlafs, der noch einen Rest hat (Backup-Register 0);
/// der Rest in Nanosekunden steht in den Registern 1 und 2.
const PENDING: u32 = u32::from_le_bytes(*b"TIEF");

const LSE_HZ: u32 = 32_768;
const LSI_HZ: u32 = 32_000;

/// So lange darf der Uhrenquarz zum Anschwingen brauchen (AN2867: bis 2 s).
const LSE_START_CYCLES: u32 = 2 * CORE_HZ;

/// Schranke fuer das Warten auf ein Statusbit, das in Mikrosekunden kommt (4.1, von Hand).
const SETTLE: u32 = 1_000_000;

fn registers() -> (&'static rcc::RegisterBlock, &'static pwr::RegisterBlock, &'static rtc::RegisterBlock) {
    // SAFETY: Beim Start und am Ende eines Laufs fasst nur dieses Modul
    // RCC_CSR, RCC_BDCR, PWR und die RTC an.
    unsafe { (&*RCC::ptr(), &*PWR::ptr(), &*RTC::ptr()) }
}

/// `sys/boot_reason` aus `PWR_CSR` und `RCC_CSR` (12.7); loescht die
/// Flaggen. Einmal beim Start zu rufen, nach [`continue_deep_sleep`].
pub fn boot_reason() -> i32 {
    let (rcc, pwr, _) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    let csr = rcc.csr().read();
    // Ein Watchdog zieht auch NRST, also vor dem Pin pruefen.
    let r = if pwr.csr().read().sbf().bit_is_set() {
        reason::DEEP_SLEEP_WAKE
    } else if csr.wdgrstf().bit_is_set() || csr.wwdgrstf().bit_is_set() {
        reason::WATCHDOG
    } else if csr.sftrstf().bit_is_set() {
        reason::SOFTWARE
    } else {
        reason::POWER_ON
    };
    pwr.cr().modify(|_, w| w.csbf().set_bit());
    rcc.csr().modify(|_, w| w.rmvf().set_bit());
    r
}

/// Schlaeft einen Tiefschlaf weiter, der noch einen Rest hat, und kehrt
/// sonst zurueck. Als Erstes beim Start zu rufen.
pub fn continue_deep_sleep() {
    let (rcc, pwr, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    if pwr.csr().read().sbf().bit_is_clear() || rtc.bkpr(0).read().bits() != PENDING {
        return;
    }
    let rest = i64::from(rtc.bkpr(1).read().bits()) | (i64::from(rtc.bkpr(2).read().bits()) << 32);
    pwr.cr().modify(|_, w| w.dbp().set_bit());
    arm(rtc, rtc_wakeup(rest, rtc_hz(rcc)));
    standby(pwr)
}

/// `reboot = DEEP_SLEEP_FOR(duration)` oder `DEEP_SLEEP` (`None`): Standby,
/// bis die Weckzeit vergangen ist; der naechste Start meldet
/// `DEEP_SLEEP_WAKE`. Das Board hat keinen Input am WKUP-Pin, also weckt
/// ohne Zeitgeber nur ein Reset.
pub fn deep_sleep(duration_ns: Option<i64>) -> ! {
    let (rcc, pwr, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    pwr.cr().modify(|_, w| w.dbp().set_bit());
    match duration_ns {
        Some(d) => arm(rtc, rtc_wakeup(d, rtc_hz(rcc))),
        None => disarm(rtc),
    }
    standby(pwr)
}

/// Der Takt der RTC; waehlt ihn beim ersten Mal. Die Backup-Domaene
/// ueberlebt Resets und Standby, eine gewaehlte Quelle bleibt also.
fn rtc_hz(rcc: &rcc::RegisterBlock) -> u32 {
    let bdcr = rcc.bdcr().read();
    if bdcr.rtcen().bit_is_set() {
        return if bdcr.rtcsel().bits() == 0b01 { LSE_HZ } else { LSI_HZ };
    }
    rcc.bdcr().modify(|_, w| w.lseon().set_bit());
    // Die Frist misst der Zykluszaehler; steht er, begrenzt die Zahl der
    // Durchlaeufe das Warten (jeder dauert mehr als einen Takt).
    let start = cycles::now();
    for _ in 0..LSE_START_CYCLES {
        if rcc.bdcr().read().lserdy().bit_is_set() {
            rcc.bdcr().modify(|_, w| unsafe { w.rtcsel().bits(0b01) }.rtcen().set_bit());
            return LSE_HZ;
        }
        if cycles::running() && cycles::now().wrapping_sub(start) >= LSE_START_CYCLES {
            break;
        }
    }
    rcc.bdcr().modify(|_, w| w.lseon().clear_bit());
    rcc.csr().modify(|_, w| w.lsion().set_bit());
    for _ in 0..SETTLE {
        if rcc.csr().read().lsirdy().bit_is_set() {
            break;
        }
    }
    rcc.bdcr().modify(|_, w| unsafe { w.rtcsel().bits(0b10) }.rtcen().set_bit());
    set_one_hertz(LSI_HZ);
    LSI_HZ
}

/// `ck_spre` = 1 Hz bei `hz`: `PREDIV_A` = 127, `PREDIV_S` = hz/128 - 1.
/// Nur fuer den LSI; die Werte nach dem Reset passen zum LSE.
fn set_one_hertz(hz: u32) {
    let (_, _, rtc) = registers();
    unlock(rtc);
    rtc.isr().modify(|_, w| w.init().set_bit());
    for _ in 0..SETTLE {
        if rtc.isr().read().initf().bit_is_set() {
            break;
        }
    }
    rtc.prer().write(|w| unsafe { w.prediv_a().bits(127).prediv_s().bits((hz / 128 - 1) as u16) });
    rtc.isr().modify(|_, w| w.init().clear_bit());
    lock(rtc);
}

/// Stellt den Wakeup-Timer auf einen Abschnitt und merkt den Rest.
fn arm(rtc: &rtc::RegisterBlock, plan: RtcWakeup) {
    unlock(rtc);
    rtc.cr().modify(|_, w| w.wute().clear_bit().wutie().clear_bit());
    for _ in 0..SETTLE {
        if rtc.isr().read().wutwf().bit_is_set() {
            break;
        }
    }
    rtc.wutr().write(|w| unsafe { w.wut().bits(plan.wutr) });
    rtc.cr().modify(|_, w| unsafe { w.wucksel().bits(plan.wucksel) });
    rtc.isr().modify(|_, w| w.wutf().clear_bit());
    rtc.cr().modify(|_, w| w.wutie().set_bit().wute().set_bit());
    lock(rtc);
    let rest = u64::try_from(plan.rest_ns).unwrap_or(0);
    rtc.bkpr(0).write(|w| unsafe { w.bits(if rest > 0 { PENDING } else { 0 }) });
    rtc.bkpr(1).write(|w| unsafe { w.bits(rest as u32) });
    rtc.bkpr(2).write(|w| unsafe { w.bits((rest >> 32) as u32) });
}

/// Kein Zeitgeber: der Wakeup-Timer aus, kein Rest.
fn disarm(rtc: &rtc::RegisterBlock) {
    unlock(rtc);
    rtc.cr().modify(|_, w| w.wute().clear_bit().wutie().clear_bit());
    rtc.isr().modify(|_, w| w.wutf().clear_bit());
    lock(rtc);
    rtc.bkpr(0).write(|w| unsafe { w.bits(0) });
}

fn unlock(rtc: &rtc::RegisterBlock) {
    rtc.wpr().write(|w| unsafe { w.key().bits(0xCA) });
    rtc.wpr().write(|w| unsafe { w.key().bits(0x53) });
}

fn lock(rtc: &rtc::RegisterBlock) {
    rtc.wpr().write(|w| unsafe { w.key().bits(0xFF) });
}

/// Standby (RM0368 5.3.7): `PDDS` und `SLEEPDEEP`, `WUF` geloescht, dann
/// `wfi`. Es geht nur per Reset weiter.
fn standby(pwr: &pwr::RegisterBlock) -> ! {
    pwr.cr().modify(|_, w| w.cwuf().set_bit().pdds().set_bit());
    // SAFETY: Nach dem Lauf haelt niemand mehr den SCB.
    let mut core = unsafe { cortex_m::Peripherals::steal() };
    core.SCB.set_sleepdeep();
    loop {
        cortex_m::asm::wfi();
    }
}
