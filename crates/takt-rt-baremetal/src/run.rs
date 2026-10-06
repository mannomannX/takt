//! Der Port „eigener Kern“ (12.3, 12.11) und die Senke der Bring-ups: ein
//! Lauf, nicht einer je Board. Das Board liefert Uhr, Leitung und Journal;
//! hier steht, was daraus ein Lauf macht — Ausgaenge im Takt der Leitung,
//! ein Paket je Tick, am Ende die Bilanz. Was ein Tick ist, rechnet der Kern
//! ([`Runtime::service`]); dieser Port wartet nur auf jede Frist.

use core::marker::PhantomData;

use takt_rt_core::{Clock, NextRun, Nvm, Outputs, Overrun, Persist, Program, Runtime, Sink, Tick, Tunables, Watchdog};

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
    /// Ueberlaeufe (7.3): Ticks ueber der Periode und Journal-Vorgaenge ueber
    /// die Frist.
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
        Trace { cadence, next: cadence.every.max(1), time_every, telemetry, slept: 0, line: PhantomData }
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
/// den Kern rechnen ([`Runtime::step_with`]), bis `cadence.limit` erreicht
/// ist oder das Programm seinen Lauf beendet (`next_run`, 12.7). An der
/// Grenze schreibt das Journal synchron. Die Tunables (8.4) gehen vor dem
/// Schritt ihrer Grenze in das Programm, im Schlaf wecken sie (9.9).
pub fn run<G, C, W, F, N, P, const R: usize>(
    rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P, R>>>,
    mut persist: Option<&mut Persist<'_, N>>,
    mut tunables: Option<&mut dyn Tunables>,
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
    if limit != 0 {
        rt.end_at(limit);
    }
    while rt.ended().is_none() && (limit == 0 || rt.tick_number() < limit) {
        let source: Option<&mut dyn Tunables> = match tunables.as_mut() {
            Some(t) => Some(&mut **t),
            None => None,
        };
        rt.step_with(persist.as_deref_mut(), source);
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
    // Der Kern zaehlt jeden Ueberlauf, auch den eines Journal-Vorgangs, der
    // in keiner Zeitzeile steht (7.3).
    Stats { slept: rt.sink.slept, overruns: rt.overrun().count, flushed, next_run: rt.ended() }
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
        let stats = run(&mut rt, None::<&mut Persist<'_, FakeNvm<0>>>, None);
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

    /// Eine Uhr, die das Geraet vorstellt.
    struct Shared<'a>(&'a Cell<i64>);

    impl Clock for Shared<'_> {
        fn now(&self) -> i64 {
            self.0.get()
        }

        fn wait_until(&mut self, deadline: i64) {
            self.0.set(self.0.get().max(deadline));
        }
    }

    /// Ein Geraet, das den Kern je Vorgang fuenf Perioden anhaelt (12.3).
    struct Stalling<'a> {
        nvm: FakeNvm<256>,
        clock: &'a Cell<i64>,
    }

    impl Nvm for Stalling<'_> {
        fn slot_size(&self) -> u32 {
            self.nvm.slot_size()
        }

        fn begin_erase(&mut self, slot: u8) -> bool {
            self.clock.set(self.clock.get() + 5_000_000);
            self.nvm.begin_erase(slot)
        }

        fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
            self.clock.set(self.clock.get() + 5_000_000);
            self.nvm.begin_write(slot, offset, bytes)
        }

        fn poll(&mut self) -> takt_rt_core::NvmState {
            self.nvm.poll()
        }

        fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
            self.nvm.read(slot, offset, into)
        }

        fn blocking_ns(&self) -> Option<i64> {
            Some(5_000_000)
        }
    }

    /// Persistiert seine Tickzahl.
    struct Counting(u32);

    impl Program for Counting {
        fn tick(&mut self, _k: u64, _now: i64) {
            self.0 += 1;
        }

        fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
            out[..4].copy_from_slice(&self.0.to_le_bytes());
            4
        }
    }

    /// **Die Bilanz zaehlt Journal-Vorgaenge ueber die Frist mit** (7.3
    /// letzter Satz): Unter `alert` ueberzieht kein Schritt, aber jeder
    /// Vorgang des blockierenden Geraets; `ueberlaeufe` nennt sie.
    #[test]
    fn the_balance_counts_journal_overruns() {
        let now = Cell::new(0);
        let (mut current, mut stored) = ([0u8; 256], [0u8; 256]);
        let device = Stalling { nvm: FakeNvm::new(), clock: &now };
        let mut persist = Persist::new(takt_rt_core::Journal::new(device, 7, 0), &mut current, &mut stored);
        let trace = Trace::new(Cadence::of(20, 1), 1_000_000, || None::<&'static mut Telemetry<NoLine, 8>>);
        let mut rt =
            Runtime::new(Counting(0), Shared(&now), NoWatchdog, trace, Profile::BAREMETAL, 1_000_000, Policy::Alert);
        persist.load(&mut rt.program);
        let stats = run(&mut rt, Some(&mut persist), None);
        assert!(persist.journal().writes() > 0);
        assert!(rt.overrun().count > 0);
        assert_eq!(stats.overruns, rt.overrun().count, "jeder Ueberlauf steht in der Bilanz");
    }

    /// Eine Leitung, die alles annimmt und dem Test zeigt.
    struct Memory<'a>(&'a core::cell::RefCell<([u8; 512], usize)>);

    impl Port for Memory<'_> {
        fn try_write(&mut self, b: u8) -> bool {
            let (sent, len) = &mut *self.0.borrow_mut();
            sent[*len] = b;
            *len += 1;
            true
        }
    }

    /// **Die Bilanzzeile in ihrer Form** (12.5, 13.8): je Kennzahl ihr Wort
    /// und ihre Zahl, in dieser Reihenfolge; `gesendet` zaehlt ab `takt
    /// trace`. Der Wirt liest die Woerter (`board::complete`, die Zaehler
    /// der Board-Tests); eine Umbenennung faellt hier auf, nicht erst am Board.
    #[test]
    fn the_balance_line_names_every_counter() {
        let line = core::cell::RefCell::new(([0u8; 512], 0));
        let mut t = Telemetry::<_, 512>::new(Memory(&line));
        t.write("boot\n");
        t.mark();
        t.write("t=0 out x 1\n");
        let mut overrun = Overrun::new(Policy::Fault);
        overrun.observe_drift(2_500_000, 1_000_000);
        overrun.observe(1_500_000, 1_000_000);
        let stats = Stats { slept: 9, overruns: 3, flushed: true, next_run: None };
        let journal = JournalStats { writes: 4, failures: 1, erase_ns: 12, program_ns: 5 };
        report(&mut t, &overrun, &stats, &journal, Some(2048));
        let (sent, len) = *line.borrow();
        let text = core::str::from_utf8(&sent[..len]).expect("ASCII");
        assert_eq!(
            text,
            "boot\ntakt trace\r\nt=0 out x 1\n\
             takt schlief 9 ueberlaeufe 3 verspaetet 1 verloren 2 rueckstand 2500000 ns verworfen 0 gesendet 12 \
             journal geschrieben 4 fehlgeschlagen 1 flush 1 nvm loeschen 12 ns programmieren 5 ns stack 2048\r\n\
             takt end\r\n"
        );
    }
}
