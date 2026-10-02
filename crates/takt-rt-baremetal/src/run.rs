//! Die Bring-up-Schleife ueber der Runtime (12.1, 12.3): ein Lauf, nicht
//! einer je Board. Das Board liefert Uhr, Leitung und Journal; hier steht,
//! was daraus ein Lauf macht — Ausgaenge im Takt der Leitung, ein Paket
//! je Tick, am Ende die Bilanz.

use takt_rt_core::{Clock, NextRun, Nvm, Overrun, Persist, Program, Runtime, Sink, Tick, Watchdog};

use crate::telemetry::{DRAIN_ROUNDS, Port, Telemetry};

/// Das erzeugte Programm, soweit die Schleife es anspricht.
pub trait Traced: Program {
    /// Gibt den Latch an die Treiber (12.1, Schritt 10).
    fn commit(&self);

    /// Die Ausgaenge als Trace-Zeilen; ohne `all` nur die geaenderten (9.3).
    fn dump(&self, all: bool);

    /// Der Programmzaehler je Maschine (11.2).
    fn pc(&self);

    /// Das Programm beendet seinen Lauf (`next_run`, 12.7): die Zeile `end`
    /// in den Trace, dann alle Ausgaenge auf `safe`.
    fn end(&self);
}

/// Kein Hardware-Watchdog angebunden.
pub struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn kick(&mut self) {}
}

/// Wie oft und wie lange die Schleife ausgibt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Alle wie viele Ticks die Ausgaenge gehen. `1` ist der
    /// Konformitaetslauf: nur die Aenderungen, dazu die Zeitzeile —
    /// je Tick, aber hoechstens eine je Millisekunde (FB-271).
    pub every: u64,
    /// Nach so vielen Ticks endet der Lauf; `0` heisst nie.
    pub limit: u64,
    /// Den Programmzaehler mitgeben (11.2, `statements`).
    pub pc: bool,
}

impl Cadence {
    /// Der Konformitaetslauf ueber `limit` Ticks, sonst alle `every` Ticks ein Abzug.
    pub fn of(limit: u64, every: u64, pc: bool) -> Cadence {
        Cadence { every: if limit > 0 { 1 } else { every.max(1) }, limit, pc }
    }

    fn conformance(self) -> bool {
        self.every == 1
    }
}

/// Die Bilanz eines Laufs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Geschlafene Ticks (9.9).
    pub slept: u64,
    /// Ticks ueber der Periode (7.3).
    pub overruns: u64,
    /// Das Journal hat am Ende geschrieben (5.9).
    pub flushed: bool,
    /// Wann der naechste Lauf beginnen soll, wenn das Programm seinen
    /// beendet hat (12.7); `None` an der Grenze des Laufs.
    pub next_run: Option<NextRun>,
}

/// Das Journal in Zahlen, fuer die Bilanz.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStats {
    /// Geschriebene Eintraege.
    pub writes: u32,
    /// Gescheiterte Schreibversuche.
    pub failures: u32,
    /// Gemessene Loeschdauer in ns.
    pub erase_ns: i64,
    /// Gemessene Programmierdauer in ns.
    pub program_ns: i64,
}

/// Was ein Lauf ueber die Ticks mitfuehrt (12.1): Ausgabe im Takt der
/// Leitung, Ende des Laufs, Grenze, Bilanz. Die Bare-Metal-Schleife ([`run`])
/// treibt ihn je Tick, und ebenso die Aufgabe unter einem RTOS (12.8,
/// `takt-rt-rtos`), die nicht selbst wartet.
#[derive(Debug)]
pub struct Runner {
    cadence: Cadence,
    /// Ab dieser Tickzahl gehen die Ausgaenge das naechste Mal.
    next: u64,
    /// Alle wie viele Ticks die Zeitzeile geht.
    time_every: u64,
    /// Ob der Stand des letzten Ticks schon ausgegeben ist; im
    /// Konformitaetslauf ist es jeder, auch der Anfangszustand.
    shown: bool,
    /// Die Grenze `cadence.limit` ist erreicht.
    ended: bool,
    /// Die Bilanz bis hierher.
    pub stats: Stats,
}

impl Runner {
    /// Beginnt einen Lauf; der Konformitaetslauf zeigt den Anfangszustand.
    pub fn start<G: Traced>(program: &G, cadence: Cadence, tick_ns: i64) -> Runner {
        if cadence.conformance() {
            program.dump(true);
        }
        // Unter einer Millisekunde Tick traegt die Leitung keine Zeile je
        // Tick (FB-271); die Zeitzeile ist Statistik und darf duenner werden.
        let time_every = (1_000_000 / tick_ns.max(1)).max(1) as u64;
        Runner {
            cadence,
            next: cadence.every.max(1),
            time_every,
            shown: cadence.conformance(),
            ended: false,
            stats: Stats { next_run: program.next_run(), ..Stats::default() },
        }
    }

