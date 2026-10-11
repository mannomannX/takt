//! Das `persist`-Journal im internen Flash (5.9, 12.3, FB-385).
//!
//! Sektor 2 und 3 (je 16 KiB, ab `0x0800_8000`) sind die zwei Slots des
//! Journals (`takt_rt_core::Journal`). Davor liegen der HID-Bootloader in
//! Sektor 0 und das Sprungbrett in Sektor 1, dahinter die Anwendung ab
//! Sektor 4 (`memory.x` des Bring-ups). Ein neues Abbild schreibt nur die
//! Sektoren der Anwendung und laesst beide Slots stehen.
//!
//! **Der Kern steht, solange das Flash arbeitet** (12.3, `nvm_blocking`).
//! Der F401 hat eine Bank, und jeder Lesezugriff auf das Flash — auch der
//! Befehlsabruf einer ISR — wartet, bis Loeschen oder Programmieren fertig
//! ist; eine Sektorloeschung dauert Hunderte Millisekunden. Die Runtime
//! schreibt darum nur, wo die Zeit bis zur naechsten Frist den Vorgang
//! deckt, oder unter `overrun = alert` (12.3, Pruefung 32). [`FlashNvm`]
//! arbeitet synchron: `begin_*` kehrt nach dem Vorgang zurueck, `poll`
//! meldet seinen Ausgang.
//!
//! Die Caches des ART sind aus (`clocks` in `lib.rs`): Ein Lesen nach dem
//! Loeschen oder Programmieren sieht das Flash selbst, keinen alten Inhalt.

use stm32f4::stm32f401::FLASH;
use stm32f4::stm32f401::flash::RegisterBlock;
use takt_board_support::clock::period_ns;
use takt_rt_core::{Nvm, NvmState};

use crate::{CORE_HZ, cycles};

/// Ein Slot ist ein Sektor.
pub const SLOT: u32 = 16 * 1024;

/// Slot 0 ist Sektor 2, Slot 1 Sektor 3.
const FIRST_SECTOR: u8 = 2;

/// Wo Slot 0 beginnt.
const BASE: u32 = 0x0800_8000;

/// Die Schluessel, die `FLASH_CR` entsperren (RM0368 3.5.1).
const KEYS: [u32; 2] = [0x4567_0123, 0xCDEF_89AB];

/// Die Fehlerbits in `FLASH_SR`: Schreibschutz, Ausrichtung, Parallelitaet,
/// Reihenfolge, Betrieb (RM0368 3.8.4).
const ERRORS: u32 = 0b1111_0010;

/// `FLASH_SR.EOP`; wie die Fehlerbits durch eine Eins geloescht.
const EOP: u32 = 1;

/// So oft fragt [`FlashNvm`] `BSY` hoechstens ab. Jede Abfrage kostet
/// mindestens einen Takt, die Schranke liegt also ueber zwei Sekunden —
/// ueber dem Vierfachen der laengsten Sektorloeschung im Datenblatt (x32).
const SPIN_MAX: u32 = 2 * CORE_HZ;

/// Zwei Slots im Flash.
pub struct FlashNvm {
    flash: FLASH,
    state: NvmState,
    /// Aus der Hardware-Konfiguration (8.10); `None` heisst unbekannt.
    blocking_ns: Option<i64>,
    /// Gemessene Hoechstdauern in Zyklen des Kerns (13.8).
    erase_max: u32,
    program_max: u32,
}

impl FlashNvm {
    /// Das Journal in Sektor 2 und 3.
    pub fn new(flash: FLASH) -> FlashNvm {
        FlashNvm { flash, state: NvmState::Idle, blocking_ns: None, erase_max: 0, program_max: 0 }
    }

    /// So lange haelt ein Vorgang den Kern hoechstens (`NVM_BLOCKING_NS`
    /// aus `takt build --hardware`); 0 heisst unbekannt.
    pub fn with_blocking_ns(mut self, ns: i64) -> FlashNvm {
        self.blocking_ns = (ns > 0).then_some(ns);
        self
    }

    /// Gemessene Hoechstdauern von Loeschen und Programmieren in
    /// Nanosekunden — die Zahlen fuer `nvm_erase_ns` und `nvm_program_ns`.
    pub fn measured_ns(&self) -> (i64, i64) {
        (period_ns(CORE_HZ, self.erase_max), period_ns(CORE_HZ, self.program_max))
    }

    /// Loescht beide Slots: ein Lauf, der wie der Interpreter ohne
    /// Speicher beginnen soll (13.8, Konformitaet).
    pub fn wipe(&mut self) -> bool {
        (0..2).all(|slot| erase(&self.flash, FIRST_SECTOR + slot))
    }

