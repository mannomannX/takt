//! Das `persist`-Journal im SPI-Flash (5.9, 12.3).
//!
//! Zwei Sektoren zu 4 KiB sind die zwei Slots des Journals
//! (`takt_rt_core::Journal`); sie liegen in der `nvs`-Partition des
//! ESP-IDF-Schemas, das `probe-rs` mit dem Bootloader flasht — sonst leer,
//! und ein neues Abbild laesst sie unberuehrt.
//!
//! **Der Tick laeuft weiter** (12.3, `xip_flash`: „`persist` ausserhalb
//! des Ticks"). Eine Sektorloeschung dauert zweistellige Millisekunden
//! mit abgeschaltetem Cache; zwei Dinge halten den Tick trotzdem: Sein
//! Code liegt im RAM (`#[ram]` in `tick.rs`, `lib.rs`, `guard.rs`), und
//! `esp-storage` laeuft ohne `critical-section`, sperrt die Interrupts
//! also nicht (`Cargo.toml`). Der Alarm kommt damit an, und die Schleife
//! holt die Ticks nach — der Zaehler kommt aus dem SYSTIMER, nicht aus
//! der ISR.
//!
//! Der Preis ist die Nebenlaeufigkeit, die sonst die kritische Sektion
//! deckte: Waehrend das Journal schreibt, darf kein zweiter Flash-Zugriff
//! dazwischen. [`FlashNvm`] haelt dafuer ein Flag, und sonst greift
//! niemand auf das Flash zu — der Code laeuft ueber den Cache, nicht
//! ueber diesen Treiber.

use core::sync::atomic::{AtomicBool, Ordering};

use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;
use takt_rt_core::{Nvm, NvmState};

/// Ein Slot ist ein Sektor.
pub const SLOT: u32 = FlashStorage::SECTOR_SIZE;

/// Laeuft gerade ein Flash-Zugriff? Ohne `critical-section` serialisiert
/// nichts sonst; ein zweiter Zugriff waehrend eines laufenden verletzt den
/// ROM-Treiber.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Zwei Slots im Flash.
pub struct FlashNvm {
    storage: FlashStorage<'static>,
    base: u32,
    state: NvmState,
}

impl FlashNvm {
    /// Journal ab `base` (sektoralig), Slot 0 dort, Slot 1 einen Sektor dahinter.
    pub fn new(flash: FLASH<'static>, base: u32) -> FlashNvm {
        FlashNvm { storage: FlashStorage::new(flash), base, state: NvmState::Idle }
    }

    /// Fuehrt einen Flash-Zugriff aus, wenn keiner laeuft; sonst `None`.
    fn exclusive<R>(f: impl FnOnce() -> R) -> Option<R> {
        if BUSY.swap(true, Ordering::Acquire) {
            return None;
        }
        let out = f();
        BUSY.store(false, Ordering::Release);
        Some(out)
    }

    /// Loescht beide Slots: ein Lauf, der wie der Interpreter ohne
    /// Speicher beginnen soll (13.8, Konformitaet).
    pub fn wipe(&mut self) -> bool {
        let (storage, base) = (&mut self.storage, self.base);
        Self::exclusive(|| storage.erase(base, base + 2 * SLOT).is_ok()).unwrap_or(false)
    }

    fn at(&self, slot: u8, offset: u32) -> u32 {
        self.base + u32::from(slot) * SLOT + offset
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
        let from = self.at(slot, 0);
        let storage = &mut self.storage;
        let Some(ok) = Self::exclusive(|| storage.erase(from, from + SLOT).is_ok()) else {
            return false;
        };
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if slot > 1 || offset % 4 != 0 || offset.saturating_add(bytes.len() as u32) > SLOT {
            return false;
        }
        // Wortweise, mit `0xFF` aufgefuellt: Im geloeschten Flash sind das
        // die Bytes, die ohnehin dort stehen.
        let mut at = self.at(slot, offset);
        let storage = &mut self.storage;
        let Some(ok) = Self::exclusive(|| {
            let mut ok = true;
            for chunk in bytes.chunks(64) {
                let mut word = [0xFFu8; 64];
                word[..chunk.len()].copy_from_slice(chunk);
                let n = chunk.len().next_multiple_of(4);
                ok &= storage.write_nor(at, &word[..n]).is_ok();
                at += n as u32;
            }
            ok
        }) else {
            return false;
        };
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
        let at = self.at(slot, offset);
        let storage = &mut self.storage;
        Self::exclusive(|| storage.read_nor(at, into).is_ok()).unwrap_or(false)
    }
}
