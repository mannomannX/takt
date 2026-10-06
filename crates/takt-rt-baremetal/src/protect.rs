//! Speicherschutz des Programmzustands (12.3).
//!
//! **Wogegen er schuetzt.** Das Programm kann per Konstruktion nicht
//! ausserhalb seiner Objekte schreiben; der Schutz wendet sich gegen die
//! TCB. Ausserhalb des Ticks darf den Programmzustand niemand beschreiben —
//! keine ISR, kein Treiber, kein Job. [`Guarded`] oeffnet ihn nur fuer die
//! Aufrufe, die ihn schreiben, und stellt eine Verletzung, die das Board
//! gemeldet hat, im naechsten Tick als `Runtime(Hardware)` fuer alle
//! Maschinen zu. Die Verletzung selbst hat der Zugriff nicht angerichtet:
//! Das Board uebergeht ihn, statt ihn nachzuholen.

use core::cell::Cell;

use takt_rt_core::{NextRun, Outputs, Program, Tolerance};

/// Eine Region, deren Regel ein Zugriff verletzt hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Der Name der Region (`Programmzustand`, `Waechter`).
    pub region: &'static str,
    /// Die Adresse des Zugriffs.
    pub address: u32,
}

/// Was ein Board zum Schutz beitraegt: Programmzustand oeffnen und
/// schliessen, und die letzte gemeldete Verletzung abholen.
pub trait Protection {
    /// Der Programmzustand wird beschreibbar.
    fn open(&self);

    /// Er wird wieder schreibgeschuetzt.
    fn close(&self);

    /// Die Verletzung seit der letzten Frage, falls es eine gab.
    fn take_violation(&self) -> Option<Violation>;
}

/// Kein Speicherschutz: ein Ziel ohne MPU.
pub struct Unprotected;

impl Protection for Unprotected {
    fn open(&self) {}
    fn close(&self) {}
    fn take_violation(&self) -> Option<Violation> {
        None
    }
}

/// Ein Programm hinter dem Speicherschutz des Boards.
pub struct Guarded<P, M> {
    /// Das Programm.
    pub program: P,
    /// Der Schutz des Boards.
    pub protection: M,
    /// Wie viele Verletzungen zugestellt wurden.
    pub violations: u32,
    /// Die letzte davon, fuer die Bilanz.
    pub last: Option<Violation>,
    /// Eine Verletzung, die schon die Frage nach dem Schlaf abgeholt hat;
    /// der naechste Tick stellt sie zu.
    held: Cell<Option<Violation>>,
}

impl<P, M: Protection> Guarded<P, M> {
    /// Schliesst den Programmzustand; bis hierher durfte `init` schreiben.
    pub fn new(program: P, protection: M) -> Self {
        protection.close();
        Guarded { program, protection, violations: 0, last: None, held: Cell::new(None) }
    }
}

impl<P: Program, M: Protection> Program for Guarded<P, M> {
    fn tick(&mut self, k: u64, now: i64) {
        if let Some(v) = self.held.take().or_else(|| self.protection.take_violation()) {
            self.violations = self.violations.saturating_add(1);
            self.last = Some(v);
            self.program.raise_hardware();
        }
        self.protection.open();
        self.program.tick(k, now);
        self.protection.close();
    }

    fn raise_overrun(&mut self) {
        self.program.raise_overrun();
    }

    fn raise_hardware(&mut self) {
        self.program.raise_hardware();
    }

    fn tick_tolerance(&self) -> Option<Tolerance> {
        self.program.tick_tolerance()
    }

    /// Eine gemeldete Verletzung ist ein Runtime-Ereignis (9.9): Sie wirkt
    /// im naechsten Tick, nicht an der Frist eines Schlafs.
    fn sleep_allowed(&self) -> bool {
        if self.held.get().is_none() {
            self.held.set(self.protection.take_violation());
        }
        self.held.get().is_none() && self.program.sleep_allowed()
    }

    fn next_deadline(&self) -> Option<i64> {
        self.program.next_deadline()
    }

    fn advance(&mut self, ticks: u64) {
        self.protection.open();
        self.program.advance(ticks);
        self.protection.close();
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        self.program.persist_snapshot(out)
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        self.protection.open();
        let n = self.program.persist_restore(bytes);
        self.protection.close();
        n
    }

