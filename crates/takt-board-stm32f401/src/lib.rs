//! STM32F401 als Takt-Board (12.3, 12.8).
//!
//! **Was hier steht.** Alles, was ein Register anfasst — Timer, Watchdog,
//! Takte —, und sonst nichts. Die Regeln daraus stehen eine Ebene hoeher
//! in `takt-rt-baremetal`, das dieses Crate nicht kennt: Es kennt nur
//! seine vier Traits, und die erfuellt dieses hier (plan/m5.md 2.2).
//!
//! **Benannt nach dem Chip, nicht nach dem Board.** Der F401 bestimmt,
//! was hier steht: Register, Peripherie, die 84-MHz-Grenze der PLL. Was
//! *ein Board* beitraegt, sind ein Dutzend Konstanten — Quarzfrequenz,
//! LED-Pin, der Versatz eines Bootloaders. Sie stehen als [`Board`]
//! beisammen, damit ein anderes F401-Board dieselbe Kiste mit drei
//! geaenderten Werten traegt. Geprueft ist bisher die WeAct Black Pill
//! (`STM32F401CCU6`, 25-MHz-Quarz), siehe [`Board::WEACT_BLACKPILL`].
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
//! ## Der Chip
//!
//! | | |
//! |---|---|
//! | Kern | Cortex-M4F, bis 84 MHz, f32 in Hardware (12.8: „32-Bit mit f32-FPU") |
//! | Flash | 256 KB (Variante CC), intern — **kein XIP**, also keine `xip_flash`-Regeln |
//! | RAM | 64 KB |
//! | Tick | TIM2, 32-bittig, vier Compare-Kanaele |
//! | Telemetrie | USART1 auf PA9/PA10 |

#![no_std]
#![allow(unsafe_code, reason = "Registerzugriff ueber die PAC; 9.5 fuehrt Treiber in der TCB")]

pub mod cycles;
pub mod guard;
pub mod led;
pub mod tick;
pub mod uart;

pub use guard::{Canary, Iwdg, WfiSleep, reboot};
pub use led::Led;
pub use takt_mcu_program::Generated;
pub use tick::{Tim2Tick, on_timer_interrupt};
pub use uart::{Telemetry, Usart1, telemetry};

/// Was ein einzelnes Board beitraegt.
///
/// Der Chip bestimmt fast alles; hier steht der Rest. Die Trennung macht
/// sichtbar, wie klein er ist — und sie erlaubt, ein anderes F401-Board
/// zu tragen, ohne eine Zeile Code zu aendern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// Frequenz des externen Quarzes in Hertz.
    ///
    /// F401-Boards gibt es mit 8 und mit 25 MHz; der Wert geht in den
    /// PLL-Teiler `pllm` ein. Ein falscher Wert bricht nichts — er
    /// verschiebt nur den ganzen Takt, und damit jede Zeitmessung.
    pub hse_hz: u32,
    /// Der Pin der Nutzer-LED, als `(Port, Nummer)`.
    ///
    /// Auf der Black Pill ist es `PC13`, und die LED ist aktiv low: Sie
    /// leuchtet, wenn der Pin auf null steht.
    pub led: (char, u8),
    /// Ist die LED aktiv low?
    pub led_active_low: bool,
}

impl Board {
    /// Die WeAct Black Pill mit STM32F401CCU6.
    ///
    /// Das Board, an dem M5 entwickelt wurde: 25-MHz-Quarz, LED an PC13
    /// (aktiv low), HID-Bootloader in den ersten 16 KiB des Flash.
    pub const WEACT_BLACKPILL: Board = Board { hse_hz: 25_000_000, led: ('C', 13), led_active_low: true };

    /// Ein F401-Board mit 8-MHz-Quarz, sonst wie die Black Pill.
    ///
    /// Die zweite gaengige Bestueckung. Sie steht hier, weil sie zeigt,
    /// was ein anderes Board wirklich kostet: eine Zeile.
    pub const HSE_8MHZ: Board = Board { hse_hz: 8_000_000, ..Board::WEACT_BLACKPILL };
}

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
    /// Die Quarzfrequenz des Boards passt nicht in die PLL.
    ///
    /// Sie muss ganzzahlig auf 1 MHz teilen (25, 8, 16 …). Ein krummer
    /// Wert wuerde den ganzen Takt verschieben, und mit ihm jede
    /// Zeitmessung — darum ein Fehler statt einer Naeherung.
    UnsupportedCrystal,
}

