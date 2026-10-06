//! Weckereignisse im Schlaf (9.9, Satz 9.9.1; FB-388, RT-001, RT-018): Ein
//! Ereignis, das das Programm nicht vorher kennt, beendet den Schlaf an der
//! Grenze danach, und der Trace gleicht dem eines Laufs ohne Schlaf.

use core::cell::RefCell;

use takt_rt_core::{Clock, Policy, Profile, Program, Runtime, Sink, Tick, Tunables, Watchdog};

const T0: i64 = 1_000_000;

/// Eine Uhr mit Weckereignissen zu festen Zeiten.
#[derive(Default)]
struct Fake {
    now: i64,
    /// Wann eine Wake-Quelle ausloest.
    events: Vec<i64>,
    /// Bis wohin `woken` schon geantwortet hat.
    seen: i64,
}

struct Shared<'a>(&'a RefCell<Fake>);

impl Clock for Shared<'_> {
    fn now(&self) -> i64 {
        self.0.borrow().now
    }

    fn wait_until(&mut self, deadline: i64) {
        let mut f = self.0.borrow_mut();
        f.now = f.now.max(deadline);
    }

    fn woken(&mut self) -> bool {
        let mut f = self.0.borrow_mut();
        let (seen, now) = (f.seen, f.now);
        f.seen = now;
        f.events.iter().any(|e| *e > seen && *e <= now)
    }
}

/// Eine Maschine in einem `idle`-Zustand mit `after 50 ms`, einer
/// Wake-Quelle (Taster) und einem Guard auf ein Tunable (`when GAIN > 5`).
/// Sie kennt keine Eingabe im Voraus: Den Taster tastet sie zu Tickbeginn
/// an der Uhr ab. Ihr Trace sind ihre Uebergaenge.
struct Idle<'a> {
    clock: &'a RefCell<Fake>,
    idle: bool,
    /// `time_in_state` in Nanosekunden.
    in_state: i64,
    gain: i64,
    log: Vec<String>,
}

impl<'a> Idle<'a> {
    fn new(clock: &'a RefCell<Fake>) -> Idle<'a> {
        Idle { clock, idle: true, in_state: 0, gain: 0, log: Vec::new() }
    }
}

impl Program for Idle<'_> {
    fn tick(&mut self, k: u64, _now: i64) {
        if !self.idle {
            return;
        }
        let at = self.clock.borrow().now;
        let pressed = self.clock.borrow().events.iter().any(|e| *e <= at);
        self.in_state += T0;
        let why = if pressed {
            "Taster"
        } else if self.gain > 5 {
            "GAIN"
        } else if self.in_state >= 50 * T0 {
            "after"
        } else {
            return;
        };
        self.idle = false;
        self.log.push(format!("t={k} {why}"));
    }

    fn sleep_allowed(&self) -> bool {
        self.idle
    }

    fn next_deadline(&self) -> Option<i64> {
        // Die Frist von `after`, absolut: der Beginn des Zustands plus 50 ms.
        self.idle.then_some(50 * T0)
    }

    fn advance(&mut self, ticks: u64) {
        self.in_state += T0 * ticks as i64;
    }

    fn tune(&mut self, _k: u64, _param: u32, value: &[u8]) {
        self.gain = value.first().copied().map_or(0, i64::from);
    }
}

/// Dieselbe Maschine mit einer Wake-Quelle, die das Programm selbst
/// abtastet (`Program::woken`): ein Pegel, der ab `high_from` steht, wie
/// ihn ein Treiber an der Grenze liest. Die Uhr weiss davon nichts.
struct Level {
    high_from: i64,
    idle: bool,
    in_state: i64,
    gain: i64,
    log: Vec<String>,
    /// Die Grenzen, an denen der Kern gefragt hat.
    polls: Vec<u64>,
}

impl Level {
    fn new(high_from: i64) -> Level {
        Level { high_from, idle: true, in_state: 0, gain: 0, log: Vec::new(), polls: Vec::new() }
    }

    /// Der Pegel, wie der Schritt von Tick `k` ihn an seiner Grenze liest.
    fn high(&self, k: u64) -> bool {
        k as i64 * T0 >= self.high_from
    }
}

