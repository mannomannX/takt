//! WeAct Black Pill (STM32F401CCU6) als Takt-Board (12.3, 12.8).
//!
//! **Was hier steht.** Alles, was ein Register anfasst — Timer, Watchdog,
//! Takte —, und sonst nichts. Die Regeln daraus stehen eine Ebene hoeher
//! in `takt-rt-baremetal`, das dieses Crate nicht kennt: Es kennt nur
//! seine vier Traits, und die erfuellt dieses hier (plan/m5.md 2.2).
//!
//! **Dieses Crate gehoert zur TCB** (9.5): „Compiler und LLVM,
//! `libtaktm`, native Funktionen, Runtime, Treiber …, OS, Hardware." Die
//! Saetze aus 9.4 gelten fuer *Programme*, nicht fuer das, was hier
//! steht. Daraus folgt die Leitlinie: **so wenig wie moeglich.** Was
//! nicht zwingend ein Register braucht, gehoert nach oben — und mit der
//! Treiberstufe `port` (15, v1.2) wandert ein Teil davon spaeter nach
//! Takt selbst.
//!
//! **Warum PAC und nicht HAL.** `stm32f4xx-hal` braechte bequemere
//! Konfiguration, aber ein eigenes Zeit- und Taktmodell mit eigenen
//! Rundungsregeln. Takt hat bereits eines, und `tick_tolerance` (7.1)
//! misst Abweichungen in genau der Groessenordnung, die eine zweite
//! Rundung erzeugt. Die PAC liefert Registerzugriff ohne Meinung
//! (plan/m5.md 2.2).
//!
//! **`unsafe` in den Abhaengigkeiten.** `cortex-m` und die PAC brauchen
//! es fuer Registerzugriffe; das workspace-weite `forbid` (13.4) gilt fuer
//! eigenen Code. Wo dieses Crate selbst `unsafe` schreibt — beim Setzen
//! von Bitfeldern, die die PAC nicht typisiert —, steht es in einem
//! Ausdruck, nicht um einen Block: Je kleiner die Stelle, desto leichter
//! ist sie zu pruefen.
//!
//! ## Das Board
//!
//! | | |
//! |---|---|
//! | Kern | Cortex-M4F, 84 MHz, f32 in Hardware (12.8: „32-Bit mit f32-FPU") |
//! | Flash | 256 KB, intern — **kein XIP**, also keine `xip_flash`-Regeln |
//! | RAM | 64 KB |
//! | Tick | TIM2, 32-bittig, vier Compare-Kanaele |
//! | LED | PC13, aktiv low |
//! | Buttons | NRST (Reset), KEY an PA0 |

#![no_std]
#![allow(unsafe_code, reason = "Registerzugriff ueber die PAC; 9.5 fuehrt Treiber in der TCB")]

pub mod cycles;
pub mod guard;
pub mod tick;

pub use guard::{Canary, Iwdg, WfiSleep, reboot};
pub use tick::{Tim2Tick, on_timer_interrupt};

/// Die Taktfrequenz des Kerns nach [`init`], in Hertz.
///
/// 84 MHz ist das Maximum des F401 und der Wert, mit dem die
/// Kalibrierung rechnet (13.8). Er steht als Konstante, weil jede
/// Zyklenmessung ihn braucht und ein abweichender Wert die ganze
/// Messreihe still verschoebe.
pub const CORE_HZ: u32 = 84_000_000;

/// Die Frequenz, mit der TIM2 nach [`init`] zaehlt.
///
/// 1 MHz: ein Zaehlschritt je Mikrosekunde. Das macht die Rechnung von
/// Timer-Schritten in Zeit exakt — ein krummer Teiler waere eine
/// Rundungsquelle in der Groessenordnung, die `tick_tolerance` misst.
pub const TIMER_HZ: u32 = 1_000_000;

/// Fehler beim Aufsetzen des Boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    /// Die gewuenschte Tickperiode passt nicht in den Timer.
    ///
    /// TIM2 ist 32-bittig, also sind bei 1 MHz rund 71 Minuten das
    /// Maximum; kuerzer als eine Mikrosekunde geht ebenfalls nicht. Der
    /// Grund steht dabei, weil er den Nutzer in verschiedene Richtungen
    /// schickt: zu kurz heisst schnellerer Timer, zu lang heisst groesserer
    /// Prescaler.
    Period(takt_board_support::PeriodError),
    /// Der externe Quarz kam nicht hoch.
    ///
    /// Der interne Oszillator waere ein Ausweg, aber kein guter: Er
    /// weicht um Prozente ab, und `tick_tolerance` (7.1) hat den Default
    /// `2 pct`. Ein Board, das seinen Quarz nicht startet, soll das
    /// melden statt still ungenau zu laufen.
    ClockNotReady,
}

