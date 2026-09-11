//! Die Tickschleife (12.1) und ihre Zusagen.

use core::cell::RefCell;

use takt_rt_core::{Clock, Overrun, Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};

/// Eine Uhr, die nur tut, was der Test ihr sagt.
#[derive(Default)]
struct Fake {
    now: i64,
    /// Wie lange ein Schritt dauert; je Tick einer, dann der letzte weiter.
    costs: Vec<i64>,
    /// Wohin `wait_until` gesprungen ist.
    waits: Vec<i64>,
}

/// Die Uhr wird von Programm und Schleife zugleich gebraucht.
struct Shared<'a>(&'a RefCell<Fake>);

impl Clock for Shared<'_> {
    fn now(&self) -> i64 {
        self.0.borrow().now
    }

    fn wait_until(&mut self, deadline: i64) {
        let mut f = self.0.borrow_mut();
        f.waits.push(deadline);
        // Eine echte Uhr springt nicht zurueck: Ein ueberfaelliger Tick
        // beginnt sofort, nicht in der Vergangenheit (12.2).
        f.now = f.now.max(deadline);
    }
}

/// Ein Programm, das die Uhr um die vorgesehene Schrittdauer vorstellt.
struct Counted<'a> {
    clock: &'a RefCell<Fake>,
    ticks: Vec<(u64, i64)>,
    overruns: u32,
    sleepy: Option<i64>,
}

impl Program for Counted<'_> {
    fn tick(&mut self, k: u64, now: i64) {
        self.ticks.push((k, now));
        let mut f = self.clock.borrow_mut();
        let cost = *f.costs.get(k as usize).or(f.costs.last()).unwrap_or(&0);
        f.now += cost;
    }

    fn raise_overrun(&mut self) {
        self.overruns += 1;
    }

    fn sleep_allowed(&self) -> bool {
        self.sleepy.is_some()
    }

    fn next_deadline(&self) -> Option<i64> {
        self.sleepy
    }
}

#[derive(Default)]
struct Kicks(u32);

impl Watchdog for Kicks {
    fn kick(&mut self) {
        self.0 += 1;
    }
}

#[derive(Default)]
struct Log(Vec<Tick>);

impl Sink for Log {
    fn record(&mut self, tick: &Tick) {
        self.0.push(*tick);
    }
}

const T0: i64 = 1_000_000; // 1 ms

/// Die logische Zeit ist ein Vielfaches von T0 — unabhaengig davon, was die
/// Uhr sagt. Ohne diese Trennung haengt der Trace an der Hardware, und
/// Satz 9.4.1 gilt auf der Box nicht mehr.
#[test]
fn logical_time_does_not_follow_the_clock() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0, 700_000, 3_000_000, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(4);
    let seen: Vec<i64> = rt.program.ticks.iter().map(|(_, now)| *now).collect();
    assert_eq!(seen, vec![T0, 2 * T0, 3 * T0, 4 * T0], "auch nach einem Overrun");
}

/// 7.3: „Der Tick wird nie uebersprungen."
#[test]
fn an_overrun_never_skips_a_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(3);
    let ks: Vec<u64> = rt.program.ticks.iter().map(|(k, _)| *k).collect();
    assert_eq!(ks, vec![0, 1, 2], "die Tickzahl laeuft luckenlos weiter");
}

/// 7.3: Der Fault wirkt im *naechsten* Tick.
#[test]
fn an_overrun_faults_in_the_following_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0, 0, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    assert_eq!(rt.program.overruns, 0, "im Tick der Ueberschreitung noch nicht");
    rt.step();
    assert_eq!(rt.program.overruns, 1, "im naechsten Tick");
    rt.step();
    assert_eq!(rt.program.overruns, 1, "und nur einmal");
}

/// 7.3: `alert` statt `fault` fuer unkritische Systeme.
#[test]
fn the_alert_policy_raises_no_fault() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Alert);
    rt.run(3);
    assert_eq!(rt.program.overruns, 0);
    assert!(rt.sink.0.iter().any(|t| t.overrun), "gemeldet wird sie trotzdem");
}

/// 12.2: absolute Deadlines — ein zu spaeter Tick verschiebt die folgenden
/// nicht.
#[test]
fn deadlines_are_absolute() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0, 2_500_000, 0, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(4);
    assert_eq!(clock.borrow().waits, vec![0, T0, 2 * T0, 3 * T0], "die Frist waechst um T0, nicht um die Dauer");
}

/// 12.4: Der Watchdog wird nach dem Schritt bestaetigt, nicht davor —
/// sonst bestaetigte er einen Tick, der noch nicht durchgelaufen ist.
#[test]
fn the_watchdog_is_kicked_once_per_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(5);
    assert_eq!(rt.watchdog.0, 5);
}

// --- Schlaf (9.9) -------------------------------------------------------

/// Satz 9.9.1: Der Schlaf ist unsichtbar — die uebersprungenen Ticks
/// waeren leere Schritte gewesen, `now` rueckt exakt um sie vor.
#[test]
fn sleeping_advances_logical_time_exactly() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    // Die naechste Frist liegt bei 10 ms; der erste Tick endet bei 1 ms.
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    let first = rt.step();
    assert_eq!(first.slept, 8, "von 1 ms bis 10 ms sind acht Ticks zu ueberspringen");
    // Der naechste ausgefuehrte Tick ist der an der Frist.
    let second = rt.step();
    assert_eq!(second.now, 10 * T0, "der Tick an der Frist wird ausgefuehrt");
}

/// Ohne erlaubten Schlaf wird nicht geschlafen — der Default eines
/// Programms, das die Bedingung nicht prueft.
#[test]
fn a_program_that_does_not_allow_sleep_never_sleeps() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 0);
}

/// 12.8: Ein Startprogramm schlaeft nicht, auch wenn es duerfte.
#[test]
fn the_boot_profile_never_sleeps() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BOOT, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 0);
}

/// Eine Frist im naechsten Tick ist kein Grund zu schlafen.
#[test]
fn a_deadline_in_the_next_tick_is_no_reason_to_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, sleepy: Some(2 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 0);
}

// --- Profile (12.8) -----------------------------------------------------

#[test]
fn every_profile_of_the_reference_has_a_name() {
    for name in ["linux_rt", "baremetal", "rtos", "boot"] {
        let p = Profile::by_name(name).unwrap_or_else(|| panic!("Profil `{name}` fehlt"));
        assert_eq!(p.name(), name, "der Name geht in den Lauf-Header (11.3)");
    }
    assert!(Profile::by_name("sim").is_none(), "`sim` ist ein Build, kein Laufzeitprofil");
}

// --- Die Ueberlaufmessung selbst ----------------------------------------

#[test]
fn overrun_records_the_worst_case() {
    let mut o = Overrun::new(Policy::Fault);
    assert!(!o.observe(T0, T0).over, "genau die Periode ist kein Ueberlauf");
    assert!(o.observe(3 * T0, T0).fault);
    assert!(o.observe(2 * T0, T0).fault);
    assert_eq!(o.count, 2);
    assert_eq!(o.worst, 2 * T0, "die groesste Ueberschreitung, nicht die letzte");
}
