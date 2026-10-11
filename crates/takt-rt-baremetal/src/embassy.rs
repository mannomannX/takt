//! Die Form `rtos` unter Embassy (12.11, `plan/m11.md` 2.4; M11 Schritt
//! 14d): Takt als Aufgabe eines Executors hoher Prioritaet. Sie wartet mit
//! `embassy-time` auf die Frist, die `service` nennt, rechnet dort den
//! Schritt und nach jedem Auftrag des Job-Kontexts das Verteilen — die
//! Interruptform ([`crate::interrupt`]) mit dem Planer von Embassy, wie
//! unter RTIC.
//!
//! Was das Board beitraegt, ist der Zeittreiber von `embassy-time` auf dem
//! Zeitgeber der Interruptform und der Interrupt, der die Jobs rechnet. Die
//! Uhr der Runtime liest dieselbe Zeitachse wie der Treiber: Eine Frist von
//! `service` in Nanosekunden ist ein Zeitpunkt von `embassy-time`.
//!
//! In logischer Zeit (13.8) wartet die Aufgabe hinter dem Tor der
//! Interruptform: Sie hat keine Frist, bis die Hauptschleife des Wirts das
//! Tor oeffnet ([`crate::interrupt::release`]).

use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use takt_rt_core::{Clock, Program, Runtime, Store, Tunables, Watchdog};

use crate::interrupt::{Alarm, Form};
use crate::run::{Stats, Trace};
use crate::telemetry::{Port, Telemetry};

/// Was die Takt-Aufgabe weckt: eine neue Frist und ein gerechneter Auftrag
/// des Job-Kontexts.
#[derive(Default)]
pub struct Wake {
    deadline: Signal<CriticalSectionRawMutex, i64>,
    job_done: Signal<CriticalSectionRawMutex, ()>,
}

impl Wake {
    /// Ohne Frist und ohne Auftrag; `const` fuer einen statischen.
    pub const fn new() -> Wake {
        Wake { deadline: Signal::new(), job_done: Signal::new() }
    }

    /// Aus dem Job-Kontext: Der Auftrag ist gerechnet, die Aufgabe verteilt
    /// den naechsten (4.5, FB-512).
    pub fn job_done(&self) {
        self.job_done.signal(());
    }
}

/// Der Alarm der Takt-Aufgabe: Er stellt keinen Zeitgeber, sondern nennt
/// der Aufgabe die Frist; die Jobs laesst `pend_jobs` anstehen wie in der
/// Interruptform.
pub struct TaskAlarm<J> {
    wake: &'static Wake,
    pend_jobs: J,
}

impl<J: FnMut()> TaskAlarm<J> {
    /// Der Alarm ueber `wake`; `pend_jobs` laesst den Job-Kontext rechnen.
    pub fn new(wake: &'static Wake, pend_jobs: J) -> TaskAlarm<J> {
        TaskAlarm { wake, pend_jobs }
    }
}

impl<J: FnMut()> Alarm for TaskAlarm<J> {
    fn arm(&mut self, at: i64) {
        self.wake.deadline.signal(at);
    }

    fn pend_jobs(&mut self) {
        (self.pend_jobs)();
    }
}

/// Der Zeitpunkt von `embassy-time` zu einer Frist in Nanosekunden seit dem
/// Start der Zeitachse.
fn instant(at: i64) -> Instant {
    Instant::from_nanos(u64::try_from(at).unwrap_or(0))
}

/// Die Takt-Aufgabe: an der Frist der Schritt, nach einem Auftrag des
/// Job-Kontexts das Verteilen, danach `after` (etwa die Leitung); am Ende
/// des Laufs die Bilanz. `form` hat seine erste Frist schon gestellt
/// ([`Form::start`]).
pub async fn run<G, C, W, F, S, P, J>(
    rt: &mut Runtime<G, C, W, Trace<F, Telemetry<P>>, S>,
    form: &mut Form<TaskAlarm<J>>,
    wake: &'static Wake,
    mut tunables: Option<&mut dyn Tunables>,
    mut after: impl FnMut(),
) -> Stats
where
    G: Program,
    C: Clock,
    W: Watchdog,
    F: FnMut() -> Option<&'static mut Telemetry<P>>,
    S: Store,
    P: Port + 'static,
    J: FnMut(),
{
    // Ohne gemeldete Frist wartet die Aufgabe nur auf ihre Signale: in
    // logischer Zeit, bis das Tor sich oeffnet.
    let mut at: Option<i64> = None;
    loop {
        let timer = Timer::at(at.map_or(Instant::MAX, instant));
        match select3(timer, wake.deadline.wait(), wake.job_done.wait()).await {
            Either3::First(()) => {
                // Die naechste Frist meldet `on_alarm` selbst, ueber `arm`.
                at = None;
                if let Some(stats) = form.on_alarm(rt, reborrow(&mut tunables)) {
                    return stats;
                }
            }
            Either3::Second(next) => at = Some(next),
            Either3::Third(()) => form.on_job_done(rt, reborrow(&mut tunables)),
        }
        after();
    }
}

