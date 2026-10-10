//! Die Interruptform (12.11, `plan/m11.md` 2.4): Der Schritt rechnet in der
//! ISR eines Zeitgebers, der einmal zu der Frist feuert, die `service`
//! nennt; die Hauptschleife gehoert dem Wirt und tut ihre eigene Arbeit.
//! Jobs (4.5) rechnen in einem Interrupt niedrigster Prioritaet, den die ISR
//! anstehen laesst, sobald `service` einen meldet.
//!
//! Der Kern ist derselbe wie in der Form „eigener Kern“ ([`crate::run`]):
//! `service` rechnet jede Grenze bis jetzt, auch mehrere, wenn die ISR zu
//! spaet kam, und das Raster bleibt `t0 + k·T0` (7.3). Was das Board
//! beitraegt, ist der [`Alarm`]: ein freilaufender Zeitgeber mit einem
//! Vergleich auf die Frist und ein Interrupt fuer die Jobs.
//!
//! In einem Konformitaetslauf zaehlt die Zeit logisch (13.8): Der Alarm
//! wartet dann hinter einem Tor, das die Hauptschleife oeffnet, wenn das
//! System ruht ([`release`]), und die Uhr steht mit jedem Alarm auf seiner
//! Frist ([`Time::Logical`]). So haelt jeder Job seine Dauer, und keine
//! Zeile geht verloren, wie unter [`crate::LogicalClock`].
//!
//! **Die Pollform** (12.11) ist dieselbe Form ohne Zeitgeber: Die
//! Hauptschleife des Wirts fragt je Runde die Frist ab und ist selbst der
//! Schrittkontext ([`Form::poll`]); der Alarm [`Polled`] stellt nur den
//! Job-Interrupt.

use core::sync::atomic::{AtomicBool, Ordering};

use takt_rt_core::{Clock, Nvm, Persist, Program, Runtime, Tunables, Watchdog};

use crate::run::{Stats, Trace, stats};
use crate::telemetry::{Port, Telemetry};

/// Was die Interruptform vom Board braucht.
pub trait Alarm {
    /// Stellt den Zeitgeber auf die absolute Frist `at` in Nanosekunden
    /// seit dem Start; ist sie schon vorbei, steht seine ISR sofort an.
    fn arm(&mut self, at: i64);

    /// Laesst den Job-Interrupt anstehen: Er rechnet, was `service` als
    /// `jobs` gemeldet hat (4.5).
    fn pend_jobs(&mut self);
}

/// Die Uhr der Interruptform.
#[derive(Clone, Copy, Debug)]
pub enum Time<C> {
    /// Die Zeitachse des Boards, auf der der Alarm steht.
    Board(C),
    /// Logische Zeit in Nanosekunden (13.8): Sie steht mit jedem Alarm auf
    /// seiner Frist.
    Logical(i64),
}

impl<C: Clock> Clock for Time<C> {
    fn now(&self) -> i64 {
        match self {
            Time::Board(clock) => clock.now(),
            Time::Logical(now) => *now,
        }
    }

    fn wait_until(&mut self, deadline: i64) {
        match self {
            Time::Board(clock) => clock.wait_until(deadline),
            Time::Logical(now) => *now = (*now).max(deadline),
        }
    }
}

/// Die Interruptform ueber dem Alarm `A`: der Alarm und die Frist, auf die
/// er steht.
#[derive(Debug)]
pub struct Form<A> {
    alarm: A,
    at: i64,
    /// In logischer Zeit das Tor, hinter dem der Alarm wartet.
    gate: Option<&'static AtomicBool>,
}

impl<A: Alarm> Form<A> {
    /// Beginnt den Lauf: Ein Konformitaetslauf endet nach `cadence.limit`
    /// Ticks, und der Alarm steht auf der Frist von Tick 0. Mit `gate`
    /// zaehlt die Zeit logisch: Statt den Zeitgeber zu stellen, schliesst
    /// der Alarm das Tor, und [`release`] gibt ihn frei.
    pub fn start<G, C, W, F, P>(
        rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P>>>,
        alarm: A,
        gate: Option<&'static AtomicBool>,
    ) -> Form<A>
    where
        G: Program,
        C: Clock,
        W: Watchdog,
        F: FnMut() -> Option<&'static mut Telemetry<P>>,
        P: Port + 'static,
    {
        let limit = rt.sink.limit();
        if limit != 0 {
            rt.end_at(limit);
        }
        let mut form = Form { alarm, at: 0, gate };
        form.arm(rt.deadline());
        form
    }