impl Program for Level {
    fn tick(&mut self, k: u64, _now: i64) {
        if !self.idle {
            return;
        }
        self.in_state += T0;
        let why = if self.high(k) {
            "Pegel"
        } else if self.gain > 5 {
            "GAIN"
        } else if self.in_state >= 50 * T0 {
            "after"
        } else {
            return;
        };
        self.idle = false;
        self.log.push(format!("t={k} {why} gain={}", self.gain));
    }

    fn sleep_allowed(&self) -> bool {
        self.idle
    }

    fn next_deadline(&self) -> Option<i64> {
        self.idle.then_some(50 * T0)
    }

    fn advance(&mut self, ticks: u64) {
        self.in_state += T0 * ticks as i64;
    }

    fn tune(&mut self, _k: u64, _param: u32, value: &[u8]) {
        self.gain = value.first().copied().map_or(0, i64::from);
    }

    fn woken(&mut self, k: u64) -> bool {
        self.polls.push(k);
        self.high(k)
    }

    fn wake_sources(&self) -> bool {
        true
    }
}

/// Ein Tunable-Satz an einer Grenze.
struct Changes(Vec<(u64, u8)>);

impl Tunables for Changes {
    fn poll(&mut self, k: u64, apply: &mut dyn FnMut(u32, &[u8])) {
        for (_, v) in self.0.iter().filter(|(at, _)| *at == k) {
            apply(0, &[*v]);
        }
    }
}

struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

#[derive(Default)]
struct Slept(u64);

impl Sink for Slept {
    fn record(&mut self, tick: &Tick) {
        self.0 += tick.slept;
    }
}

/// Der eigene Kern ueber `ticks` Ticks: Trace und geschlafene Ticks.
fn own_core(events: &[i64], changes: &[(u64, u8)], may_sleep: bool, ticks: u64) -> (Vec<String>, u64) {
    let clock = RefCell::new(Fake { events: events.to_vec(), ..Fake::default() });
    let profile = Profile { may_sleep, ..Profile::BAREMETAL };
    let mut rt = Runtime::new(Idle::new(&clock), Shared(&clock), Quiet, Slept::default(), profile, T0, Policy::Fault);
    let mut tunables = Changes(changes.to_vec());
    while rt.tick_number() < ticks {
        rt.step_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, Some(&mut tunables));
    }
    let slept = rt.sink.0;
    (rt.program.log.clone(), slept)
}

/// **Ein Taster im Schlaf weckt an der Grenze danach** (9.9, RT-001,
/// FB-388): Er loest zwischen den Grenzen 13 und 14 aus; Tick 14 sieht ihn,
/// mit Schlaf wie ohne, und geschlafen wurden die Ticks 1 bis 13.
#[test]
fn a_wake_source_ends_the_sleep_at_the_next_boundary() {
    let events = [13 * T0 + T0 / 2];
    let (awake, none) = own_core(&events, &[], false, 60);
    let (asleep, slept) = own_core(&events, &[], true, 60);
    assert_eq!(none, 0);
    assert_eq!(awake, ["t=14 Taster"]);
    assert_eq!(asleep, awake, "Satz 9.9.1");
    assert_eq!(slept, 13, "Tick 1 bis 13; ohne Weckereignis waeren es 48 bis zur Frist");
}

/// **Eine Tunable-Aenderung im Schlaf weckt an ihrer Grenze** (9.9 neu,
/// RT-018): `GAIN = 7` an Grenze 20 laesst Tick 20 den Guard sehen, mit
/// Schlaf wie ohne.
#[test]
fn a_tunable_change_ends_the_sleep_at_its_boundary() {
    let changes = [(20, 7)];
    let (awake, _) = own_core(&[], &changes, false, 60);
    let (asleep, slept) = own_core(&[], &changes, true, 60);
    assert_eq!(awake, ["t=20 GAIN"]);
    assert_eq!(asleep, awake, "Satz 9.9.1");
    assert_eq!(slept, 19);
}