/// Setzt Takte und Tickquelle auf und gibt die Board-Teile zurueck.
///
/// `tick_ns` ist die Periode aus `system: tick`. Die Reihenfolge ist
/// nicht beliebig: Erst der Takt, dann der Timer — ein Timer, der vor der
/// PLL konfiguriert wird, rechnet mit der falschen Eingangsfrequenz.
pub fn init(
    rcc: &stm32f4::stm32f401::RCC,
    flash: &stm32f4::stm32f401::FLASH,
    tim2: &stm32f4::stm32f401::TIM2,
    tick_ns: i64,
) -> Result<Tim2Tick, InitError> {
    let counts = takt_board_support::counts_for(TIMER_HZ, tick_ns).map_err(InitError::Period)?;
    let psc = takt_board_support::prescaler_for(CORE_HZ, TIMER_HZ).ok_or(InitError::ClockNotReady)?;
    clocks(rcc, flash)?;
    start_tim2(rcc, tim2, psc, counts);
    Ok(Tim2Tick::new(TIMER_HZ, counts))
}

/// PLL auf 84 MHz aus dem 25-MHz-Quarz des Boards.
///
/// Die Kette ist `25 MHz / 25 * 336 / 4 = 84 MHz`. Der Umweg ueber 336
/// ist nicht Zierde: Die PLL verlangt einen VCO zwischen 100 und 432 MHz,
/// und 336 ist der Wert, der mit Teiler 4 genau 84 ergibt.
fn clocks(rcc: &stm32f4::stm32f401::RCC, flash: &stm32f4::stm32f401::FLASH) -> Result<(), InitError> {
    // Flash braucht Wartezyklen, bevor der Takt steigt — andersherum
    // liest der Kern Befehle, die noch nicht da sind.
    flash.acr().modify(|_, w| unsafe { w.latency().bits(2) });

    rcc.cr().modify(|_, w| w.hseon().set_bit());
    if !wait_for(|| rcc.cr().read().hserdy().bit_is_set()) {
        return Err(InitError::ClockNotReady);
    }

    rcc.pllcfgr().write(|w| unsafe {
        w.pllsrc().set_bit(); // HSE als Quelle
        w.pllm().bits(25);
        w.plln().bits(336);
        w.pllp().bits(0b01); // Teiler 4
        w
    });
    rcc.cr().modify(|_, w| w.pllon().set_bit());
    if !wait_for(|| rcc.cr().read().pllrdy().bit_is_set()) {
        return Err(InitError::ClockNotReady);
    }

    // APB1 auf 42 MHz (Teiler 2): Der F401 erlaubt dort hoechstens 42.
    // Die Timer an APB1 bekommen dafuer den doppelten Takt, also wieder
    // 84 MHz — deshalb rechnet `start_tim2` mit `CORE_HZ`.
    rcc.cfgr().modify(|_, w| unsafe { w.ppre1().bits(0b100) });
    rcc.cfgr().modify(|_, w| unsafe { w.sw().bits(0b10) });
    if !wait_for(|| rcc.cfgr().read().sws().bits() == 0b10) {
        return Err(InitError::ClockNotReady);
    }
    Ok(())
}

/// Wartet beschraenkt auf eine Bedingung.
///
/// **Eine Schleife mit Schranke, keine mit `while`.** Ein Board, dessen
/// Quarz nicht anschwingt, soll melden statt zu haengen — das ist
/// dieselbe Regel, die 4.1 fuer die Sprache aufstellt, hier von Hand
/// eingehalten, weil die TCB sie nicht geschenkt bekommt.
fn wait_for(mut ready: impl FnMut() -> bool) -> bool {
    // 100 000 Durchlaeufe sind bei 16 MHz Startakt einige Millisekunden —
    // ein Quarz braucht typisch unter einer.
    (0..100_000).any(|_| ready())
}

/// TIM2 als Tickquelle: Aufwaertszaehler mit Update-Interrupt.
fn start_tim2(rcc: &stm32f4::stm32f401::RCC, tim2: &stm32f4::stm32f401::TIM2, psc: u16, counts: u32) {
    rcc.apb1enr().modify(|_, w| w.tim2en().set_bit());

    tim2.psc().write(|w| unsafe { w.psc().bits(psc) });
    // Der Zaehler laeuft von 0 bis `arr`, das sind `arr + 1` Schritte.
    tim2.arr().write(|w| unsafe { w.bits(counts.saturating_sub(1)) });
    // Die neuen Werte uebernehmen, bevor der Zaehler laeuft.
    tim2.egr().write(|w| w.ug().set_bit());
    tim2.dier().modify(|_, w| w.uie().set_bit());
    tim2.cr1().modify(|_, w| w.cen().set_bit());
}
