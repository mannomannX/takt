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
use esp_hal::timer::systimer::{SystemTimer, Unit};
use esp_storage::FlashStorage;
use takt_board_support::clock::period_ns;
use takt_rt_core::{Nvm, NvmState};

use crate::TIMER_HZ;

/// Ein Slot ist ein Sektor.
pub const SLOT: u32 = FlashStorage::SECTOR_SIZE;

/// Ein Stueck des Journals im RAM, wortausgerichtet: Ein Slice ohne diese
/// Ausrichtung kopiert `esp-storage` ueber einen Sektorpuffer von 4 KiB auf
/// dem Stack (`read_nor`, `write_nor`), und der laege unter jedem Tick, der
/// das Journal schreibt (12.3, FB-456).
#[repr(C, align(4))]
struct Chunk([u8; Chunk::BYTES]);

impl Chunk {
    const BYTES: usize = 64;
}

/// Laeuft gerade ein Flash-Zugriff? Ohne `critical-section` serialisiert
/// nichts sonst; ein zweiter Zugriff waehrend eines laufenden verletzt den
/// ROM-Treiber.
static BUSY: AtomicBool = AtomicBool::new(false);

/// Zwei Slots im Flash.
pub struct FlashNvm {
    storage: FlashStorage<'static>,
    base: u32,
    state: NvmState,
    /// Aus der Hardware-Konfiguration (8.10); `None` heisst unbekannt.
    blocking_ns: Option<i64>,
    /// Gemessene Hoechstdauern in SYSTIMER-Schritten (13.8).
    erase_max: u64,
    program_max: u64,
}

impl FlashNvm {
    /// Journal ab `base` (sektoralig), Slot 0 dort, Slot 1 einen Sektor dahinter.
    pub fn new(flash: FLASH<'static>, base: u32) -> FlashNvm {
        FlashNvm {
            storage: FlashStorage::new(flash),
            base,
            state: NvmState::Idle,
            blocking_ns: None,
            erase_max: 0,
            program_max: 0,
        }
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
        let ns = |counts: u64| period_ns(TIMER_HZ, u32::try_from(counts).unwrap_or(u32::MAX));
        (ns(self.erase_max), ns(self.program_max))
    }

    /// Fuehrt einen Flash-Zugriff aus, wenn keiner laeuft; sonst `None`.
    /// Liefert das Ergebnis und die Dauer in SYSTIMER-Schritten.
    fn exclusive<R>(f: impl FnOnce() -> R) -> Option<(R, u64)> {
        if BUSY.swap(true, Ordering::SeqCst) {
            return None;
        }
        let start = SystemTimer::unit_value(Unit::Unit0);
        let out = f();
        let took = SystemTimer::unit_value(Unit::Unit0).wrapping_sub(start);
        BUSY.store(false, Ordering::SeqCst);
        Some((out, took))
    }

    /// Loescht beide Slots: ein Lauf, der wie der Interpreter ohne
    /// Speicher beginnen soll (13.8, Konformitaet).
    pub fn wipe(&mut self) -> bool {
        let (storage, base) = (&mut self.storage, self.base);
        Self::exclusive(|| storage.erase(base, base.saturating_add(SLOT.saturating_mul(2))).is_ok())
            .is_some_and(|(ok, _)| ok)
    }

    /// Die Adresse von `offset` in Slot `slot`; die Aufrufer pruefen beide
    /// vorher gegen [`SLOT`].
    fn at(&self, slot: u8, offset: u32) -> u32 {
        self.base.saturating_add(u32::from(slot).saturating_mul(SLOT)).saturating_add(offset)
    }

    /// Passen `len` Byte ab `offset` in einen Slot?
    fn fits(offset: u32, len: usize) -> bool {
        u32::try_from(len).is_ok_and(|len| offset.saturating_add(len) <= SLOT)
    }

    /// `at` um die Laenge eines Stuecks weiter.
    fn after(at: u32, len: usize) -> u32 {
        at.saturating_add(u32::try_from(len).unwrap_or(u32::MAX))
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
        let Some((ok, took)) = Self::exclusive(|| storage.erase(from, from.saturating_add(SLOT)).is_ok()) else {
            return false;
        };
        self.erase_max = self.erase_max.max(took);
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if slot > 1 || offset % 4 != 0 || !Self::fits(offset, bytes.len()) {
            return false;
        }
        // Wortweise, mit `0xFF` aufgefuellt: Im geloeschten Flash sind das
        // die Bytes, die ohnehin dort stehen.
        let mut at = self.at(slot, offset);
        let storage = &mut self.storage;
        let Some((ok, took)) = Self::exclusive(|| {
            let mut ok = true;
            for part in bytes.chunks(Chunk::BYTES) {
                let mut chunk = Chunk([0xFF; Chunk::BYTES]);
                for (to, from) in chunk.0.iter_mut().zip(part) {
                    *to = *from;
                }
                let n = part.len().next_multiple_of(4);
                ok &= chunk.0.get(..n).is_some_and(|words| storage.write_nor(at, words).is_ok());
                at = Self::after(at, n);
            }
            ok
        }) else {
            return false;
        };
        self.program_max = self.program_max.max(took);
        self.state = if ok { NvmState::Done } else { NvmState::Failed };
        true
    }

    fn poll(&mut self) -> NvmState {
        core::mem::take(&mut self.state)
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        if slot > 1 || offset % 4 != 0 || !Self::fits(offset, into.len()) {
            return false;
        }
        let mut at = self.at(slot, offset);
        let storage = &mut self.storage;
        Self::exclusive(|| {
            let mut ok = true;
            for part in into.chunks_mut(Chunk::BYTES) {
                let mut chunk = Chunk([0; Chunk::BYTES]);
                if let Some(read) = chunk.0.get_mut(..part.len()) {
                    ok &= storage.read_nor(at, read).is_ok();
                    part.copy_from_slice(read);
                } else {
                    ok = false;
                }
                at = Self::after(at, part.len());
            }
            ok
        })
        .is_some_and(|(ok, _)| ok)
    }

    fn blocking_ns(&self) -> Option<i64> {
        self.blocking_ns
    }
}