    /// Aus der ISR des Alarms: rechnet jede Grenze bis jetzt, laesst bei
    /// einem wartenden Job den Job-Interrupt anstehen und stellt den Alarm
    /// auf die naechste Frist. Am Ende des Laufs — nach `cadence.limit`
    /// Ticks oder durch `next_run` (12.7) — schreibt das Journal synchron,
    /// kein Alarm steht mehr, und die Bilanz kommt zurueck.
    pub fn on_alarm<G, C, W, F, N, P>(
        &mut self,
        rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P>>>,
        mut persist: Option<&mut Persist<'_, N>>,
        tunables: Option<&mut dyn Tunables>,
    ) -> Option<Stats>
    where
        G: Program,
        C: Clock,
        W: Watchdog,
        F: FnMut() -> Option<&'static mut Telemetry<P>>,
        N: Nvm,
        P: Port + 'static,
    {
        // Der Alarm feuert nie vor seiner Frist; eine logische Uhr steht
        // erst jetzt auf ihr.
        rt.clock.wait_until(self.at);
        let next = rt.service_with(persist.as_deref_mut(), tunables);
        if next.jobs {
            self.alarm.pend_jobs();
        }
        let limit = rt.sink.limit();
        if rt.ended().is_some() || (limit != 0 && rt.tick_number() >= limit) {
            return Some(stats(rt, persist));
        }
        self.arm(next.deadline);
        None
    }

    /// Aus der ISR des Alarms, wenn nicht die Frist sie ausloeste, sondern
    /// der Job-Interrupt nach seinem Auftrag: `service` vor der Frist
    /// rechnet keine Grenze, gibt aber dem ruhenden Job-Kontext den
    /// naechsten wartenden Job (4.5). So rechnet der Job-Interrupt jeden
    /// wartenden Job vor der naechsten Grenze, wie der Job-Faden des eigenen
    /// Kerns. Der Alarm bleibt auf seiner Frist; ist sie inzwischen erreicht,
    /// rechnet [`Form::on_alarm`] dort nichts mehr nach, was hier schon lief.
    pub fn on_job_done<G, C, W, F, N, P>(
        &mut self,
        rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P>>>,
        persist: Option<&mut Persist<'_, N>>,
        tunables: Option<&mut dyn Tunables>,
    ) where
        G: Program,
        C: Clock,
        W: Watchdog,
        F: FnMut() -> Option<&'static mut Telemetry<P>>,
        N: Nvm,
        P: Port + 'static,
    {
        if rt.service_with(persist, tunables).jobs {
            self.alarm.pend_jobs();
        }
    }

    /// Eine Runde der Hauptschleife in der Pollform (12.11): Hat der
    /// Job-Interrupt seit der letzten Runde einen Auftrag gerechnet
    /// (`job_done`), verteilt `service` den naechsten
    /// ([`Form::on_job_done`]); sonst rechnet es, sobald die Frist erreicht
    /// ist ([`Form::on_alarm`]). In logischer Zeit ist sie in jeder Runde
    /// erreicht, in der das Tor offen ist: Die Hauptschleife laeuft erst,
    /// wenn der Job-Interrupt ruht. Am Ende des Laufs die Bilanz.
    pub fn poll<G, C, W, F, N, P>(
        &mut self,
        rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P>>>,
        persist: Option<&mut Persist<'_, N>>,
        tunables: Option<&mut dyn Tunables>,
        job_done: bool,
    ) -> Option<Stats>
    where
        G: Program,
        C: Clock,
        W: Watchdog,
        F: FnMut() -> Option<&'static mut Telemetry<P>>,
        N: Nvm,
        P: Port + 'static,
    {
        if job_done {
            self.on_job_done(rt, persist, tunables);
            return None;
        }
        let due = match self.gate {
            Some(gate) => gate.swap(false, Ordering::AcqRel),
            None => rt.clock.now() >= self.at,
        };
        if due { self.on_alarm(rt, persist, tunables) } else { None }
    }

    fn arm(&mut self, at: i64) {
        self.at = at;
        match self.gate {
            Some(gate) => gate.store(true, Ordering::Release),
            None => self.alarm.arm(at),
        }
    }
}

/// Der Alarm der Pollform (12.11): Einen Zeitgeber gibt es nicht, die
/// Hauptschleife fragt die Frist ab ([`Form::poll`]). `pend` laesst den
/// Job-Interrupt anstehen.
#[derive(Debug)]
pub struct Polled<F>(pub F);

impl<F: FnMut()> Alarm for Polled<F> {
    fn arm(&mut self, _at: i64) {}

    fn pend_jobs(&mut self) {
        (self.0)();
    }
}