    fn tune(&mut self, param: u32, value: &[u8]) {
        self.protection.open();
        self.program.tune(param, value);
        self.protection.close();
    }

    fn job_done(&mut self, slot: u32, result: Option<&[u8]>) {
        self.protection.open();
        self.program.job_done(slot, result);
        self.protection.close();
    }

    fn next_run(&self) -> Option<NextRun> {
        self.program.next_run()
    }

    /// Der Commit liest den Latch nur und gibt ihn an die Treiber (12.1): Er
    /// laeuft bei geschlossenem Programmzustand, und kein Output-Treiber kann
    /// ihn beschreiben.
    fn commit(&mut self) {
        self.program.commit();
    }

    fn commit_at_boundary(&self) -> bool {
        self.program.commit_at_boundary()
    }

    fn trace(&mut self, outputs: Outputs) {
        self.program.trace(outputs);
    }

    /// Das Ende setzt die Ausgaenge auf `safe` und schreibt damit den Zustand.
    fn end(&mut self) {
        self.protection.open();
        self.program.end();
        self.protection.close();
    }

    fn dispatch_job(&mut self) -> bool {
        self.program.dispatch_job()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::{Cell, RefCell};

    /// Ein Programm, das mitschreibt, wann es rechnet und ob es offen war.
    struct Recorder<'a> {
        log: &'a RefCell<[u8; 16]>,
        len: &'a Cell<usize>,
        open: &'a Cell<bool>,
        sleepy: bool,
    }

    impl Recorder<'_> {
        fn note(&self, b: u8) {
            let n = self.len.get();
            self.log.borrow_mut()[n] = b;
            self.len.set(n + 1);
        }