/// Setzt Takte und Tickquelle auf und gibt die Board-Teile zurueck.
///
/// `tick_ns` ist die Periode aus `system: tick`. Die Reihenfolge ist
/// nicht beliebig: Erst der Takt, dann der Timer — ein Timer, der vor der
/// PLL konfiguriert wird, rechnet mit der falschen Eingangsfrequenz.
pub fn init(
    board: Board,
    rcc: &stm32f4::stm32f401::RCC,
    flash: &stm32f4::stm32f401::FLASH,
    pwr: &stm32f4::stm32f401::PWR,
    tim2: &stm32f4::stm32f401::TIM2,
    tick_ns: i64,
) -> Result<Tim2Tick, InitError> {
    let counts = takt_board_support::counts_for(TIMER_HZ, tick_ns).map_err(InitError::Period)?;
    let psc = takt_board_support::prescaler_for(CORE_HZ, TIMER_HZ).ok_or(InitError::ClockNotReady)?;
    let pllm = takt_board_support::pll::divider_m(board.hse_hz).ok_or(InitError::UnsupportedCrystal)?;
    clocks(rcc, flash, pwr, pllm)?;
    start_tim2(rcc, tim2, psc, counts);
    Ok(Tim2Tick::new(TIMER_HZ, counts, CORE_HZ))
}

