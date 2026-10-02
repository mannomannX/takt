//! Profilaufsatz `shared` (12.8): Takt als hoechstpriore Aufgabe unter einem
//! RTOS.
//!
//! **Was der Aufsatz vom RTOS braucht.** Eine Benachrichtigung je
//! Tickgrenze aus der Timer-ISR ([`Boundary`]) — mehr nicht. Die Aufgabe
//! fuer die Jobs (4.5) und die kritischen Abschnitte des Treiberrands sind
//! Mittel des RTOS selbst und stehen in der Bindung; hier stuenden sie nur
//! als Namen fuer dasselbe.
//!
//! **Ein Port ueber dem Kern ohne Warten** (12.11). Auf dem blanken Board
//! wartet der eigene Kern in `Clock::wait_until` auf den Timer
//! ([`takt_rt_baremetal::run`]). Unter einem RTOS hielte dasselbe Warten als
//! hoechstpriore Aufgabe jede Aufgabe darunter an. Die Takt-Aufgabe wartet
//! darum auf die Benachrichtigung und laesst den Kern rechnen, was faellig
//! ist ([`Runtime::service`]) — denselben Tick, denselben Watchdog-Takt im
//! Schlaf (9.9), dieselbe Ausgabe ([`Trace`]).
//!
//! **Die Zeitgarantie wird gemessen, nicht bewiesen** (12.8): Wie spaet die
//! Aufgabe nach der Grenze beginnt, steht in `drift` jedes Ticks (7.3);
//! ISRs darueber und kritische Abschnitte darunter bestimmen es.

#![no_std]

use core::cell::Cell;
use core::future::Future;

use takt_rt_baremetal::{Port, Stats, Telemetry, Trace};
use takt_rt_core::{Clock, Nvm, Persist, Program, Runtime, Watchdog};

/// Die Benachrichtigung je Tickgrenze, die die Timer-ISR der Takt-Aufgabe
/// gibt.
pub trait Boundary {
    /// Wartet, bis seit dem letzten Aufruf eine Grenze gemeldet ist.
    /// Mehrere Grenzen zaehlen wie eine: Die Aufgabe arbeitet danach alle
    /// ab, die faellig sind, und keine geht verloren (7.3).
    fn reached(&mut self) -> impl Future<Output = ()>;
}

/// Eine geliehene Benachrichtigung: Die Bindung behaelt sie ueber den Lauf
/// hinaus.
impl<B: Boundary> Boundary for &mut B {
    fn reached(&mut self) -> impl Future<Output = ()> {
        (**self).reached()
    }
}

/// Die logische Zeit unter einem RTOS, fuer Konformitaetslaeufe (13.8):
/// Jede gemeldete Grenze ist genau eine Periode weiter, und die Uhr steht
/// auf ihr. Ein Tick ist so nie zu spaet, auch wenn die Leitung den Trace
/// langsamer abnimmt, als der Timer zaehlt (FB-292); die Aufgaben darunter
/// rechnen dabei wie im Betrieb.
pub struct Logical<'a, B> {
    boundary: B,
    now: &'a Cell<i64>,
    tick_ns: i64,
    /// Die Zeit der naechsten gemeldeten Grenze; die erste ist Grenze 0.
    next: i64,
}

impl<'a, B: Boundary> Logical<'a, B> {
    /// Zaehlt die Grenzen von `boundary` in `now`.
    pub fn new(boundary: B, now: &'a Cell<i64>, tick_ns: i64) -> Logical<'a, B> {
        Logical { boundary, now, tick_ns, next: 0 }
    }
}

impl<B: Boundary> Boundary for Logical<'_, B> {
    async fn reached(&mut self) {
        self.boundary.reached().await;
        self.now.set(self.next);
        self.next = self.next.saturating_add(self.tick_ns);
    }
}

/// Die Uhr zu [`Logical`].
pub struct LogicalTime<'a>(pub &'a Cell<i64>);

impl Clock for LogicalTime<'_> {
    fn now(&self) -> i64 {
        self.0.get()
    }

    /// Unter einem RTOS wartet die Schleife nicht selbst ([`run`]); die
    /// Frist gilt dann als erreicht, wie in `LogicalClock`.
    fn wait_until(&mut self, deadline: i64) {
        self.0.set(self.0.get().max(deadline));
    }
}