/// Leiht die Tunables fuer einen Aufruf erneut.
fn reborrow<'a>(tunables: &'a mut Option<&mut dyn Tunables>) -> Option<&'a mut dyn Tunables> {
    match tunables {
        Some(t) => Some(&mut **t),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::vec::Vec;

    use embassy_futures::select::{Either, select};
    use embassy_futures::{block_on, yield_now};
    use takt_rt_core::{NextRun, Outputs, Policy, Profile};

    use super::*;
    use crate::interrupt::release;
    use crate::run::{Cadence, NoWatchdog};

    const T0: i64 = 1_000_000;

    /// Zaehlt die Ticks und beendet ab `at` den Lauf; in Tick `start.0`
    /// beginnen `start.1` Jobs, und je Verteilung merkt es sich, nach wie
    /// vielen Ticks sie geschah.
    struct Counting {
        at: u64,
        start: (u64, u32),
        waiting: u32,
        dispatched: Vec<u64>,
        ticks: u64,
    }

    impl Program for Counting {
        type Memory = takt_rt_core::Volatile;

        fn tick(&mut self, k: u64, _now: i64) {
            self.ticks = k + 1;
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

    /// Logische Zeit: Sie steht mit jeder Frist auf ihr.
    struct Logical(i64);

    impl Clock for Logical {
        fn now(&self) -> i64 {
            self.0
        }

        fn wait_until(&mut self, deadline: i64) {
            self.0 = self.0.max(deadline);
        }
    }

    type Line = Trace<fn() -> Option<&'static mut Telemetry<NoLine>>, Telemetry<NoLine>>;

    fn no_line() -> Option<&'static mut Telemetry<NoLine>> {
        None
    }

    /// **Unter Embassy rechnet die Takt-Aufgabe jede Grenze, in logischer
    /// Zeit hinter dem Tor** (12.11, M11 Schritt 14d): Die Hauptschleife gibt
    /// die Frist frei, sobald sie laeuft; ein Job-Interrupt rechnet, was die
    /// Aufgabe anstossen laesst, und meldet es ihr. Zwei Jobs eines Ticks sind
    /// im selben Tick verteilt (FB-512), und der Lauf endet mit seiner Bilanz.
    #[test]
    fn the_task_steps_every_boundary_and_hands_out_every_job() {
        static WAKE: Wake = Wake::new();
        static GATE: AtomicBool = AtomicBool::new(false);
        static PENDING: AtomicBool = AtomicBool::new(false);
        static WORKED: AtomicU32 = AtomicU32::new(0);

        let program = Counting { at: 12, start: (3, 2), waiting: 0, dispatched: Vec::new(), ticks: 0 };
        let trace: Line = Trace::new(Cadence::of(0, 1), T0, no_line);
        let mut rt = Runtime::new(program, Logical(0), NoWatchdog, trace, Profile::SHARED, T0, Policy::Fault);
        let alarm = TaskAlarm::new(&WAKE, || PENDING.store(true, Ordering::Release));
        let mut form = Form::start(&mut rt, alarm, Some(&GATE));
        let task = run(&mut rt, &mut form, &WAKE, None, || {});
        // Die Hauptschleife, und mit ihr der Job-Interrupt: Er rechnet, bevor
        // sie weiterlaeuft.
        let host = async {
            let mut gate = TaskAlarm::new(&WAKE, || {});
            loop {
                if PENDING.swap(false, Ordering::AcqRel) {
                    WORKED.fetch_add(1, Ordering::Relaxed);
                    WAKE.job_done();
                } else {
                    release(&GATE, &mut gate);
                }
                yield_now().await;
            }
        };
        let stats = match block_on(select(task, host)) {
            Either::First(stats) => stats,
            Either::Second(()) => unreachable!("die Hauptschleife endet nicht"),
        };
        assert_eq!(stats.next_run, Some(NextRun::OnStart));
        assert_eq!(rt.program.ticks, 12, "jede Grenze bis zum Ende");
        assert_eq!(rt.program.dispatched, [4, 4], "beide Jobs im Tick ihres Starts verteilt");
        assert_eq!(WORKED.load(Ordering::Relaxed), 2);
    }
}
