//! Anfang und Ende eines Laufs auf dem Board (12.7): `previous_run`, und
//! was zwischen zwei Laeufen geschieht — Neustart und Tiefschlaf.
//!
//! **Das Wort des geordneten Endes steht im Backup-Register 3** der RTC:
//! Es ueberlebt Reset, Watchdog und Standby, das Ausschalten nicht. Ein
//! Lauf, der ueber `next_run` endet, schreibt es; jeder Lauf loescht es zu
//! Beginn. Kein Flash-Schreibvorgang je Start.
//!
//! **Tiefschlaf ist Standby** (RM0368 5.3.7): Kern und RAM sind aus, der
//! Wakeup-Timer der RTC weckt, und das Wecken ist ein Reset mit `SBF` in
//! `PWR_CSR`. Die RTC laeuft am Uhrenquarz (LSE, 32 768 Hz); schwingt er
//! nicht an, am internen RC (LSI, nominal 32 kHz, laut RM0368 17 bis
//! 47 kHz) — dann weckt das Board so ungenau, wie der RC geht.
//!
//! **Erst ein Reset, dann der Standby.** Den Watchdog haelt nur ein Reset
//! an, und im Standby zaehlte er weiter (RM0368 17.3). Darum merkt sich
//! [`deep_sleep`] die Weckzeit in Backup-Registern der RTC und setzt den
//! Chip zurueck; [`continue_deep_sleep`] schlaeft beim naechsten Start,
//! bevor der Watchdog wieder laeuft.
//!
//! **Laenger, als der Timer zaehlt** (2^17 s), schlaeft das Board in
//! Abschnitten. Den Rest tragen dieselben Register, die den Standby
//! ueberleben, und jeder Start nach einem Abschnitt schlaeft weiter.

use cortex_m::peripheral::SCB;
use stm32f4::stm32f401::{PWR, RCC, RTC, pwr, rcc, rtc};
use takt_board_support::platform::{ENDED_MARK, RUNNING, RtcWakeup, rtc_wakeup};

use crate::{CORE_HZ, cycles};

/// Kennung eines Tiefschlafs, der bevorsteht oder einen Rest hat
/// (Backup-Register 0); was zu schlafen bleibt, steht in Nanosekunden in
/// den Registern 1 und 2.
const PENDING: u32 = u32::from_le_bytes(*b"TIEF");

/// Was zu schlafen bleibt, wenn kein Zeitgeber weckt (`ON_WAKE`, `ON_START`).
const WITHOUT_TIMER: u64 = u64::MAX;

/// Das Backup-Register mit dem Wort des geordneten Endes (12.7).
const ENDED: usize = 3;

const LSE_HZ: u32 = 32_768;
const LSI_HZ: u32 = 32_000;

/// So lange darf der Uhrenquarz zum Anschwingen brauchen (AN2867: bis 2 s).
const LSE_START_CYCLES: u32 = 2 * CORE_HZ;

/// Schranke fuer das Warten auf ein Statusbit, das in Mikrosekunden kommt (4.1, von Hand).
const SETTLE: u32 = 1_000_000;

fn registers() -> (&'static rcc::RegisterBlock, &'static pwr::RegisterBlock, &'static rtc::RegisterBlock) {
    // Beim Start und am Ende eines Laufs fasst nur dieses Modul RCC_CSR,
    // RCC_BDCR, PWR und die RTC an.
    // SAFETY: die Register des RCC, siehe oben.
    let rcc = unsafe { &*RCC::ptr() };
    // SAFETY: die Register von PWR, siehe oben.
    let pwr = unsafe { &*PWR::ptr() };
    // SAFETY: die Register der RTC, siehe oben.
    let rtc = unsafe { &*RTC::ptr() };
    (rcc, pwr, rtc)
}

/// Die untere und die obere Haelfte von `x`, je ein Backup-Register.
fn halves(x: u64) -> (u32, u32) {
    let [a, b, c, d, e, f, g, h] = x.to_le_bytes();
    (u32::from_le_bytes([a, b, c, d]), u32::from_le_bytes([e, f, g, h]))
}

