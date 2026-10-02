//! Der Port „eigener Kern“ (12.3, 12.11) und die Senke der Bring-ups: ein
//! Lauf, nicht einer je Board. Das Board liefert Uhr, Leitung und Journal;
//! hier steht, was daraus ein Lauf macht — Ausgaenge im Takt der Leitung,
//! ein Paket je Tick, am Ende die Bilanz. Was ein Tick ist, rechnet der Kern
//! ([`Runtime::service`]); dieser Port wartet nur auf jede Frist.

use core::marker::PhantomData;

use takt_rt_core::{Clock, NextRun, Nvm, Outputs, Overrun, Persist, Program, Runtime, Sink, Tick, Watchdog};

use crate::telemetry::{DRAIN_ROUNDS, Port, Telemetry};

/// Kein Hardware-Watchdog angebunden.
pub struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn kick(&mut self) {}
}

/// Wie oft und wie lange der Lauf ausgibt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Alle wie viele Ticks die Ausgaenge gehen. `1` ist der
    /// Konformitaetslauf: nur die Aenderungen, dazu die Zeitzeile —
    /// je Tick, aber hoechstens eine je Millisekunde (FB-271).
    pub every: u64,
    /// Nach so vielen Ticks endet der Lauf; `0` heisst nie.
    pub limit: u64,
}

impl Cadence {
    /// Der Konformitaetslauf ueber `limit` Ticks, sonst alle `every` Ticks ein Abzug.
    pub fn of(limit: u64, every: u64) -> Cadence {
        Cadence { every: if limit > 0 { 1 } else { every.max(1) }, limit }
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

/// Die Senke eines Bring-ups (12.5): die Ausgaenge im Takt der Leitung. Im
/// Konformitaetslauf zeigt sie den Anfangszustand ganz und danach jeden
/// Tick seine Aenderungen mit Zeitzeile, sonst alle `every` Ticks alles.
///
/// `telemetry` holt die Leitung je Aufruf, wie der erzeugte Rahmen sie
/// ueber `takt_board_trace` holt — so gibt es nie zwei Griffe zugleich.
pub struct Trace<F, L> {
    cadence: Cadence,
    /// Ab dieser Tickzahl gehen die Ausgaenge das naechste Mal.
    next: u64,
    /// Alle wie viele Ticks die Zeitzeile geht.
    time_every: u64,
    telemetry: F,
    /// Geschlafene Ticks (9.9).
    slept: u64,
    /// Ticks ueber der Periode (7.3).
    overruns: u64,
    line: PhantomData<L>,
}

impl<F, P, const R: usize> Trace<F, Telemetry<P, R>>
where
    F: FnMut() -> Option<&'static mut Telemetry<P, R>>,
    P: Port + 'static,
{
    /// Eine Senke im Takt `cadence` fuer Ticks von `tick_ns`.
    pub fn new(cadence: Cadence, tick_ns: i64, telemetry: F) -> Trace<F, Telemetry<P, R>> {
        // Unter einer Millisekunde Tick traegt die Leitung keine Zeile je
        // Tick (FB-271); die Zeitzeile ist Statistik und darf duenner werden.
        let time_every = (1_000_000 / tick_ns.max(1)).max(1) as u64;
        Trace { cadence, next: cadence.every.max(1), time_every, telemetry, slept: 0, overruns: 0, line: PhantomData }
    }

    /// Nach so vielen Ticks endet der Lauf; `0` heisst nie.
    pub fn limit(&self) -> u64 {
        self.cadence.limit
    }
}

impl<F, P, const R: usize> Sink for Trace<F, Telemetry<P, R>>
where
    F: FnMut() -> Option<&'static mut Telemetry<P, R>>,
    P: Port + 'static,
{
    fn outputs(&mut self, tick: Option<&Tick>) -> Outputs {
        let conformance = self.cadence.conformance();
        let Some(tick) = tick else { return if conformance { Outputs::All } else { Outputs::None } };
        self.slept += tick.slept;
        self.overruns += u64::from(tick.overrun);
        if conformance
            && tick.k % self.time_every == 0
            && let Some(t) = (self.telemetry)()
        {
            t.write_time(tick);
        }
        let k = tick.k + 1 + tick.slept;
        if k < self.next {
            return Outputs::None;
        }
        self.next = k + self.cadence.every.max(1);
        if conformance { Outputs::Changed } else { Outputs::All }
    }

    fn record(&mut self, _tick: &Tick) {
        if let Some(t) = (self.telemetry)() {
            t.flush();
        }
    }
}

/// Der Port „eigener Kern“ (12.3, 12.11): wartet auf jede Frist und laesst
/// den Kern rechnen ([`Runtime::step`]), bis `cadence.limit` erreicht ist
/// oder das Programm seinen Lauf beendet (`next_run`, 12.7). An der Grenze
/// schreibt das Journal synchron.
pub fn run<G, C, W, F, N, P, const R: usize>(
    rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P, R>>>,
    mut persist: Option<&mut Persist<'_, N>>,
) -> Stats
where
    G: Program,
    C: Clock,
    W: Watchdog,
    F: FnMut() -> Option<&'static mut Telemetry<P, R>>,
    N: Nvm,
    P: Port + 'static,
{
    let limit = rt.sink.limit();
    while rt.ended().is_none() && (limit == 0 || rt.tick_number() < limit) {
        match persist.as_deref_mut() {
            Some(p) => rt.step_persisting(p),
            None => rt.step(),
        };
    }
    stats(rt, persist)
}

/// Die Bilanz am Ende eines Laufs; das Journal schreibt dabei synchron, wenn
/// der Kern es nicht schon am Ende des Programms getan hat.
pub fn stats<G, C, W, F, N, P, const R: usize>(
    rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P, R>>>,
    persist: Option<&mut Persist<'_, N>>,
) -> Stats
where
    G: Program,
    C: Clock,
    W: Watchdog,
    F: FnMut() -> Option<&'static mut Telemetry<P, R>>,
    N: Nvm,
    P: Port + 'static,
{
    let flushed = rt.finish(persist);
    Stats { slept: rt.sink.slept, overruns: rt.sink.overruns, flushed, next_run: rt.ended() }
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

    use crate::LogicalClock;

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

        fn commit(&mut self) {
            self.committed_after_end.set(self.ended.get());
        }

        fn trace(&mut self, outputs: Outputs) {
            if outputs == Outputs::All && !self.ended.get() {
                self.shown.set(Some(self.ticks));
                self.full_dumps.set(self.full_dumps.get() + 1);
            }
        }

        fn end(&mut self) {
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
        run_with(at, Cadence::of(60, 1))
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
        let trace = Trace::new(cadence, 1_000_000, || None::<&'static mut Telemetry<NoLine, 8>>);
        let mut rt = Runtime::new(program, clock, NoWatchdog, trace, Profile::BAREMETAL, 1_000_000, Policy::Fault);
        let stats = run(&mut rt, None::<&mut Persist<'_, FakeNvm<0>>>);
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
            let (_, program) = run_with(at, Cadence::of(0, 100));
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

    /// Ein Konformitaetslauf endet nach `limit` Ticks, ohne dass das
    /// Programm ihn beendet.
    #[test]
    fn a_conformance_run_ends_at_its_limit() {
        let (stats, program) = run_with(u64::MAX, Cadence::of(60, 1));
        assert_eq!((stats.next_run, program.ticks), (None, 60));
        assert!(!program.ended.get());
    }
}