    /// Laeuft der Lauf weiter? Nicht, wenn das Programm ihn beendet hat
    /// (12.7), und nicht nach der Grenze.
    pub fn running(&self) -> bool {
        self.stats.next_run.is_none() && !self.ended
    }

    /// Nach einem Tick: der Latch an die Treiber, die Ausgabe, das Ende des
    /// Laufs, die Grenze.
    ///
    /// `telemetry` holt die Leitung je Aufruf, wie der erzeugte Rahmen sie
    /// ueber `takt_board_trace` holt — so gibt es nie zwei Griffe zugleich.
    pub fn after<G, C, W, S, P, const R: usize>(
        &mut self,
        rt: &mut Runtime<G, C, W, S>,
        tick: &Tick,
        telemetry: &mut impl FnMut() -> Option<&'static mut Telemetry<P, R>>,
    ) where
        G: Traced,
        C: Clock,
        W: Watchdog,
        S: Sink,
        P: Port + 'static,
    {
        self.stats.slept += tick.slept;
        self.stats.overruns += u64::from(tick.overrun);
        rt.program.commit();
        let conformance = self.cadence.conformance();
        if conformance
            && tick.k % self.time_every == 0
            && let Some(t) = telemetry()
        {
            t.write_time(tick);
        }
        let k = rt.tick_number();
        self.shown = k >= self.next;
        if self.shown {
            self.next = k + self.cadence.every.max(1);
            rt.program.dump(!conformance);
            if self.cadence.pc {
                rt.program.pc();
            }
        }
        if let Some(t) = telemetry() {
            t.flush();
        }
        self.stats.next_run = rt.program.next_run();
        self.ended = self.cadence.limit > 0 && k >= self.cadence.limit;
    }