/// PLL auf 84 MHz aus dem Quarz des Boards.
///
/// Die Teiler kommen aus `takt-board-support::pll`, wo die Kette gegen
/// beide gaengigen Quarze geprueft ist. Hier steht nur, wie sie in die
/// Register kommen.
fn clocks(
    rcc: &stm32f4::stm32f401::RCC,
    flash: &stm32f4::stm32f401::FLASH,
    pwr: &stm32f4::stm32f401::PWR,
    pllm: u8,
) -> Result<(), InitError> {
    // **Erst der Spannungsregler.** Der F401 erreicht 84 MHz nur im
    // Regler-Bereich 1 (`VOS = 0b10`); im Standardbereich 2 liegt die
    // Grenze bei 60 MHz. Die Folge eines Versaeumnisses ist kein Fehler,
    // sondern Unzuverlaessigkeit: Der Kern laeuft, meist sogar lange,
    // und faellt bei Temperatur oder Spannungsschwankung aus.
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());
    // Der Dummy-Read ist kein Aberglaube: Zwischen dem Freigeben eines
    // Peripherietakts und dem ersten Registerzugriff liegen bis zu zwei
    // APB-Takte (ST-Errata „Delay after an RCC peripheral clock
    // enabling"). Ohne ihn kann der folgende Schreibvorgang verloren
    // gehen — und zwar je nach Optimierungsgrad mal ja, mal nein.
    let _ = rcc.apb1enr().read();
    pwr.cr().modify(|_, w| unsafe { w.vos().bits(0b10) });

    // Dann die Flash-Wartezyklen, bevor der Takt steigt — andersherum
    // liest der Kern Befehle, die noch nicht da sind. Bei 84 MHz und
    // 3,3 V sind zwei Zyklen gefordert (RM0368, Tabelle 6).
    //
    // **Und zurueckgelesen**, wie RM0368 es verlangt: Kommt der Wert
    // nicht an und der Takt steigt trotzdem, liest der Kern Befehle aus
    // einem Flash, der noch nicht geliefert hat — ein HardFault beim
    // ersten Zugriff, sporadisch und schwer zu finden.
    flash.acr().modify(|_, w| unsafe { w.latency().bits(2) });
    if !wait_for(|| flash.acr().read().latency().bits() == 2) {
        return Err(InitError::ClockNotReady);
    }

    // **Auf HSI zurueck und die PLL aus, bevor sie neu gesetzt wird.**
    // `PLLCFGR` ist schreibgeschuetzt, solange die PLL laeuft — und nach
    // einem Sprung aus dem HID-Bootloader laeuft sie: Der Bootloader
    // braucht sie fuer USB. Ohne diesen Schritt verpufft die
    // Konfiguration still, der Kern behaelt die Frequenz des Bootloaders,
    // und `CORE_HZ` luegt. Jede Zeitmessung waere dann falsch, ohne dass
    // irgendetwas auffiele — genau die Sorte Fehler, gegen die 7.1 die
    // gemessene Periode stellt.
    rcc.cfgr().modify(|_, w| unsafe { w.sw().bits(0b00) });
    if !wait_for(|| rcc.cfgr().read().sws().bits() == 0b00) {
        return Err(InitError::ClockNotReady);
    }
    rcc.cr().modify(|_, w| w.pllon().clear_bit());
    if !wait_for(|| rcc.cr().read().pllrdy().bit_is_clear()) {
        return Err(InitError::ClockNotReady);
    }

    rcc.cr().modify(|_, w| w.hseon().set_bit());
    if !wait_for(|| rcc.cr().read().hserdy().bit_is_set()) {
        return Err(InitError::ClockNotReady);
    }

    let pllp = takt_board_support::pll::divider_p().ok_or(InitError::UnsupportedCrystal)?;
    rcc.pllcfgr().write(|w| unsafe {
        w.pllsrc().set_bit(); // HSE als Quelle
        w.pllm().bits(pllm);
        w.plln().bits(takt_board_support::pll::divider_n());
        w.pllp().bits(pllp);
        w
    });
    rcc.cr().modify(|_, w| w.pllon().set_bit());
    if !wait_for(|| rcc.cr().read().pllrdy().bit_is_set()) {
        return Err(InitError::ClockNotReady);
    }

    // APB1 auf 42 MHz (Teiler 2): Der F401 erlaubt dort hoechstens 42.
    // Die Timer an APB1 bekommen dafuer den doppelten Takt, also wieder
    // 84 MHz — deshalb rechnet `start_tim2` mit `CORE_HZ`.
    //
    // APB2 bleibt bei Teiler 1, also 84 MHz. Der Reset-Wert waere
    // derselbe; er steht trotzdem hier, weil `Telemetry` den Takt als
    // `CORE_HZ` uebergeben bekommt und diese Zeile die Zusage dazu ist.
    // Ein Reset-Wert, auf den man sich stillschweigend verlaesst, ist
    // eine Annahme ohne Beleg.
    rcc.cfgr().modify(|_, w| unsafe {
        w.ppre1().bits(0b100);
        w.ppre2().bits(0b000)
    });
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
    let _ = rcc.apb1enr().read();

    tim2.psc().write(|w| unsafe { w.psc().bits(psc) });
    // Der Zaehler laeuft von 0 bis `arr`, das sind `arr + 1` Schritte.
    tim2.arr().write(|w| unsafe { w.bits(counts.saturating_sub(1)) });

    // Die neuen Werte uebernehmen, bevor der Zaehler laeuft.
    tim2.egr().write(|w| w.ug().set_bit());

    // **Und das dabei gesetzte Flag wieder loeschen.** `ug` erzeugt ein
    // Update-Ereignis — genau dasselbe, das der Ueberlauf spaeter
    // erzeugt —, und es setzt `UIF`. Bliebe es stehen, feuerte die ISR
    // sofort beim Freigeben des Interrupts, und der erste Tick kaeme vom
    // Aufsetzen statt vom Timer. Die Periodenmessung haette damit einen
    // sinnlosen ersten Wert, und die Tickzahl waere um eins verschoben.
    //
    // `write` mit Maske, weil `SR` ein `rc_w0`-Register ist: Nullen
    // loeschen, Einsen lassen stehen. Hier ist ohnehin alles frisch, aber
    // die Form bleibt dieselbe wie in der ISR.
    tim2.sr().write(|w| unsafe { w.bits(!1) });

    tim2.dier().modify(|_, w| w.uie().set_bit());
    tim2.cr1().modify(|_, w| w.cen().set_bit());
}