/// Die Takt-Aufgabe (12.8, 12.11): Auf jede gemeldete Grenze rechnet der
/// Kern, was faellig ist, und bestaetigt im Schlaf den Watchdog; bis zur
/// Grenze der Senke oder bis das Programm seinen Lauf beendet (12.7). An
/// der Grenze schreibt das Journal synchron.
pub async fn run<G, C, W, F, N, P, B, const R: usize>(
    rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P, R>>>,
    mut persist: Option<&mut Persist<'_, N>>,
    boundary: &mut B,
) -> Stats
where
    G: Program,
    C: Clock,
    W: Watchdog,
    F: FnMut() -> Option<&'static mut Telemetry<P, R>>,
    N: Nvm,
    P: Port + 'static,
    B: Boundary,
{
    let limit = rt.sink.limit();
    while rt.ended().is_none() && (limit == 0 || rt.tick_number() < limit) {
        boundary.reached().await;
        match persist.as_deref_mut() {
            Some(p) => rt.service_persisting(p),
            None => rt.service(),
        };
    }
    takt_rt_baremetal::stats(rt, persist)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::pin;
    use core::task::{Context, Poll};
    use takt_rt_baremetal::Cadence;
    use takt_rt_core::{FakeNvm, Policy, Profile};

    extern crate std;

    const T0: i64 = 1_000_000;

    /// Treibt eine Zukunft, die nie wirklich wartet, bis zum Ende.
    fn block_on<F: Future>(f: F) -> F::Output {
        let mut f = pin!(f);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    /// Die Zeit des Timers; die Takt-Aufgabe liest sie, die ISR stellt sie.
    struct Timer<'a>(&'a Cell<i64>);

    impl Clock for Timer<'_> {
        fn now(&self) -> i64 {
            self.0.get()
        }

        fn wait_until(&mut self, _deadline: i64) {
            panic!("die Takt-Aufgabe wartet nie selbst");
        }
    }

    /// Jede Benachrichtigung ist eine Grenze weiter.
    struct Isr<'a>(&'a Cell<i64>);

    impl Boundary for Isr<'_> {
        fn reached(&mut self) -> impl Future<Output = ()> {
            self.0.set(self.0.get() + T0);
            core::future::ready(())
        }
    }

    #[derive(Default)]
    struct Kicks(u32);

    impl Watchdog for Kicks {
        fn kick(&mut self) {
            self.0 += 1;
        }
    }

    /// Zaehlt Ticks und Commits; schlaeft auf Wunsch bis `sleep_until`.
    #[derive(Default)]
    struct Counted {
        ticks: u64,
        commits: Cell<u32>,
        sleep_until: Option<i64>,
    }

    impl Program for Counted {
        fn tick(&mut self, _k: u64, _now: i64) {
            self.ticks += 1;
        }

        fn sleep_allowed(&self) -> bool {
            self.sleep_until.is_some()
        }

        fn next_deadline(&self) -> Option<i64> {
            self.sleep_until
        }

        fn commit(&mut self) {
            self.commits.set(self.commits.get() + 1);
        }
    }

    struct NoLine;

    impl Port for NoLine {
        fn try_write(&mut self, _b: u8) -> bool {
            true
        }
    }

    /// Eine Senke ohne Leitung.
    type Quiet = Trace<fn() -> Option<&'static mut Telemetry<NoLine, 8>>, Telemetry<NoLine, 8>>;

    fn quiet(limit: u64) -> Quiet {
        Trace::new(Cadence::of(limit, 1), T0, || None)
    }

    fn run_for(program: Counted, limit: u64) -> (Stats, Runtime<Counted, Timer<'static>, Kicks, Quiet>) {
        let now: &'static Cell<i64> = std::boxed::Box::leak(std::boxed::Box::new(Cell::new(0)));
        let mut rt =
            Runtime::new(program, Timer(now), Kicks::default(), quiet(limit), Profile::BAREMETAL, T0, Policy::Fault);
        let stats = block_on(run(&mut rt, None::<&mut Persist<'_, FakeNvm<0>>>, &mut Isr(now)));
        (stats, rt)
    }

    /// Je gemeldeter Grenze ein Tick und ein Commit, bis zur Grenze des
    /// Laufs; gewartet wird nur auf die Benachrichtigung.
    #[test]
    fn the_task_runs_one_tick_per_reported_boundary() {
        let (stats, rt) = run_for(Counted::default(), 5);
        assert_eq!((rt.program.ticks, rt.program.commits.get()), (5, 5));
        assert_eq!(rt.watchdog.0, 5, "nach jedem Tick bestaetigt");
        assert_eq!(stats.overruns, 0);
    }

    /// In logischer Zeit ist jede gemeldete Grenze genau eine Periode:
    /// kein Tick zu spaet, gleich wie lange der Timer dazwischen zaehlt.
    #[test]
    fn logical_time_advances_one_period_per_boundary() {
        let wall = Cell::new(0);
        let logical = Cell::new(0);
        let mut rt = Runtime::new(
            Counted::default(),
            LogicalTime(&logical),
            Kicks::default(),
            quiet(4),
            Profile::BAREMETAL,
            T0,
            Policy::Fault,
        );
        let stats =
            block_on(run(&mut rt, None::<&mut Persist<'_, FakeNvm<0>>>, &mut Logical::new(Isr(&wall), &logical, T0)));
        assert_eq!((rt.program.ticks, stats.overruns), (4, 0));
        assert_eq!(logical.get(), 3 * T0, "Tick k beginnt bei k * T0");
    }

    /// Im Schlaf (9.9) bestaetigt jede gemeldete Grenze den Watchdog, auch
    /// ohne Tick — die Aufgabe sieht, dass der Timer lebt (12.3).
    #[test]
    fn a_sleeping_task_still_kicks_the_watchdog_at_every_boundary() {
        let (stats, rt) = run_for(Counted { sleep_until: Some(10 * T0), ..Counted::default() }, 12);
        assert_eq!(stats.slept, 8, "Tick 0 schlaeft bis vor die Frist");
        assert!(rt.program.ticks >= 3, "{}", rt.program.ticks);
        let boundaries = rt.program.ticks + stats.slept;
        assert_eq!(u64::from(rt.watchdog.0), boundaries, "je Grenze einmal");
    }
}
