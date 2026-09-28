//! Watchdog und Schlaf (12.3, 12.4, 9.9); den Schutzbereich unter dem
//! Stack traegt die MPU (`mpu`).
//!
//! Die kleineren Board-Traits an einem Ort. Was sie verbindet: Jeder
//! ist eine *Verteidigung*, keine Funktion — sie tun im Normalbetrieb
//! nichts und werden nur sichtbar, wenn etwas schiefgeht.

use stm32f4::stm32f401::{IWDG, iwdg};
use takt_rt_baremetal::Sleep;
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