    /// Beendet den Lauf. Das Journal schreibt synchron, und erst danach
    /// gehen bei einem geordneten Ende alle Ausgaenge auf `safe` — auch wenn
    /// schon der Anfangszustand `next_run` setzt.
    pub fn finish<G, C, W, S, N>(mut self, rt: &mut Runtime<G, C, W, S>, persist: Option<&mut Persist<'_, N>>) -> Stats
    where
        G: Traced,
        C: Clock,
        W: Watchdog,
        S: Sink,
        N: Nvm,
    {
        self.stats.flushed = persist.is_some_and(|p| p.flush(&mut rt.program));
        if self.stats.next_run.is_some() {
            let conformance = self.cadence.conformance();
            // Den Tick, der das Ende verlangt, zeigt auch ein freier Lauf,
            // der sonst nur jeden `every`-ten ausgibt: Er erklaert das Ende.
            if !self.shown {
                rt.program.dump(!conformance);
            }
            rt.program.end();
            rt.program.commit();
            rt.program.dump(!conformance);
        }
        self.stats
    }
}

/// Laeuft, bis `cadence.limit` erreicht ist oder das Programm seinen Lauf
/// beendet (`next_run`, 12.7); die Schleife wartet selbst auf
/// jeden Tick ([`Runtime::step`]).
pub fn run<G, C, W, S, N, P, const R: usize>(
    rt: &mut Runtime<G, C, W, S>,
    mut persist: Option<&mut Persist<'_, N>>,
    cadence: Cadence,
    mut telemetry: impl FnMut() -> Option<&'static mut Telemetry<P, R>>,
) -> Stats
where
    G: Traced,
    C: Clock,
    W: Watchdog,
    S: Sink,
    N: Nvm,
    P: Port + 'static,
{
    let mut runner = Runner::start(&rt.program, cadence, rt.tick_ns());
    while runner.running() {
        let tick = match persist.as_deref_mut() {
            Some(p) => rt.step_persisting(p),
            None => rt.step(),
        };
        runner.after(rt, &tick, &mut telemetry);
    }
    runner.finish(rt, persist)
}

/// Die Bilanz als letzte Zeile, dann `takt end`; leert den Ring.
///
/// `stack` ist die Tiefe des Stacks unter Last in Byte, wenn das Board sie
/// gemessen hat (Painting, 13.8): Aus dem Lauf eines leeren Programms wird
/// die Stack-Reserve von Runtime, Treibern und ISRs (12.3).
pub fn report<P: Port, const R: usize>(
    t: &mut Telemetry<P, R>,
    overrun: &Overrun,
    stats: &Stats,
    journal: &JournalStats,
    stack: Option<u32>,
) {
    let (dropped, sent) = (u64::from(t.dropped()), u64::from(t.sent()));
    let counts = [
        ("takt schlief ", stats.slept),
        (" ueberlaeufe ", stats.overruns),
        (" verspaetet ", overrun.late),
        (" verloren ", overrun.lost),
    ];
    for (label, n) in counts {
        t.write(label);
        t.write_u64(n);
    }
    t.write(" rueckstand ");
    t.write_i64(overrun.worst_drift);
    t.write(" ns verworfen ");
    t.write_u64(dropped);
    t.write(" gesendet ");
    t.write_u64(sent);
    let journal_counts = [
        (" journal geschrieben ", u64::from(journal.writes)),
        (" fehlgeschlagen ", u64::from(journal.failures)),
        (" flush ", u64::from(stats.flushed)),
    ];
    for (label, n) in journal_counts {
        t.write(label);
        t.write_u64(n);
    }
    t.write(" nvm loeschen ");
    t.write_i64(journal.erase_ns);
    t.write(" ns programmieren ");
    t.write_i64(journal.program_ns);
    t.write(" ns");
    if let Some(bytes) = stack {
        t.write(" stack ");
        t.write_u64(u64::from(bytes));
    }
    t.newline();
    t.write("takt end");
    t.newline();
    t.drain(DRAIN_ROUNDS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use takt_rt_core::{FakeNvm, Policy, Profile};

    use crate::{LogicalClock, Telemetry};

    /// Beendet ab Tick `at` den Lauf und merkt sich, was die Schleife tat.
    struct Ending {
        at: u64,
        ticks: u64,
        ended: Cell<bool>,
        committed_after_end: Cell<bool>,
        /// Der letzte Tick, dessen ganzer Stand vor dem Ende ausgegeben
        /// wurde, und wie oft das geschah.
        shown: Cell<Option<u64>>,
        full_dumps: Cell<u32>,
    }

    impl Program for Ending {
        fn tick(&mut self, k: u64, _now: i64) {
            self.ticks = k + 1;
        }

        fn next_run(&self) -> Option<NextRun> {
            (self.ticks >= self.at).then_some(NextRun::OnStart)
        }
    }

    impl Traced for Ending {
        fn commit(&self) {
            self.committed_after_end.set(self.ended.get());
        }

        fn dump(&self, all: bool) {
            if all && !self.ended.get() {
                self.shown.set(Some(self.ticks));
                self.full_dumps.set(self.full_dumps.get() + 1);
            }
        }

        fn pc(&self) {}

        fn end(&self) {
            self.ended.set(true);
        }
    }

    struct NoLine;

    impl Port for NoLine {
        fn try_write(&mut self, _b: u8) -> bool {
            true
        }
    }

    fn run_until(at: u64) -> (Stats, Ending) {
        run_with(at, Cadence::of(60, 1, false))
    }

    fn run_with(at: u64, cadence: Cadence) -> (Stats, Ending) {
        let program = Ending {
            at,
            ticks: 0,
            ended: Cell::new(false),
            committed_after_end: Cell::new(false),
            shown: Cell::new(None),
            full_dumps: Cell::new(0),
        };
        let clock = LogicalClock::new(|| {});
        let mut rt = Runtime::new(program, clock, NoWatchdog, (), Profile::BAREMETAL, 1_000_000, Policy::Fault);
        let stats =
            run(&mut rt, None::<&mut Persist<'_, FakeNvm<0>>>, cadence, || None::<&'static mut Telemetry<NoLine, 8>>);
        (stats, rt.program)
    }

    /// `next_run` beendet den Lauf nach seinem Tick; die `safe`-Werte gehen
    /// danach noch an die Treiber (12.7).
    #[test]
    fn next_run_ends_the_run_after_its_tick() {
        let (stats, program) = run_until(3);
        assert_eq!(stats.next_run, Some(NextRun::OnStart));
        assert_eq!(program.ticks, 3);
        assert!(program.ended.get() && program.committed_after_end.get());
    }

    /// Ein freier Lauf gibt nur jeden hundertsten Tick aus; den Tick, der
    /// das Ende verlangt, zeigt er trotzdem, vor den `safe`-Werten — und ohne
    /// ihn doppelt zu zeigen, wenn er ohnehin dran war.
    #[test]
    fn a_free_run_shows_the_tick_that_ends_it() {
        for at in [30, 100, 0] {
            let (_, program) = run_with(at, Cadence::of(0, 100, false));
            assert_eq!((program.shown.get(), program.full_dumps.get()), (Some(at), 1), "Ende ab Tick {at}");
        }
    }

    /// Setzt schon der Anfangszustand `next_run`, laeuft kein Tick.
    #[test]
    fn an_end_from_the_start_ends_the_run_before_the_first_tick() {
        let (stats, program) = run_until(0);
        assert_eq!(stats.next_run, Some(NextRun::OnStart));
        assert_eq!(program.ticks, 0);
    }
}