/// `sys/previous_run` (12.7) aus der Reset-Ursache in `RCC_CSR` und dem
/// Wort im Backup-Register; loescht beide, sodass jeder Lauf nur seinen
/// Vorgaenger sieht. Einmal beim Start zu rufen, nach [`continue_deep_sleep`].
pub fn previous_run() -> u32 {
    let (rcc, pwr, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    let csr = rcc.csr().read();
    let watchdog = csr.wdgrstf().bit_is_set() || csr.wwdgrstf().bit_is_set();
    let previous = takt_board_support::platform::previous_run(watchdog, rtc.bkpr(ENDED).read().bits());
    pwr.cr().modify(|_, w| w.csbf().set_bit());
    rcc.csr().modify(|_, w| w.rmvf().set_bit());
    backup(ENDED, RUNNING);
    previous
}

/// `next_run = NOW`: der Systemreset; der naechste Lauf meldet `ENDED`.
pub fn restart() -> ! {
    backup(ENDED, ENDED_MARK);
    SCB::sys_reset()
}

/// Schlaeft einen Tiefschlaf, der bevorsteht oder noch einen Rest hat, und
/// kehrt sonst zurueck. Als Erstes beim Start zu rufen: Der Watchdog ist
/// dann noch aus.
pub fn continue_deep_sleep() {
    let (rcc, pwr, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    if rtc.bkpr(0).read().bits() != PENDING {
        return;
    }
    let rest = u64::from(rtc.bkpr(1).read().bits()) | (u64::from(rtc.bkpr(2).read().bits()) << 32);
    pwr.cr().modify(|_, w| w.dbp().set_bit());
    if rest == WITHOUT_TIMER {
        disarm(rtc);
    } else {
        arm(rtc, rtc_wakeup(i64::try_from(rest).unwrap_or(i64::MAX), rtc_hz(rcc)));
    }
    standby(pwr)
}

/// `next_run = AFTER(delay)` (`Some`), `ON_WAKE` oder `ON_START` (`None`):
/// Standby, bis die Weckzeit vergangen ist; der naechste Lauf meldet
/// `ENDED`. Das Board hat keinen Input am WKUP-Pin, also weckt ohne
/// Zeitgeber nur ein Reset — `ON_WAKE` und `ON_START` sind hier dasselbe.
/// Geschlafen wird nach einem Reset, der den Watchdog anhaelt
/// ([`continue_deep_sleep`]).
pub fn deep_sleep(duration_ns: Option<i64>) -> ! {
    let rest = duration_ns.map_or(WITHOUT_TIMER, |d| u64::try_from(d).unwrap_or(0));
    let (low, high) = halves(rest);
    backup(ENDED, ENDED_MARK);
    backup(1, low);
    backup(2, high);
    backup(0, PENDING);
    SCB::sys_reset()
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
            // SAFETY: `0b01` waehlt in `RTCSEL` den LSE, der eben anlief.
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
    // SAFETY: `0b10` waehlt in `RTCSEL` den LSI.
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
    let prediv_s = u16::try_from((hz / 128).saturating_sub(1)).unwrap_or(u16::MAX).min(0x7FFF);
    rtc.prer().write(|w| {
        // SAFETY: 127 passt in die sieben Bit von `PREDIV_A` ...
        unsafe { w.prediv_a().bits(127) };
        // SAFETY: ... und `prediv_s` ist auf die 15 Bit von `PREDIV_S` begrenzt.
        unsafe { w.prediv_s().bits(prediv_s) }
    });
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
    // SAFETY: `WUT` ist 16 Bit breit und nimmt jeden Wert.
    rtc.wutr().write(|w| unsafe { w.wut().bits(plan.wutr) });
    // SAFETY: `rtc_wakeup` waehlt `0b000`, `0b100` oder `0b110`, Takte, die
    // `WUCKSEL` kennt.
    rtc.cr().modify(|_, w| unsafe { w.wucksel().bits(plan.wucksel) });
    rtc.isr().modify(|_, w| w.wutf().clear_bit());
    rtc.cr().modify(|_, w| w.wutie().set_bit().wute().set_bit());
    lock(rtc);
    let rest = u64::try_from(plan.rest_ns).unwrap_or(0);
    let (low, high) = halves(rest);
    // SAFETY: Die Backup-Register sind Speicher und nehmen jedes Wort.
    rtc.bkpr(0).write(|w| unsafe { w.bits(if rest > 0 { PENDING } else { 0 }) });
    // SAFETY: wie oben.
    rtc.bkpr(1).write(|w| unsafe { w.bits(low) });
    // SAFETY: wie oben.
    rtc.bkpr(2).write(|w| unsafe { w.bits(high) });
}

/// Kein Zeitgeber: der Wakeup-Timer aus, kein Rest.
fn disarm(rtc: &rtc::RegisterBlock) {
    unlock(rtc);
    rtc.cr().modify(|_, w| w.wute().clear_bit().wutie().clear_bit());
    rtc.isr().modify(|_, w| w.wutf().clear_bit());
    lock(rtc);
    // SAFETY: Die Backup-Register sind Speicher und nehmen jedes Wort.
    rtc.bkpr(0).write(|w| unsafe { w.bits(0) });
}

/// Liest das Backup-Register `n`; nur das Sprungbrett fragt seine Wuensche ab.
#[cfg(feature = "trampoline")]
pub(crate) fn backup_read(n: usize) -> u32 {
    let (rcc, _, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    rtc.bkpr(n).read().bits()
}

/// Schreibt das Backup-Register `n`; die Domaene ist nur dafuer offen.
pub(crate) fn backup(n: usize, value: u32) {
    let (rcc, pwr, rtc) = registers();
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    pwr.cr().modify(|_, w| w.dbp().set_bit());
    // SAFETY: Die Backup-Register sind Speicher und nehmen jedes Wort.
    rtc.bkpr(n).write(|w| unsafe { w.bits(value) });
    pwr.cr().modify(|_, w| w.dbp().clear_bit());
}

/// Hebt den Schreibschutz der RTC auf (RM0368 17.3.5).
fn unlock(rtc: &rtc::RegisterBlock) {
    // SAFETY: `0xCA`, dann `0x53` ist die Folge, die `WPR` erwartet.
    rtc.wpr().write(|w| unsafe { w.key().bits(0xCA) });
    // SAFETY: siehe oben.
    rtc.wpr().write(|w| unsafe { w.key().bits(0x53) });
}

/// Setzt den Schreibschutz wieder.
fn lock(rtc: &rtc::RegisterBlock) {
    // SAFETY: Jeder andere Wert als die Folge sperrt `WPR` wieder.
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
