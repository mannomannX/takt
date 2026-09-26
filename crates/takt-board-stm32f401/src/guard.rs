//! Watchdog, Schutzbereich und Schlaf (12.3, 12.4, 9.9).
//!
//! Die drei kleineren Board-Traits an einem Ort. Was sie verbindet: Jeder
//! ist eine *Verteidigung*, keine Funktion — sie tun im Normalbetrieb
//! nichts und werden nur sichtbar, wenn etwas schiefgeht.

use stm32f4::stm32f401::{IWDG, RCC};
use takt_rt_baremetal::{HardwareWatchdog, Sleep, StackGuard};

use crate::tick;

/// Der unabhaengige Watchdog (IWDG) des F401 (12.3).
///
/// Er laeuft auf dem internen 32-kHz-Oszillator und damit unabhaengig vom
/// Systemtakt — genau das will 12.3: Ein Watchdog, der an derselben Uhr
/// haengt wie das Programm, faellt mit ihr zusammen aus.
pub struct Iwdg {
    iwdg: IWDG,
    /// Kam der letzte Reset vom Watchdog?
    ///
    /// Beim Start aus dem RCC gelesen und gemerkt: Das Flag muss geloescht
    /// werden, damit der *naechste* Reset es wieder setzen kann, und
    /// danach ist die Information weg.
    was_watchdog: bool,
}

impl Iwdg {
    /// Liest die Reset-Ursache und startet den Watchdog.
    ///
    /// `period_ms` ist die Frist, nach der ohne `kick` zurueckgesetzt
    /// wird. Sie muss ueber der Tickperiode liegen, mit Reserve: Ein
    /// Watchdog, der bei einem einzelnen langen Tick zuschlaegt, macht aus
    /// einem `Runtime(Overrun)` einen Neustart — und verliert damit die
    /// Diagnose, die 7.3 vorsieht.
    pub fn start(iwdg: IWDG, rcc: &RCC, period_ms: u32) -> Iwdg {
        // 12.3: „Outputs auf `safe` bei Reset-Ursache Watchdog vor
        // Neustart." Die Ursache steht im RCC und ueberlebt den Reset.
        let was_watchdog = rcc.csr().read().wdgrstf().bit_is_set();
        rcc.csr().modify(|_, w| w.rmvf().set_bit());

        // Der IWDG will eine Entsperrsequenz, bevor er Register annimmt.
        iwdg.kr().write(|w| unsafe { w.key().bits(0x5555) });
        // Vorteiler 32: 32 kHz / 32 = 1 kHz, also ein Zaehlschritt je
        // Millisekunde. Damit ist `period_ms` unmittelbar der Zaehlerwert.
        iwdg.pr().write(|w| unsafe { w.pr().bits(0b011) });
        iwdg.rlr().write(|w| unsafe { w.rl().bits(period_ms.min(0x0FFF) as u16) });
        iwdg.kr().write(|w| unsafe { w.key().bits(0xCCCC) });

        Iwdg { iwdg, was_watchdog }
    }
}

impl HardwareWatchdog for Iwdg {
    fn kick(&mut self) {
        self.iwdg.kr().write(|w| unsafe { w.key().bits(0xAAAA) });
    }

    fn reset_was_watchdog(&self) -> bool {
        self.was_watchdog
    }
}

/// Der Schutzbereich unter dem Stack als Kanarienwort (12.3).
///
/// **Warum nicht die MPU.** Der F401 hat eine, und 12.3 nennt sie zuerst.
/// Eine MPU-Region ohne Zugriff meldet den Fehler frueher — beim Zugriff
/// statt am Tickende — und ist damit die bessere Bauform. Sie setzt aber
/// ein Linker-Skript voraus, das die Region ausrichtet (MPU-Regionen
/// muessen an ihrer Groesse ausgerichtet sein), und das Skript entsteht
/// erst mit dem Bring-up. Bis dahin traegt das Kanarienwort dieselbe
/// Aussage mit einer Tickperiode Verzoegerung.
///
/// Beide beweisen im Fehlerfall einen Fehler in der TCB, nicht im
/// Programm: „Das Programm kann per Konstruktion nicht ausserhalb seiner
/// Objekte schreiben" (12.3).
pub struct Canary {
    /// Adresse des Wortes unter dem Stack.
    cell: &'static core::sync::atomic::AtomicU32,
    /// Das Muster, das dort stehen muss.
    pattern: u32,
}

impl Canary {
    /// Ein Muster, das kein plausibler Nutzwert ist.
    ///
    /// Weder null noch `0xFFFF_FFFF`: Beide entstehen bei geloeschtem
    /// Speicher oder einem fehlgeschlagenen Lesevorgang, und ein
    /// Kanarienwort, das wie ein Unfall aussieht, kann seinen eigenen
    /// nicht anzeigen.
    pub const PATTERN: u32 = 0xC0DE_FACE;

    /// Legt das Muster ab.
    ///
    /// `cell` zeigt auf ein Wort, das das Linker-Skript unter den Stack
    /// legt. Es als `&'static` zu nehmen statt als rohe Adresse haelt das
    /// Crate frei von `unsafe` — wer es aufruft, muss die Zelle ohnehin
    /// irgendwo deklarieren, und dort steht sie typisiert.
    pub fn arm(cell: &'static core::sync::atomic::AtomicU32) -> Canary {
        cell.store(Canary::PATTERN, core::sync::atomic::Ordering::Relaxed);
        Canary { cell, pattern: Canary::PATTERN }
    }
}

impl StackGuard for Canary {
    fn intact(&self) -> bool {
        self.cell.load(core::sync::atomic::Ordering::Relaxed) == self.pattern
    }
}

/// Der Schlafmodus (9.9, 12.3).
///
/// `wfi` haelt den Kern an, bis ein Interrupt kommt. Der Timer laeuft
/// weiter — das ist die Bedingung aus Satz 9.9.1: Der Schlaf setzt die
/// Rechnung aus, nicht die Zeit. Die uebersprungenen Ticks holt die
/// Schleife als virtuelle Ticks nach.
pub struct WfiSleep;

impl Sleep for WfiSleep {
    fn sleep_until_event(&mut self) -> u64 {
        let before = tick::count();
        cortex_m::asm::wfi();
        tick::count().saturating_sub(before)
    }
}