/// Ohne Ereignis schlaeft der Kern bis zur Frist, und `after` feuert dort.
#[test]
fn without_an_event_the_sleep_lasts_until_the_deadline() {
    let (awake, _) = own_core(&[], &[], false, 60);
    let (asleep, slept) = own_core(&[], &[], true, 60);
    assert_eq!(awake, ["t=49 after"]);
    assert_eq!(asleep, awake);
    assert_eq!(slept, 48);
}

/// **Im Kern ohne Warten verkuerzt ein Weckereignis die Frist** (12.11,
/// 9.9): Ruft der Port beim Ereignis, nennt `service` die Grenze danach
/// statt der Frist des Schlafs, und dort rechnet der Tick, der es sieht.
#[test]
fn a_wake_event_shortens_the_deadline_service_names() {
    let clock = RefCell::new(Fake { events: vec![13 * T0 + T0 / 2], ..Fake::default() });
    let mut rt =
        Runtime::new(Idle::new(&clock), Shared(&clock), Quiet, Slept::default(), Profile::BAREMETAL, T0, Policy::Fault);
    assert_eq!(rt.service().deadline, 49 * T0, "Tick 0, dann Schlaf bis zur Frist");
    clock.borrow_mut().now = 13 * T0 + T0 / 2;
    assert_eq!(rt.service().deadline, 14 * T0, "das Ereignis zieht die Frist vor");
    clock.borrow_mut().now = 14 * T0;
    rt.service();
    assert_eq!(rt.program.log, ["t=14 Taster"]);
    assert_eq!(rt.sink.0, 13);
}

/// **Ruft der Port erst an der Frist, findet `service` die Grenze der
/// Aenderung** (8.4, 9.9): Die Grenzen im Schlaf werden in Reihenfolge
/// abgefragt; ab der ersten mit einer Aenderung rechnet der Kern wach nach.
#[test]
fn a_late_service_finds_the_boundary_of_the_change() {
    let clock = RefCell::new(Fake::default());
    let mut rt =
        Runtime::new(Idle::new(&clock), Shared(&clock), Quiet, Slept::default(), Profile::BAREMETAL, T0, Policy::Fault);
    let mut tunables = Changes(vec![(20, 7)]);
    let next = rt.service_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, Some(&mut tunables));
    assert_eq!(next.deadline, 49 * T0);
    clock.borrow_mut().now = 49 * T0;
    rt.service_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, Some(&mut tunables));
    assert_eq!(rt.program.log, ["t=20 GAIN"]);
    assert_eq!(rt.sink.0, 19, "geschlafen bis vor die Aenderung");
}

/// Der eigene Kern mit [`Level`] ueber `ticks` Ticks: Trace, geschlafene
/// Ticks und die Grenzen, an denen er das Programm gefragt hat.
fn own_core_level(high_from: i64, may_sleep: bool, ticks: u64) -> (Vec<String>, u64, Vec<u64>) {
    let clock = RefCell::new(Fake::default());
    let profile = Profile { may_sleep, ..Profile::BAREMETAL };
    let mut rt =
        Runtime::new(Level::new(high_from), Shared(&clock), Quiet, Slept::default(), profile, T0, Policy::Fault);
    while rt.tick_number() < ticks {
        rt.step();
    }
    let slept = rt.sink.0;
    (rt.program.log.clone(), slept, rt.program.polls.clone())
}

/// **Eine Wake-Quelle des Programms weckt an der Grenze danach** (9.9,
/// FB-388): Der Pegel steht ab 13,5 Perioden; der Kern fragt das Programm
/// an jeder geschlafenen Grenze, einmal und in Reihenfolge, und Tick 14
/// sieht ihn — mit Schlaf wie ohne.
#[test]
fn a_wake_source_of_the_program_ends_the_sleep_at_the_next_boundary() {
    let high_from = 13 * T0 + T0 / 2;
    let (awake, none, unasked) = own_core_level(high_from, false, 60);
    let (asleep, slept, polls) = own_core_level(high_from, true, 60);
    assert_eq!((none, unasked), (0, Vec::new()), "wach fragt der Kern nicht");
    assert_eq!(awake, ["t=14 Pegel gain=0"]);
    assert_eq!(asleep, awake, "Satz 9.9.1");
    assert_eq!(slept, 13);
    assert_eq!(polls, (1..=14).collect::<Vec<u64>>());
}