    fn at(slot: u8, offset: u32) -> u32 {
        BASE + u32::from(slot) * SLOT + offset
    }
}

impl Nvm for FlashNvm {
    fn slot_size(&self) -> u32 {
        SLOT
    }

    fn begin_erase(&mut self, slot: u8) -> bool {
        if slot > 1 {
            return false;
        }
        let start = cycles::now();
        let ok = erase(&self.flash, FIRST_SECTOR + slot);
        self.erase_max = self.erase_max.max(cycles::now().wrapping_sub(start));
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if slot > 1 || offset % 4 != 0 || offset.saturating_add(bytes.len() as u32) > SLOT {
            return false;
        }
        let start = cycles::now();
        let ok = program(&self.flash, FlashNvm::at(slot, offset), bytes);
        self.program_max = self.program_max.max(cycles::now().wrapping_sub(start));
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn poll(&mut self) -> NvmState {
        core::mem::take(&mut self.state)
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        if slot > 1 || offset.saturating_add(into.len() as u32) > SLOT {
            return false;
        }
        // SAFETY: Der Bereich liegt in Sektor 2 oder 3, im Flash, das immer
        // lesbar abgebildet ist. Beschrieben wird er nur ueber `&mut self`,
        // also nicht, solange die Scheibe lebt.
        let slot = unsafe { core::slice::from_raw_parts(FlashNvm::at(slot, offset) as *const u8, into.len()) };
        into.copy_from_slice(slot);
        true
    }

    fn blocking_ns(&self) -> Option<i64> {
        self.blocking_ns
    }
}

/// Loescht einen Sektor (RM0368 3.5.3).
fn erase(flash: &RegisterBlock, sector: u8) -> bool {
    operate(flash, |flash| {
        flash.cr().modify(|_, w| unsafe { w.psize().psize32().ser().set_bit().snb().bits(sector) });
        flash.cr().modify(|_, w| w.strt().set_bit());
        let done = settled(flash);
        flash.cr().modify(|_, w| w.ser().clear_bit());
        done
    })
}

/// Programmiert `bytes` wortweise ab `address`, mit `0xFF` aufgefuellt: Im
/// geloeschten Flash sind das die Bytes, die ohnehin dort stehen
/// (RM0368 3.5.4).
fn program(flash: &RegisterBlock, address: u32, bytes: &[u8]) -> bool {
    operate(flash, |flash| {
        flash.cr().modify(|_, w| w.psize().psize32().pg().set_bit());
        let mut done = true;
        for (i, part) in bytes.chunks(4).enumerate() {
            let mut word = [0xFF; 4];
            word[..part.len()].copy_from_slice(part);
            let at = (address + 4 * i as u32) as *mut u32;
            // SAFETY: Die Adresse liegt im Journal (Sektor 2 oder 3) und ist
            // wortausgerichtet; mit `PG` gesetzt programmiert der Schreibzugriff
            // das Flash, statt es zu veraendern wie RAM.
            unsafe { at.write_volatile(u32::from_le_bytes(word)) };
            done = settled(flash);
            if !done {
                break;
            }
        }
        flash.cr().modify(|_, w| w.pg().clear_bit());
        done
    })
}

/// Entsperrt `FLASH_CR`, loescht alte Fehler, fuehrt `f` aus und sperrt
/// wieder. Ein falscher Schluessel sperrte das Register bis zum naechsten
/// Reset; darum nur, wenn es gesperrt ist.
fn operate(flash: &RegisterBlock, f: impl FnOnce(&RegisterBlock) -> bool) -> bool {
    if !idle(flash) {
        return false;
    }
    if flash.cr().read().lock().bit_is_set() {
        for key in KEYS {
            flash.keyr().write(|w| w.key().set(key));
        }
    }
    flash.sr().write(|w| unsafe { w.bits(ERRORS | EOP) });
    let done = f(flash);
    flash.cr().modify(|_, w| w.lock().set_bit());
    done
}

/// Wartet, bis der laufende Vorgang endet; falsch bei einem Fehlerbit oder
/// wenn er nicht binnen [`SPIN_MAX`] endet.
fn settled(flash: &RegisterBlock) -> bool {
    idle(flash) && flash.sr().read().bits() & ERRORS == 0
}

/// Ist das Flash frei, oder wird es binnen [`SPIN_MAX`] Abfragen frei?
fn idle(flash: &RegisterBlock) -> bool {
    (0..SPIN_MAX).any(|_| flash.sr().read().bsy().bit_is_clear())
}
