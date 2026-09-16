//! Satz 9.9.1 (Schlaf ist unsichtbar) unter `linux_rt` (12.2, 12.8):
//! dasselbe Programm laeuft in der Schleife aus 12.1 mit und ohne Schlaf,
//! und die Traces sind byteidentisch. Das Programm ist der Interpreter
//! (`takt_interp::Run`), die Uhr die des Profils.

use takt_diag::Policy as Diagnostics;
use takt_interp::{Run, RunOptions, Trace};
use takt_rt_core::{Policy, Profile, Runtime, Sink, Tick, Watchdog};
use takt_rt_linux::RealtimeClock;
use takt_sema::{Build, Options};

/// Ein `idle`-Zustand mit Wake-Quelle und `after`-Frist (5.10, 9.9); die
/// Attribute des Tasters entscheiden, wann seine Abtastung veraltet.
fn program(button: &str) -> String {
    format!(
        "system:
    language = 1
    tick = 10 ms

input  button : bool @ hw(\"gpio/btn\") with {button}
output led    : bool @ hw(\"ui/led\")   with safe = false

machine m:
    initial SLEEP

    state SLEEP idle:
        enter:
            led = false
        when button: -> RUN
        after 500 ms: -> RUN

    state RUN:
        enter:
            led = true
        after 200 ms: -> SLEEP
"
    )
}

struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Zaehlt die uebersprungenen Ticks.
#[derive(Default)]
struct Slept(u64);

impl Sink for Slept {
    fn record(&mut self, tick: &Tick) {
        self.0 += tick.slept;
    }
}

/// Der Trace des Laufs und wie viele Ticks die Schleife uebersprungen hat.
fn traced(source: &str, stimulus: &str, may_sleep: bool, ticks: u64) -> (String, u64) {
    let options = Options { policy: Diagnostics::default(), build: Build::Sim, profile: None };
    let p = takt_sema::compile(source, &options).program.expect("Programm");
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let run = Run::new(&p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let profile = Profile { may_sleep, ..Profile::LINUX_RT };
    let clock = RealtimeClock::new();
    let mut rt = Runtime::new(run, clock, Quiet, Slept::default(), profile, p.config.tick, Policy::default());
    while rt.tick_number() < ticks {
        rt.step();
    }
    let slept = rt.sink.0;
    (rt.program.finish().expect("Ergebnis").trace.render(), slept)
}

#[test]
fn the_trace_with_sleep_is_the_trace_without() {
    let source = program("wake = true, max_age = 5 s");
    let stimulus = "t=0 in button false\nt=30 in button true\nt=32 in button false\n";
    let (awake, none) = traced(&source, stimulus, false, 120);
    let (asleep, slept) = traced(&source, stimulus, true, 120);
    assert_eq!(none, 0, "ohne `may_sleep` schlaeft die Schleife nie");
    assert_eq!(awake, asleep, "Satz 9.9.1: der Schlaf ist im Trace sichtbar");
    // Der Lauf tut etwas: Der Taster weckt (t=30), `after 200 ms` schlaeft
    // wieder ein (t=50), `after 500 ms` weckt aus dem Schlaf (t=100).
    for line in ["t=30 state m RUN", "t=50 state m SLEEP", "t=100 state m RUN", "t=120 state m SLEEP"] {
        assert!(awake.contains(line), "`{line}` fehlt:\n{awake}");
    }
    // Geschlafen wird bis zum Taster (28 Ticks) und bis zur Frist (49).
    assert_eq!(slept, 77, "{asleep}");
}

/// Eine Wake-Quelle, deren Abtastung veraltet, weckt: Der Guard faultet im
/// selben Tick wie ohne Schlaf (3.5, 12.6). Ohne `max_age` gilt das
/// Doppelte der Periode, also veraltet der Taster im Tick 3.
#[test]
fn a_stale_wake_source_wakes_the_loop() {
    let source = program("wake = true");
    let stimulus = "t=0 in button false\n";
    let (awake, _) = traced(&source, stimulus, false, 40);
    let (asleep, slept) = traced(&source, stimulus, true, 40);
    assert_eq!(awake, asleep, "Satz 9.9.1: der Schlaf ist im Trace sichtbar");
    assert!(awake.contains("t=3 fault m SensorFault"), "{awake}");
    // Nur Tick 2 wird uebersprungen; ab dem Fault ist die Maschine nicht idle.
    assert_eq!(slept, 1, "{asleep}");
}
