//! Das erzeugte Programm hinter seiner C-ABI (12.1, 12.3).
//!
//! Der Rahmen aus `takt_conformance::mcu` liefert `takt_mcu_init_with`,
//! `takt_mcu_tick` und die Schwestern; hier werden sie zum
//! [`takt_rt_core::Program`], das die Tickschleife kennt.
//!
//! **Laden vor dem Eintritt.** s0 enthaelt die geladenen `persist`-Werte
//! (5.9), also muss das Journal *vor* dem ersten `enter` gelesen sein. Das
//! Programm beginnt darum uninitialisiert; die erste `persist_restore`
//! initialisiert es mit den Bytes, und [`Generated::ensure_init`] holt den
//! Start ohne Journal nach, wenn keines da war.

use core::ffi::c_void;

use takt_rt_core::Program;

unsafe extern "C" {
    fn takt_mcu_init();
    fn takt_mcu_init_with(persist: *const c_void, len: i32) -> i32;
    fn takt_mcu_tick(k: i64);
    fn takt_mcu_dump();
    fn takt_mcu_output(index: i32) -> i64;
    fn takt_mcu_commit();
    fn takt_mcu_idle() -> bool;
    fn takt_mcu_deadline() -> i64;
    fn takt_mcu_advance(n: i64);
    fn takt_mcu_persist_snapshot(out: *mut c_void, cap: i32) -> i32;
    fn takt_mcu_persist_restore(bytes: *const c_void, len: i32) -> i32;
}

/// Das gebundene Programm.
#[derive(Clone, Copy, Debug, Default)]
pub struct Generated {
    /// Nach jedem Tick den Zustand ausgeben.
    pub trace: bool,
    initialized: bool,
}

impl Generated {
    /// Ein Programm vor Tick 0: das Journal darf noch laden.
    pub fn new(trace: bool) -> Generated {
        Generated { trace, initialized: false }
    }

    /// Initialisiert das Programm ohne Journal (Tick 0, 9.4).
    pub fn init(trace: bool) -> Generated {
        let mut p = Generated::new(trace);
        p.ensure_init();
        p
    }

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    pub fn ensure_init(&mut self) {
        if !self.initialized {
            // SAFETY: einmal vor dem ersten Tick; der Rahmen haelt seinen Zustand statisch.
            unsafe { takt_mcu_init() };
            self.initialized = true;
        }
    }

    /// Gibt Outputs und Zustaende ueber die Telemetrie aus.
    pub fn dump(&self) {
        // SAFETY: liest nur den statischen Zustand des Rahmens.
        unsafe { takt_mcu_dump() };
    }

    /// Commit der Outputs am Tick-Ende (9.4).
    pub fn commit(&self) {
        // SAFETY: schreibt die Latches des Rahmens; die Schleife ruft es einmal je Tick.
        unsafe { takt_mcu_commit() };
    }

    /// Der Wert eines Outputs, als Bitmuster.
    pub fn output(&self, index: i32) -> i64 {
        // SAFETY: liest einen Latch des Rahmens; ein fremder Index liefert 0.
        unsafe { takt_mcu_output(index) }
    }
}

impl Program for Generated {
    fn sleep_allowed(&self) -> bool {
        // SAFETY: liest nur den statischen Zustand des Rahmens.
        unsafe { takt_mcu_idle() }
    }

    fn next_deadline(&self) -> Option<i64> {
        // SAFETY: liest nur den statischen Zustand des Rahmens.
        let ns = unsafe { takt_mcu_deadline() };
        (ns >= 0).then_some(ns)
    }

    fn advance(&mut self, ticks: u64) {
        // SAFETY: der Rahmen rueckt seine Uhr vor; die Schleife ruft es nur im Schlaf.
        unsafe { takt_mcu_advance(ticks as i64) };
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        let cap = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen schreibt hoechstens `cap` Byte an `out`.
        let n = unsafe { takt_mcu_persist_snapshot(out.as_mut_ptr().cast(), cap) };
        usize::try_from(n).unwrap_or(0)
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen liest `len` Byte ab `bytes`; vor dem Start
        // initialisiert er sich damit, danach ersetzt er nur die Werte.
        let n = unsafe {
            if self.initialized {
                takt_mcu_persist_restore(bytes.as_ptr().cast(), len)
            } else {
                self.initialized = true;
                takt_mcu_init_with(bytes.as_ptr().cast(), len)
            }
        };
        usize::try_from(n).unwrap_or(0)
    }

    fn tick(&mut self, k: u64, _now: i64) {
        // Die Schleife zaehlt ihre Schritte ab 0; Rahmen und Interpreter
        // nennen den ersten Tick nach dem Start `t=1` (Tick 0 ist der Start).
        // SAFETY: ein Schritt des Rahmens, einmal je Tick aus der Schleife.
        unsafe { takt_mcu_tick(k as i64 + 1) };
        if self.trace {
            self.dump();
        }
    }
}