/// **Wer Wake-Quellen abtastet, kommt im Schlaf an jeder Grenze wieder**
/// (12.11, 9.9): `service` nennt die naechste Grenze statt der Frist des
/// Schlafs, sonst laege der Pegel bis zur Frist ungesehen.
#[test]
fn service_names_every_boundary_while_the_program_polls() {
    let clock = RefCell::new(Fake::default());
    let mut rt = Runtime::new(
        Level::new(13 * T0 + T0 / 2),
        Shared(&clock),
        Quiet,
        Slept::default(),
        Profile::BAREMETAL,
        T0,
        Policy::Fault,
    );
    let mut next = rt.service();
    assert_eq!(next.deadline, T0, "Tick 0, dann die erste geschlafene Grenze");
    while rt.program.log.is_empty() && next.deadline < 50 * T0 {
        clock.borrow_mut().now = next.deadline;
        next = rt.service();
    }
    assert_eq!(rt.program.log, ["t=14 Pegel gain=0"]);
    assert_eq!(rt.sink.0, 13);
}

/// **Ruft der Port spaet, gilt die erste Grenze mit einer Aenderung, und ein
/// spaeterer Tune wartet auf seine** (8.4, 9.9): Je Grenze erst die
/// Tunables, dann die Wake-Quellen. Der Pegel ab 13,5 Perioden weckt an
/// Grenze 14, `GAIN = 7` an Grenze 20 sieht Tick 14 noch nicht; steht der
/// Tune an Grenze 10, weckt er dort, und das Programm wird danach nicht
/// mehr gefragt.
#[test]
fn a_late_service_takes_the_first_change_in_boundary_order() {
    let run = |changes: Vec<(u64, u8)>| {
        let clock = RefCell::new(Fake::default());
        let mut rt = Runtime::new(
            Level::new(13 * T0 + T0 / 2),
            Shared(&clock),
            Quiet,
            Slept::default(),
            Profile::BAREMETAL,
            T0,
            Policy::Fault,
        );
        let mut tunables = Changes(changes);
        rt.service_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, Some(&mut tunables));
        clock.borrow_mut().now = 49 * T0;
        rt.service_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, Some(&mut tunables));
        (rt.program.log.clone(), rt.program.polls.clone())
    };
    let (log, polls) = run(vec![(20, 7)]);
    assert_eq!(log, ["t=14 Pegel gain=0"]);
    assert_eq!(polls, (1..=14).collect::<Vec<u64>>());
    let (log, polls) = run(vec![(10, 7)]);
    assert_eq!(log, ["t=10 GAIN gain=7"]);
    assert_eq!(polls, (1..=9).collect::<Vec<u64>>());
}

/// **Ein Lauf mit Ende schlaeft hoechstens bis zu seinem letzten Tick**
/// (13.8, 9.9): Sonst laege die Frist hinter dem Ende, die Schleife haette
/// nichts mehr zu warten, und kein Weckereignis davor wirkte. Mit einem Ende
/// nach 30 Ticks weckt der Pegel ab 13,5 Perioden in Tick 14; ohne Ereignis
/// endet der Schlaf in Tick 29, einem leeren Schritt.
#[test]
fn a_run_with_an_end_sleeps_at_most_until_its_last_tick() {
    let run = |high_from: i64| {
        let clock = RefCell::new(Fake::default());
        let mut rt = Runtime::new(
            Level::new(high_from),
            Shared(&clock),
            Quiet,
            Slept::default(),
            Profile::BAREMETAL,
            T0,
            Policy::Fault,
        );
        rt.end_at(30);
        while rt.tick_number() < 30 {
            rt.step();
        }
        (rt.program.log.clone(), rt.sink.0, rt.tick_number())
    };
    assert_eq!(run(13 * T0 + T0 / 2), (vec!["t=14 Pegel gain=0".to_string()], 13, 30));
    assert_eq!(run(i64::MAX), (Vec::new(), 28, 30), "Tick 1 bis 28 geschlafen, Tick 29 rechnet");
}
