//! Der Port `linux_rt` (12.2, 12.11): Tick-Faden, Job-Faden und ihre
//! Uebergabe, mit Programmen, die nur zaehlen. Die Tests laufen auf jedem
//! Rechner und in Echtzeit; die Perioden sind so lang, dass ein
//! gewoehnlicher Scheduler sie haelt. Was nur Linux zusagt, steht im Befund.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use takt_rt_core::{NextRun, Policy, Profile, Program, Runtime, Volatile, Watchdog};
use takt_rt_linux::RealtimeClock;
use takt_rt_linux::port::{self, Config};

/// Kein Watchdog.
struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Der Job-Kontext, wie ihn der Rahmen fuehrt (4.5): Ein Auftrag ist
/// gegeben, solange `busy` steht, und nur der ruhende Kontext bekommt einen.
#[derive(Default)]
struct Context {
    busy: AtomicBool,
    finished: AtomicU32,
}

/// Startet in Tick 0 drei Jobs und zaehlt in Tick 1, wie viele fertig sind;
/// merkt sich, in welchem Faden jeder Schritt rechnete.
struct Queue {
    context: Arc<Context>,
    waiting: u32,
    seen: Option<u32>,
    threads: Vec<Option<String>>,
}

impl Program for Queue {
    type Memory = Volatile;

    fn tick(&mut self, k: u64, _now: i64) {
        self.threads.push(std::thread::current().name().map(str::to_string));
        if k == 0 {
            self.waiting = 3;
        }
        if k == 1 {
            self.seen = Some(self.context.finished.load(Ordering::SeqCst));
        }
    }

    fn dispatch_job(&mut self) -> bool {
        if self.waiting == 0 || self.context.busy.load(Ordering::SeqCst) {
            return false;
        }
        self.waiting -= 1;
        self.context.busy.store(true, Ordering::SeqCst);
        true
    }
}

/// **Jeder Job eines Ticks rechnet vor der naechsten Grenze** (12.11,
/// Portvertrag 3): Tick 0 startet drei Jobs, der Job-Kontext nimmt einen zur
/// Zeit. Ist einer fertig, weckt der Job-Faden den Tick-Faden, und `service`
/// gibt vor der Frist den naechsten aus; Tick 1 sieht alle drei fertig. Ohne
/// den Wecker saehe er einen. Der Schritt rechnet im Tick-Faden, die Jobs im
/// Job-Faden, und die Fristen sind absolut: drei Ticks zu 100 ms enden nicht
/// vor 200 ms.
#[test]
fn every_job_of_a_tick_runs_before_the_next_boundary() {
    const T0: i64 = 100_000_000;
    let config = Config::new(0, 0).ticks(3);
    let context = Arc::new(Context::default());
    let started = Instant::now();
    let (served, seen, threads) = port::spawn(&config, |guarantee| {
        let program = Queue { context: Arc::clone(&context), waiting: 0, seen: None, threads: Vec::new() };
        let clock = RealtimeClock::under(guarantee);
        let mut rt = Runtime::new(program, clock, Quiet, (), Profile::LINUX_RT, T0, Policy::Fault);
        let job = Arc::clone(&context);
        let work = move || {
            assert_eq!(std::thread::current().name(), Some("takt-job"), "der Job rechnet im Job-Faden");
            if job.busy.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(2));
                job.finished.fetch_add(1, Ordering::SeqCst);
                job.busy.store(false, Ordering::SeqCst);
            }
        };
        let served = port::serve(&mut rt, Some(work), &config).expect("Job-Faden");
        (served, rt.program.seen, rt.program.threads.clone())
    })
    .expect("Tick-Faden");
    assert_eq!(seen, Some(3), "Tick 1 sieht jeden Job aus Tick 0 fertig");
    assert_eq!(served.jobs, 3);
    assert_eq!((served.ticks, served.ended), (3, None), "drei Ticks, dann endet der Lauf");
    assert!(
        threads.iter().all(|t| t.as_deref() == Some("takt-tick")),
        "der Schritt rechnet im Tick-Faden: {threads:?}"
    );
    assert!(started.elapsed() >= Duration::from_millis(200), "die Fristen wurden abgewartet");
}

/// Beendet den Lauf nach seinem zweiten Tick (12.7).
struct Ending(u64);

impl Program for Ending {
    type Memory = Volatile;

    fn tick(&mut self, k: u64, _now: i64) {
        self.0 = k + 1;
    }

    fn next_run(&self) -> Option<NextRun> {
        (self.0 >= 2).then_some(NextRun::After(5_000_000_000))
    }
}

/// **Der Lauf endet, wenn das Programm ihn beendet** (12.7): ohne Grenze der
/// Ticks und ohne Job-Kontext; der Port nennt das Ende und wie weit er kam.
#[test]
fn the_run_ends_when_the_program_ends_it() {
    let config = Config::new(0, 0);
    let served = port::spawn(&config, |guarantee| {
        let clock = RealtimeClock::under(guarantee);
        let mut rt = Runtime::new(Ending(0), clock, Quiet, (), Profile::LINUX_RT, 10_000_000, Policy::Fault);
        port::serve(&mut rt, None::<fn()>, &config).expect("ohne Job-Faden")
    })
    .expect("Tick-Faden");
    assert_eq!(served.ended, Some(NextRun::After(5_000_000_000)));
    assert_eq!((served.ticks, served.jobs), (2, 0));
}

/// Faultet im Schritt von Tick 1, waehrend der Job-Faden ruht.
struct Panicking;

impl Program for Panicking {
    type Memory = Volatile;

    fn tick(&mut self, k: u64, _now: i64) {
        assert!(k < 1, "Absicht: der Schritt bricht ab");
    }
}

/// **Eine Panik im Tick-Faden kommt beim Aufrufer an** und haelt nicht: Der
/// Job-Faden, der auf Arbeit wartet, endet mit der Schleife, und der
/// Aufrufer sieht die Meldung des Schritts.
#[test]
#[should_panic(expected = "Absicht")]
fn a_panic_in_the_tick_thread_reaches_the_caller() {
    let config = Config::new(0, 0).ticks(5);
    let _ = port::spawn(&config, |guarantee| {
        let clock = RealtimeClock::under(guarantee);
        let mut rt = Runtime::new(Panicking, clock, Quiet, (), Profile::LINUX_RT, 10_000_000, Policy::Fault);
        port::serve(&mut rt, Some(|| {}), &config)
    });
}
