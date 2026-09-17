//! Das erzeugte Programm hinter seiner C-ABI (12.1, 12.3).
//!
//! Der Rahmen aus `takt_conformance::mcu` liefert `takt_mcu_init`,
//! `takt_mcu_tick` und die Schwestern; hier werden sie zum
//! [`takt_rt_core::Program`], das die Tickschleife kennt. Dieselbe Datei
//! wie beim F401: Die ABI haengt am Rahmen, nicht am Chip.

use takt_rt_core::Program;

unsafe extern "C" {
    fn takt_mcu_init();
    fn takt_mcu_tick(k: i64);
    fn takt_mcu_dump();
    fn takt_mcu_output(index: i32) -> i64;
    fn takt_mcu_commit();
    fn takt_mcu_idle() -> bool;
    fn takt_mcu_deadline() -> i64;
    fn takt_mcu_advance(n: i64);
}

/// Das gebundene Programm.
#[derive(Clone, Copy, Debug, Default)]
pub struct Generated {
    /// Nach jedem Tick den Zustand ausgeben.
    pub trace: bool,
}

impl Generated {
    /// Initialisiert das Programm (Tick 0, 9.4).
    pub fn init(trace: bool) -> Generated {
        // SAFETY: einmal vor dem ersten Tick; der Rahmen haelt seinen Zustand statisch.
        unsafe { takt_mcu_init() };
        Generated { trace }
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

    fn tick(&mut self, k: u64, _now: i64) {
        // SAFETY: ein Schritt des Rahmens, einmal je Tick aus der Schleife.
        unsafe { takt_mcu_tick(k as i64) };
        if self.trace {
            self.dump();
        }
    }
}
