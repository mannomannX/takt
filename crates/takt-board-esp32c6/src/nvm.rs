//! Das `persist`-Journal im SPI-Flash (5.9, 12.3).
//!
//! Zwei Sektoren zu 4 KiB sind die zwei Slots des Journals
//! (`takt_rt_core::Journal`); sie liegen in der `nvs`-Partition des
//! ESP-IDF-Schemas, das `probe-rs` mit dem Bootloader flasht — sonst leer,
//! und ein neues Abbild laesst sie unberuehrt.
//!
//! **Blockierend, und das ist der Befund fuer Schritt 7.** `esp-storage`
//! ruft die ROM-Routinen mit abgeschaltetem Cache und gesperrten
//! Interrupts; eine Sektorloeschung dauert zweistellige Millisekunden, und
//! solange steht der Tick (12.3, `xip_flash`: „`persist` ausserhalb des
//! Ticks"). Die Schleife zaehlt die verlorenen Ticks; `min_interval`
//! haelt den Preis selten.

use esp_hal::peripherals::FLASH;
use esp_storage::FlashStorage;
use takt_rt_core::{Nvm, NvmState};

/// Ein Slot ist ein Sektor.
pub const SLOT: u32 = FlashStorage::SECTOR_SIZE;

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

    /// Loescht beide Slots: ein Lauf, der wie der Interpreter ohne
    /// Speicher beginnen soll (13.8, Konformitaet).
    pub fn wipe(&mut self) -> bool {
        self.storage.erase(self.base, self.base + 2 * SLOT).is_ok()
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
        self.state = if self.storage.erase(from, from + SLOT).is_ok() { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if slot > 1 || offset % 4 != 0 || offset.saturating_add(bytes.len() as u32) > SLOT {
            return false;
        }
        // Wortweise, mit `0xFF` aufgefuellt: Im geloeschten Flash sind das
        // die Bytes, die ohnehin dort stehen.
        let mut at = self.at(slot, offset);
        let mut ok = true;
        for chunk in bytes.chunks(64) {
            let mut word = [0xFFu8; 64];
            word[..chunk.len()].copy_from_slice(chunk);
            let n = chunk.len().next_multiple_of(4);
            ok &= self.storage.write_nor(at, &word[..n]).is_ok();
            at += n as u32;
        }
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn poll(&mut self) -> NvmState {
        core::mem::take(&mut self.state)
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        slot < 2
            && offset.saturating_add(into.len() as u32) <= SLOT
            && self.storage.read_nor(self.at(slot, offset), into).is_ok()
    }
}
