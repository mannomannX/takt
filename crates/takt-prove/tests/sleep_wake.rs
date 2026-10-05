//! Der Interpreter in der Schleife aus 12.1 mit Schlaf (9.9, Satz 9.9.1):
//! Was weckt, weckt in dem Tick, in dem der Lauf ohne Schlaf weitergeht.

use takt_diag::Policy as Diagnostics;
use takt_interp::{Run, RunOptions, Trace};
use takt_mir::Program;
use takt_rt_core::{Clock, Policy, Profile, Runtime, Sink, Tick, Watchdog};
use takt_sema::{Build, Options};

fn compile(src: &str) -> Program {
    let options = Options { policy: Diagnostics::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Eine Uhr, die nur springt, wenn die Schleife wartet.
struct Virtual(i64);

impl Clock for Virtual {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
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

/// Der Trace eines Laufs in der Schleife und die uebersprungenen Ticks.
fn looped(p: &Program, stimulus: &str, may_sleep: bool, ticks: u64) -> (String, u64) {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let run = Run::new(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let profile = Profile { may_sleep, ..Profile::LINUX_RT };
    let mut rt = Runtime::new(run, Virtual(0), Quiet, Slept::default(), profile, p.config.tick, Policy::default());
    while rt.tick_number() < ticks {
        rt.step();
    }
    let slept = rt.sink.0;
    (rt.program.finish().expect("Ergebnis").trace.render(), slept)
}

/// RT-018 (9.9 neu): Eine uebernommene Tunable-Aenderung ist ein
/// Weckereignis. Das System schlaeft im `idle`-Zustand, dessen Guard das
/// Tunable liest; die Aenderung in Tick 30 weckt es an dieser Tickgrenze,
/// und der Uebergang faellt in denselben Tick wie ohne Schlaf. Eine
/// verworfene Aenderung (ausserhalb der Range) aendert nichts.
#[test]
fn an_accepted_tunable_change_wakes_the_sleeping_system() {
    let p = compile(
        "system:
    language = 1
    tick = 1 ms

tunable param KP : int in 0..10 = 1
output led : bool @ hw(\"o/led\") with safe = false

machine m:
    initial WAIT
    state WAIT idle:
        enter:
            led = false
        when KP > 5: -> RUN
    state RUN:
        enter:
            led = true
",
    );
    for (stimulus, woken) in [("t=30 tune KP 7\n", Some("t=30 state m RUN")), ("t=30 tune KP 70\n", None)] {
        let (awake, none) = looped(&p, stimulus, false, 60);
        let (asleep, slept) = looped(&p, stimulus, true, 60);
        assert_eq!(none, 0, "{stimulus}");
        assert_eq!(awake, asleep, "Satz 9.9.1, `{stimulus}`");
        match woken {
            Some(line) => assert!(awake.contains(line), "`{line}`:\n{awake}"),
            None => assert!(!awake.contains("state m RUN") && awake.contains("rejected"), "{awake}"),
        }
        assert!(slept > 0, "`{stimulus}`: geschlafen wird:\n{asleep}");
    }
}