        /// Grossbuchstabe bei offenem, Kleinbuchstabe bei geschlossenem Zustand.
        fn mark(&self, b: u8) {
            self.note(if self.open.get() { b.to_ascii_uppercase() } else { b.to_ascii_lowercase() });
        }
    }

    impl Program for Recorder<'_> {
        fn tick(&mut self, _k: u64, _now: i64) {
            self.mark(b't');
        }
        fn raise_overrun(&mut self) {
            self.mark(b'o');
        }
        fn raise_hardware(&mut self) {
            self.mark(b'h');
        }
        fn tick_tolerance(&self) -> Option<Tolerance> {
            Some(Tolerance { ns: 7, runs: 3 })
        }
        fn advance(&mut self, _ticks: u64) {
            self.mark(b'a');
        }
        fn persist_snapshot(&mut self, _out: &mut [u8]) -> usize {
            self.mark(b's');
            0
        }
        fn persist_restore(&mut self, _bytes: &[u8]) -> usize {
            self.mark(b'r');
            0
        }
        fn tune(&mut self, _param: u32, _value: &[u8]) {
            self.mark(b'u');
        }
        fn job_done(&mut self, _slot: u32, _result: Option<&[u8]>) {
            self.mark(b'j');
        }
        fn commit(&mut self) {
            self.mark(b'c');
        }
        fn trace(&mut self, _outputs: Outputs) {
            self.mark(b'x');
        }
        fn end(&mut self) {
            self.mark(b'e');
        }
        fn sleep_allowed(&self) -> bool {
            self.sleepy
        }
    }

    struct Board<'a> {
        open: &'a Cell<bool>,
        pending: Cell<Option<Violation>>,
    }

    impl Protection for Board<'_> {
        fn open(&self) {
            self.open.set(true);
        }
        fn close(&self) {
            self.open.set(false);
        }
        fn take_violation(&self) -> Option<Violation> {
            self.pending.take()
        }
    }

    #[test]
    fn the_state_is_open_only_while_the_program_writes_it() {
        let (log, len, open) = (RefCell::new([0; 16]), Cell::new(0), Cell::new(true));
        let board = Board { open: &open, pending: Cell::new(None) };
        let mut g = Guarded::new(Recorder { log: &log, len: &len, open: &open, sleepy: false }, board);
        assert!(!open.get(), "nach `init` geschlossen");
        g.tick(0, 0);
        g.commit();
        assert!(!open.get());
        assert_eq!(&log.borrow()[..len.get()], b"Tc", "der Commit schreibt den Zustand nicht");
    }

    #[test]
    fn a_violation_becomes_runtime_hardware_in_the_next_tick() {
        let (log, len, open) = (RefCell::new([0; 16]), Cell::new(0), Cell::new(false));
        let v = Violation { region: "Programmzustand", address: 0x2000_0010 };
        let board = Board { open: &open, pending: Cell::new(Some(v)) };
        let mut g = Guarded::new(Recorder { log: &log, len: &len, open: &open, sleepy: false }, board);
        g.tick(0, 0);
        g.tick(1, 0);
        assert_eq!(&log.borrow()[..len.get()], b"hTT", "einmal zugestellt, vor dem Tick");
        assert_eq!((g.violations, g.last), (1, Some(v)));
    }

    /// Ohne die Weitergabe prueft die Schleife die Periode hinter dem
    /// Schutz nie (12.6 Zeile 7): Die Voreinstellung des Traits ist `None`.
    #[test]
    fn the_tick_tolerance_passes_through() {
        let (log, len, open) = (RefCell::new([0; 16]), Cell::new(0), Cell::new(false));
        let board = Board { open: &open, pending: Cell::new(None) };
        let g = Guarded::new(Recorder { log: &log, len: &len, open: &open, sleepy: false }, board);
        assert_eq!(g.tick_tolerance(), Some(Tolerance { ns: 7, runs: 3 }));
    }

    /// **Eine gemeldete Verletzung haelt die Schleife wach** (12.3, 9.9):
    /// Sie ist ein Runtime-Ereignis und wirkt im naechsten Tick, nicht erst
    /// an der Frist eines Schlafs; die Frage nach dem Schlaf verliert sie nicht.
    #[test]
    fn a_reported_violation_forbids_the_sleep_and_reaches_the_next_tick() {
        let (log, len, open) = (RefCell::new([0; 16]), Cell::new(0), Cell::new(false));
        let board = Board { open: &open, pending: Cell::new(None) };
        let mut g = Guarded::new(Recorder { log: &log, len: &len, open: &open, sleepy: true }, board);
        g.tick(0, 0);
        assert!(g.sleep_allowed(), "ohne Verletzung darf geschlafen werden");
        let v = Violation { region: "Programmzustand", address: 0x2000_0010 };
        g.protection.pending.set(Some(v));
        assert!(!g.sleep_allowed(), "mit gemeldeter Verletzung nicht");
        assert!(!g.sleep_allowed(), "auch bei der zweiten Frage nicht");
        g.tick(1, 0);
        assert_eq!(&log.borrow()[..len.get()], b"ThT", "zugestellt vor dem naechsten Tick");
        assert_eq!((g.violations, g.last), (1, Some(v)));
        assert!(g.sleep_allowed(), "danach wieder");
    }

    /// **Offen nur, wo der Rahmen den Zustand schreibt** (12.3): im Schritt,
    /// beim Nachtragen geschlafener Ticks, beim Laden des Journals, beim
    /// Uebernehmen eines Tunables oder Job-Ergebnisses und beim Ende auf
    /// `safe`; geschlossen beim Lesen fuer Journal und Trace, beim Vormerken
    /// von Faults und im Commit.
    #[test]
    fn every_method_sees_the_state_open_only_where_it_writes() {
        let (log, len, open) = (RefCell::new([0; 16]), Cell::new(0), Cell::new(false));
        let board = Board { open: &open, pending: Cell::new(None) };
        let mut g = Guarded::new(Recorder { log: &log, len: &len, open: &open, sleepy: false }, board);
        g.tick(0, 0);
        g.advance(3);
        g.persist_restore(&[1]);
        g.tune(0, &[2]);
        g.job_done(0, Some(&[3]));
        g.end();
        g.persist_snapshot(&mut [0; 4]);
        g.trace(Outputs::All);
        g.raise_overrun();
        g.raise_hardware();
        g.commit();
        assert_eq!(&log.borrow()[..len.get()], b"TARUJEsxohc");
        assert!(!open.get(), "danach wieder geschlossen");
    }
}