/// Aus der Hauptschleife, in logischer Zeit: Wartet ein Alarm hinter
/// `gate`, steht seine ISR jetzt an. Die Hauptschleife laeuft erst, wenn
/// keine ISR mehr aktiv ist und der Job-Interrupt seinen Auftrag gerechnet
/// hat; so haelt jeder Job seine Dauer (4.5).
pub fn release(gate: &AtomicBool, alarm: &mut impl Alarm) {
    if gate.swap(false, Ordering::AcqRel) {
        alarm.arm(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use takt_rt_core::{FakeNvm, NextRun, Outputs, Policy, Profile};

    use crate::run::{Cadence, NoWatchdog};

    extern crate std;
    use std::vec::Vec;

    /// Zaehlt die Ticks und ihre Zeiten und beendet ab `at` den Lauf. In
    /// Tick `start.0` beginnen `start.1` Jobs; je Verteilung merkt es sich,
    /// nach wie vielen Ticks sie geschah.
    struct Counting {
        at: u64,
        start: (u64, u32),
        waiting: u32,
        dispatched: Vec<u64>,
        ticks: u64,
        times: Vec<i64>,
    }

    impl Counting {
        fn new(at: u64) -> Counting {
            Counting { at, start: (u64::MAX, 0), waiting: 0, dispatched: Vec::new(), ticks: 0, times: Vec::new() }
        }

        fn with_jobs(self, tick: u64, count: u32) -> Counting {
            Counting { start: (tick, count), ..self }
        }
    }

    impl Program for Counting {
        fn tick(&mut self, k: u64, now: i64) {
            self.ticks = k + 1;
            self.times.push(now);
            if k == self.start.0 {
                self.waiting += self.start.1;
            }
        }

        fn next_run(&self) -> Option<NextRun> {
            (self.ticks >= self.at).then_some(NextRun::OnStart)
        }

        fn trace(&mut self, _outputs: Outputs) {}

        fn dispatch_job(&mut self) -> bool {
            if self.waiting == 0 {
                return false;
            }
            self.waiting -= 1;
            self.dispatched.push(self.ticks);
            true
        }
    }

    struct NoLine;

    impl Port for NoLine {
        fn try_write(&mut self, _b: u8) -> bool {
            true
        }
    }

    /// Eine Uhr, die der Test vorstellt.
    struct Shared<'a>(&'a Cell<i64>);

    impl Clock for Shared<'_> {
        fn now(&self) -> i64 {
            self.0.get()
        }

        fn wait_until(&mut self, deadline: i64) {
            self.0.set(self.0.get().max(deadline));
        }
    }

    /// Ein Alarm, der sich Frist und Job-Interrupt merkt.
    #[derive(Default)]
    struct Recorded {
        armed: Option<i64>,
        jobs: u32,
        pending: bool,
    }

    impl Alarm for Recorded {
        fn arm(&mut self, at: i64) {
            self.armed = Some(at);
        }

        fn pend_jobs(&mut self) {
            self.jobs += 1;
            self.pending = true;
        }
    }

    const T0: i64 = 1_000_000;

    type Line = Trace<fn() -> Option<&'static mut Telemetry<NoLine>>, Telemetry<NoLine>>;

    fn no_line() -> Option<&'static mut Telemetry<NoLine>> {
        None
    }

    fn no_journal<'a>() -> Option<&'a mut Persist<'a, FakeNvm<0>>> {
        None
    }

    /// Faehrt die Interruptform: Die ISR feuert zur gestellten Frist, um
    /// `late` verspaetet, bis der Lauf endet. Steht der Job-Interrupt an,
    /// rechnet er seinen Auftrag sofort und laesst neu verteilen.
    fn interrupt_run(program: Counting, cadence: Cadence, late: i64) -> (Stats, Counting, Form<Recorded>) {
        let now = Cell::new(0);
        let trace: Line = Trace::new(cadence, T0, no_line);
        let mut rt = Runtime::new(program, Shared(&now), NoWatchdog, trace, Profile::SHARED, T0, Policy::Alert);
        let mut form = Form::start(&mut rt, Recorded::default(), None);
        let stats = loop {
            let at = form.alarm.armed.take().expect("ein Alarm steht");
            now.set(now.get().max(at) + late);
            if let Some(stats) = form.on_alarm(&mut rt, no_journal(), None) {
                break stats;
            }
            while core::mem::take(&mut form.alarm.pending) {
                form.on_job_done(&mut rt, no_journal(), None);
            }
        };
        (stats, rt.program, form)
    }

    /// Dieselbe Bilanz wie die Form „eigener Kern“: Ein Konformitaetslauf
    /// endet nach `limit` Ticks, `next_run` nach seinem Tick; danach steht
    /// kein Alarm mehr.
    #[test]
    fn the_interrupt_form_runs_the_same_ticks_as_the_own_core() {
        let (stats, program, form) = interrupt_run(Counting::new(u64::MAX), Cadence::of(40, 1), 0);
        assert_eq!((stats.next_run, program.ticks), (None, 40));
        assert_eq!(form.alarm.armed, None);
        let (stats, program, _) = interrupt_run(Counting::new(7), Cadence::of(40, 1), 0);
        assert_eq!((stats.next_run, program.ticks), (Some(NextRun::OnStart), 7));
    }

    /// Kommt die ISR zu spaet, rechnet `service` jede verpasste Grenze
    /// nach; das Raster bleibt, die Zahl der Ticks stimmt.
    #[test]
    fn a_late_interrupt_catches_up_every_boundary() {
        let (_, program, _) = interrupt_run(Counting::new(u64::MAX), Cadence::of(30, 1), 3 * T0 / 2);
        assert_eq!(program.ticks, 30);
    }

    /// Meldet `service` einen wartenden Job, steht der Job-Interrupt an.
    #[test]
    fn a_waiting_job_pends_the_job_interrupt() {
        let (_, _, form) = interrupt_run(Counting::new(u64::MAX).with_jobs(5, 1), Cadence::of(10, 1), 0);
        assert_eq!(form.alarm.jobs, 1);
        let (_, _, quiet) = interrupt_run(Counting::new(u64::MAX), Cadence::of(10, 1), 0);
        assert_eq!(quiet.alarm.jobs, 0);
    }

    /// Warten mehrere Jobs, verteilt `service` nach jedem Auftrag des
    /// Job-Interrupts den naechsten: Alle rechnen vor der naechsten Grenze,
    /// keiner einen Tick spaeter (4.5).
    #[test]
    fn every_waiting_job_runs_before_the_next_boundary() {
        let (_, program, form) = interrupt_run(Counting::new(u64::MAX).with_jobs(5, 3), Cadence::of(10, 1), 0);
        assert_eq!((program.dispatched, form.alarm.jobs), (std::vec![6, 6, 6], 3));
    }

    /// **Die Pollform**: Die Hauptschleife fragt je Runde die Frist ab, eine
    /// Runde dauert ein Siebtel der Periode. Jeder Tick liegt auf seinem
    /// Raster, keiner faellt aus, und nach jedem Auftrag des Job-Interrupts
    /// verteilt die naechste Runde den naechsten Job, alle vor der naechsten
    /// Grenze.
    #[test]
    fn the_poll_form_steps_on_the_grid_and_dispatches_every_job() {
        let now = Cell::new(0);
        let trace: Line = Trace::new(Cadence::of(20, 1), T0, no_line);
        let program = Counting::new(u64::MAX).with_jobs(5, 3);
        let mut rt = Runtime::new(program, Shared(&now), NoWatchdog, trace, Profile::SHARED, T0, Policy::Alert);
        let pended = Cell::new(0u32);
        let mut form = Form::start(&mut rt, Polled(|| pended.set(pended.get() + 1)), None);
        let round = T0 / 7;
        let mut done = 0;
        let stats = loop {
            // Der Job-Interrupt rechnet, sobald er ansteht, und meldet es der naechsten Runde.
            let job_done = pended.get() > done;
            done = pended.get();
            if let Some(stats) = form.poll(&mut rt, no_journal(), None, job_done) {
                break stats;
            }
            now.set(now.get() + round);
        };
        assert_eq!((stats.next_run, rt.program.ticks), (None, 20));
        let k = 1..=20;
        assert!(rt.program.times.iter().zip(k).all(|(t, k)| *t == k * T0), "das Raster: {:?}", rt.program.times);
        assert_eq!((rt.program.dispatched, pended.get()), (std::vec![6, 6, 6], 3));
    }

    /// In logischer Zeit stellt der Alarm keinen Zeitgeber, sondern
    /// schliesst das Tor; erst `release` laesst die ISR anstehen, und jeder
    /// Tick beginnt genau auf seiner Grenze.
    #[test]
    fn a_logical_alarm_waits_behind_the_gate_and_steps_on_the_grid() {
        static GATE: AtomicBool = AtomicBool::new(false);
        let trace: Line = Trace::new(Cadence::of(12, 1), T0, no_line);
        let clock: Time<Shared<'static>> = Time::Logical(0);
        let mut rt =
            Runtime::new(Counting::new(u64::MAX), clock, NoWatchdog, trace, Profile::SHARED, T0, Policy::Alert);
        let mut form = Form::start(&mut rt, Recorded::default(), Some(&GATE));
        let mut board = Recorded::default();
        let stats = loop {
            assert_eq!(form.alarm.armed, None, "der Zeitgeber bleibt in logischer Zeit unberuehrt");
            release(&GATE, &mut board);
            assert_eq!(board.armed.take(), Some(0), "das Tor war zu");
            if let Some(stats) = form.on_alarm(&mut rt, no_journal(), None) {
                break stats;
            }
        };
        assert!(!GATE.load(Ordering::Relaxed), "nach dem Ende wartet kein Alarm");
        release(&GATE, &mut board);
        assert_eq!(board.armed, None);
        let grid: Vec<i64> = (1..=12).map(|k| k * T0).collect();
        assert_eq!((stats.overruns, rt.program.times), (0, grid));
    }
}
