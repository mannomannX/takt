//! Watchdog, Schutzbereich und Schlaf (12.3, 12.4, 9.9).
//!
//! Die drei kleineren Board-Traits an einem Ort. Was sie verbindet: Jeder
//! ist eine *Verteidigung*, keine Funktion — sie tun im Normalbetrieb
//! nichts und werden nur sichtbar, wenn etwas schiefgeht.

use stm32f4::stm32f401::{IWDG, iwdg};
use takt_rt_baremetal::{Sleep, StackGuard};
use takt_rt_core::Watchdog;

use crate::tick;

/// Der unabhaengige Watchdog (IWDG) des F401 (12.3).
///
/// Er laeuft am internen RC-Oszillator (LSI) und damit unabhaengig vom
/// Systemtakt — genau das will 12.3: Ein Watchdog, der an derselben Uhr
/// haengt wie das Programm, faellt mit ihr zusammen aus. Nach seiner Frist
/// setzt er den Chip zurueck, und der naechste Start meldet `WATCHDOG`
/// (`platform::boot_reason`).
///
/// **Einmal gestartet, haelt ihn nur ein Reset an** (RM0368 17.3). Er
/// zaehlt im Standby weiter und im Bootloader, der ihn nicht bedient; darum
/// gehen Tiefschlaf und Rueckgabe an den Host ueber einen Reset
/// ([`crate::platform::deep_sleep`], [`crate::bootloader::request`]).
#[derive(Clone, Copy, Debug)]
pub struct Iwdg;

/// Der LSI laut Datenblatt zwischen 17 und 47 kHz; die Frist rechnet mit
/// dem schnellsten, damit der Watchdog nie vor ihr zuschlaegt.
const LSI_MAX_HZ: u32 = 47_000;

/// Schranke fuer das Warten, bis der IWDG Vorteiler und Nachladewert
/// uebernommen hat — hoechstens fuenf Takte des LSI (4.1, von Hand).
const SETTLE: u32 = 100_000;

impl Iwdg {
    /// Startet den Watchdog mit der Frist `timeout_ns`, oder gibt einem
    /// laufenden eine neue, und bestaetigt ihn.
    pub fn arm(timeout_ns: i64) -> Iwdg {
        let (pr, rlr) = takt_board_support::watchdog::iwdg(timeout_ns, LSI_MAX_HZ);
        let iwdg = registers();
        iwdg.kr().write(|w| unsafe { w.key().bits(0xCCCC) });
        iwdg.kr().write(|w| unsafe { w.key().bits(0x5555) });
        settle(iwdg);
        iwdg.pr().write(|w| unsafe { w.pr().bits(pr) });
        iwdg.rlr().write(|w| unsafe { w.rl().bits(rlr) });
        settle(iwdg);
        Iwdg::feed();
        Iwdg
    }

    /// Setzt die Frist zurueck; ohne gestarteten Watchdog wirkungslos.
    pub fn feed() {
        registers().kr().write(|w| unsafe { w.key().bits(0xAAAA) });
    }
}

impl Watchdog for Iwdg {
    fn kick(&mut self) {
        Iwdg::feed();
    }
}

fn registers() -> &'static iwdg::RegisterBlock {
    // SAFETY: Der Watchdog hat keinen Zustand ausser seinen Registern, und
    // jeder Zugriff steht fuer sich.
    unsafe { &*IWDG::ptr() }
}

/// Wartet, bis eine Aenderung von Vorteiler und Nachladewert uebernommen
/// ist (`PVU`, `RVU`); erst dann nimmt der IWDG die naechste an.
fn settle(iwdg: &iwdg::RegisterBlock) {
    for _ in 0..SETTLE {
        if iwdg.sr().read().bits() == 0 {
            break;
        }
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
