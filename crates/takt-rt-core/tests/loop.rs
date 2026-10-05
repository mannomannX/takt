//! Die Tickschleife (12.1) und ihre Zusagen.

use core::cell::RefCell;

use takt_rt_core::{Clock, Outputs, Overrun, Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};

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

/// Eine Uhr mit eigener Tickquelle (12.3): Ereignisse auf den Vielfachen von
/// T0, gewartet wird bis zum ersten an oder nach der Frist, und das naechste
/// Ereignis ist der Ursprung des Rasters.
struct Evented<'a>(&'a RefCell<Fake>);

impl Clock for Evented<'_> {
    fn now(&self) -> i64 {
        self.0.borrow().now
    }

    fn wait_until(&mut self, deadline: i64) {
        let mut f = self.0.borrow_mut();
        f.waits.push(deadline);
        f.now = f.now.max((deadline + T0 - 1) / T0 * T0);
    }

    fn origin(&self) -> i64 {
        (self.0.borrow().now / T0 + 1) * T0
    }
}

/// Ein Programm, das die Uhr um die vorgesehene Schrittdauer vorstellt.
struct Counted<'a> {
    clock: &'a RefCell<Fake>,
    ticks: Vec<(u64, i64)>,
    overruns: u32,
    sleepy: Option<i64>,
    /// Was `advance` nachgetragen bekam.
    advanced: u64,
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

    fn advance(&mut self, ticks: u64) {
        self.advanced += ticks;
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
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
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
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(3);
    let ks: Vec<u64> = rt.program.ticks.iter().map(|(k, _)| *k).collect();
    assert_eq!(ks, vec![0, 1, 2], "die Tickzahl laeuft luckenlos weiter");
}

/// 7.3: Der Fault wirkt im *naechsten* Tick. Der lange Schritt endet eine
/// halbe Periode in Tick 1, der darum noch vor seinem Raster fertig wird.
#[test]
fn an_overrun_faults_in_the_following_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![3 * T0 / 2, 0, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
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
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
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
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
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
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(5);
    assert_eq!(rt.watchdog.0, 5);
}

/// Ohne Watchdog wird nichts bestaetigt; mit ihm jeder Tick.
#[test]
fn an_absent_watchdog_is_never_kicked() {
    let mut absent: Option<Kicks> = None;
    absent.kick();
    let mut present = Some(Kicks::default());
    present.kick();
    assert!(absent.is_none() && present.is_some_and(|k| k.0 == 1));
}

// --- Schlaf (9.9) -------------------------------------------------------

/// Satz 9.9.1: Der Schlaf ist unsichtbar — die uebersprungenen Ticks
/// waeren leere Schritte gewesen, `now` rueckt exakt um sie vor.
#[test]
fn sleeping_advances_logical_time_exactly() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    // Die naechste Frist liegt bei 10 ms; der erste Tick endet bei 1 ms.
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    // Der naechste ausgefuehrte Tick ist der an der Frist.
    let second = rt.step();
    assert_eq!(second.now, 10 * T0, "der Tick an der Frist wird ausgefuehrt");
    assert_eq!(rt.sink.0[0].slept, 8, "von 1 ms bis 10 ms sind acht Ticks zu ueberspringen");
}

/// 12.3: Auch im Schlaf sieht der Watchdog jede Tickgrenze — sonst schluege
/// er in einem langen `idle` zu, obwohl die Tickquelle lebt.
#[test]
fn the_watchdog_is_kicked_at_every_slept_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    rt.step();
    assert_eq!(rt.sink.0[0].slept, 8);
    assert_eq!(rt.watchdog.0, 2 + 8, "zwei Schritte und acht geschlafene Ticks");
    let waits: Vec<i64> = (0..=9).map(|k| k * T0).collect();
    assert_eq!(clock.borrow().waits, waits, "Periode fuer Periode bis zur Frist");
}

/// Ohne erlaubten Schlaf wird nicht geschlafen — der Default eines
/// Programms, das die Bedingung nicht prueft.
#[test]
fn a_program_that_does_not_allow_sleep_never_sleeps() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    assert_eq!((rt.sink.0[0].slept, rt.tick_number()), (0, 1));
}

/// Ein Lauf, der mit einem ohne Schlaf verglichen wird (Satz 9.9.1),
/// schlaeft nicht, auch wenn das Programm duerfte.
#[test]
fn a_run_without_sleep_never_sleeps() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let profile = Profile { may_sleep: false, ..Profile::BAREMETAL };
    let mut rt = Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), profile, T0, Policy::Fault);
    rt.step();
    assert_eq!((rt.sink.0[0].slept, rt.tick_number()), (0, 1));
}

/// Eine Frist im naechsten Tick ist kein Grund zu schlafen.
#[test]
fn a_deadline_in_the_next_tick_is_no_reason_to_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(2 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    assert_eq!((rt.sink.0[0].slept, rt.tick_number()), (0, 1));
}

// --- Profile (12.8) -----------------------------------------------------

#[test]
fn every_profile_of_the_reference_has_a_name() {
    for name in ["linux_rt", "baremetal", "shared"] {
        let p = Profile::by_name(name).unwrap_or_else(|| panic!("Profil `{name}` fehlt"));
        assert_eq!(p.name(), name, "der Name geht in den Lauf-Header (11.3)");
    }
    assert!(Profile::by_name("sim").is_none(), "`sim` ist ein Build, kein Laufzeitprofil");
    assert!(Profile::by_name("boot").is_none(), "kein Profil fuer Startprogramme (12.8)");
    assert!(Profile::by_name("rtos").is_none(), "`rtos` heisst `shared` (12.8)");
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

/// **Die uebersprungenen Ticks werden nachgetragen** (9.9).
///
/// „fuer jede Maschine: time_in_state += n*T0". Ohne das bliebe der
/// Zaehler beim Einschlafen stehen, und jede `after`-Frist feuerte um die
/// geschlafenen Ticks zu spaet — Satz 9.9.1 waere verletzt.
#[test]
fn skipped_ticks_are_carried_over_to_the_program() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    assert_eq!(
        (rt.program().advanced, rt.tick_number()),
        (0, 9),
        "ohne Vorgriff: erst das Ende des Schlafs traegt nach"
    );
    rt.step();
    assert_eq!(rt.sink.0[0].slept, 8);
    assert_eq!(rt.program().advanced, 8, "genau die uebersprungenen Ticks, nicht mehr und nicht weniger");
}

/// Ohne Schlaf wird nichts nachgetragen.
#[test]
fn a_tick_without_sleep_carries_nothing_over() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.step();
    assert_eq!(rt.program().advanced, 0);
}

/// Meldet die Tickgrenzen bis `ticks` von aussen, wie ein Port unter einem
/// RTOS (12.8 `shared`, 12.11): Die Uhr steht auf der Grenze, und `service`
/// rechnet, was faellig ist. Liefert die gerechneten Ticks.
fn boundaries<P: Program, W: Watchdog>(
    rt: &mut Runtime<P, Shared<'_>, W, Log>,
    clock: &RefCell<Fake>,
    ticks: i64,
) -> Vec<Tick> {
    for b in 0..ticks {
        {
            let mut f = clock.borrow_mut();
            f.now = f.now.max(b * T0);
        }
        rt.service();
    }
    rt.sink.0.clone()
}

/// **Gemeldete Grenzen rechnen dieselben Ticks** wie die Schleife, die
/// selbst wartet (12.8): gleiche Tickzahlen, gleiche logische Zeit, auch
/// nach einem Overrun, und die Schleife wartet dabei nie.
#[test]
fn reported_boundaries_run_the_same_ticks_as_waiting() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0, 700_000, 3 * T0 / 2, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    let ticks = boundaries(&mut rt, &clock, 6);
    let want: Vec<(u64, i64)> = (0..6).map(|k| (k, (k as i64 + 1) * T0)).collect();
    assert_eq!(rt.program.ticks, want);
    assert!(ticks[2].overrun, "der lange Schritt ueberzieht");
    assert_eq!(rt.program.overruns, 1, "und faultet im naechsten Tick");
    assert!(clock.borrow().waits.is_empty(), "keine Grenze wurde abgewartet");
}

/// Nach virtuellen Ticks (9.9) bestaetigt jede gemeldete Grenze bis zur
/// Frist nur den Watchdog; der Tick an der Frist laeuft.
#[test]
fn a_boundary_before_the_deadline_only_kicks_the_watchdog() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    let ticks = boundaries(&mut rt, &clock, 10);
    assert_eq!(ticks.len(), 2, "Tick 0 und der an der Frist: {ticks:?}");
    assert_eq!((ticks[0].slept, ticks[1].k), (8, 9));
    assert_eq!(rt.watchdog.0, 2 + 8, "je Tick einmal, dazu jede geschlafene Grenze");
}

// --- Kern ohne Warten (12.11) -------------------------------------------

/// **`service` wartet nie und nennt die naechste Frist.** Ein Port ruft zur
/// Frist; was dazwischen geschieht, ist seine Sache.
#[test]
fn service_never_waits_and_names_the_next_deadline() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    let next = rt.service();
    assert_eq!((rt.program.ticks.len(), next.deadline, next.ended), (1, T0, None));
    assert_eq!(rt.service(), next, "vor der Frist ist nichts faellig");
    assert!(clock.borrow().waits.is_empty());
}

/// **Ein spaeter Aufruf rechnet jede Grenze bis jetzt**, in Reihenfolge und
/// mit ihrer logischen Zeit (7.3: kein Tick wird uebersprungen).
#[test]
fn a_late_call_catches_up_every_boundary() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    clock.borrow_mut().now = 3 * T0 + T0 / 2;
    let next = rt.service();
    let want: Vec<(u64, i64)> = (0..4).map(|k| (k, (k as i64 + 1) * T0)).collect();
    assert_eq!(rt.program.ticks, want);
    assert_eq!(next.deadline, 4 * T0);
    assert_eq!(rt.sink.0[3].drift, T0 / 2, "der Rueckstand steht in `drift`");
}

/// **„Jetzt“ ist die Zeit beim Eintritt.** Dauert jeder Tick laenger als die
/// Periode, rechnet ein Aufruf nur die Grenzen, die bei seinem Eintritt
/// faellig waren — sonst kaeme `service` nie zurueck.
#[test]
fn service_computes_only_what_was_due_when_it_was_called() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    rt.service();
    assert_eq!(rt.program.ticks.len(), 1);
    rt.service();
    assert_eq!(rt.program.ticks.len(), 1 + 5, "beim zweiten Eintritt waren fuenf Grenzen faellig");
}

/// **Mit erlaubtem Schlaf ist die Frist die des Schlafs** (9.9, 12.11).
#[test]
fn service_names_the_deadline_of_the_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    assert_eq!(rt.service().deadline, 9 * T0, "Tick 0, dann acht geschlafene; Tick 9 endet an der Frist");
    assert_eq!(rt.program.advanced, 0, "ohne Vorgriff (FB-388)");
    clock.borrow_mut().now = 9 * T0;
    rt.service();
    assert_eq!(rt.program.advanced, 8);
}

/// Ein Programm, das mitschreibt, in welcher Reihenfolge der Kern es ruft.
#[derive(Default)]
struct Ordered {
    log: Vec<&'static str>,
    ends_after: Option<u64>,
    ticks: u64,
}

impl Program for Ordered {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.ticks += 1;
        self.log.push("tick");
    }

    fn sleep_allowed(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<i64> {
        Some(10 * T0)
    }

    fn advance(&mut self, _ticks: u64) {
        self.log.push("advance");
    }

    fn next_run(&self) -> Option<takt_rt_core::NextRun> {
        self.ends_after.filter(|n| self.ticks >= *n).map(|_| takt_rt_core::NextRun::OnStart)
    }

    fn commit(&mut self) {
        self.log.push("commit");
    }

    fn trace(&mut self, outputs: Outputs) {
        if outputs != Outputs::None {
            self.log.push("trace");
        }
    }

    fn end(&mut self) {
        self.log.push("end");
    }
}

/// Zeigt jeden Tick seine Aenderungen, wie ein Konformitaetslauf.
struct EveryTick;

impl Sink for EveryTick {
    fn outputs(&mut self, tick: Option<&Tick>) -> Outputs {
        if tick.is_some() { Outputs::Changed } else { Outputs::All }
    }

    fn record(&mut self, _tick: &Tick) {}
}

/// **Der Latch geht vor dem Schlaf an die Treiber** (12.1 Schritt 10, 9.9;
/// FB-371): Schritt, Commit, Schlaf, Trace — der Commit gehoert zum Tick,
/// den er stellt.
#[test]
fn the_latch_is_committed_before_the_sleep() {
    let mut rt = Runtime::new(
        Ordered::default(),
        Logical(0),
        Kicks::default(),
        EveryTick,
        Profile::BAREMETAL,
        T0,
        Policy::Fault,
    );
    rt.service();
    assert_eq!(
        rt.program.log,
        ["commit", "trace", "tick", "commit"],
        "Tick 0 committet; der Trace des Ticks wartet auf das Ende des Schlafs"
    );
    rt.clock.0 = 9 * T0;
    rt.service();
    assert_eq!(rt.program.log[4..], ["advance", "trace", "tick", "commit", "trace"]);
}

/// **Das geordnete Ende gehoert zum Kern** (12.7): Nach dem Tick, der
/// `next_run` setzt, gehen die `safe`-Werte an die Treiber und in den Trace,
/// `Next` nennt den Befehl, und kein weiterer Tick laeuft.
#[test]
fn the_end_of_a_run_is_part_of_the_service() {
    let program = Ordered { ends_after: Some(1), ..Ordered::default() };
    let mut rt = Runtime::new(program, Logical(0), Kicks::default(), EveryTick, Profile::BAREMETAL, T0, Policy::Fault);
    let next = rt.service();
    assert_eq!(next.ended, Some(takt_rt_core::NextRun::OnStart));
    assert_eq!(rt.program.log, ["commit", "trace", "tick", "commit", "trace", "end", "commit", "trace"]);
    rt.clock.0 = 100 * T0;
    rt.service();
    assert_eq!(rt.program.ticks, 1, "nach dem Ende rechnet der Kern nichts mehr");
}

/// Eine Uhr, die der Test stellt und die nie wartet.
struct Logical(i64);

impl Clock for Logical {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

/// **Die Nummern von `P_next_run` laufen ab 1 ohne Luecke und kommen
/// zurueck** (12.7, GEN-029): Der Rahmen schreibt `NextRun::code`, jede
/// Huelle liest mit `NextRun::from_code`; die Zeile `end` im Rahmen
/// waehlt ihr Wort ueber `code - 1`.
#[test]
fn every_end_of_a_run_comes_back_from_its_code() {
    use takt_rt_core::NextRun;
    let ends = [NextRun::Now, NextRun::After(5), NextRun::OnWake, NextRun::OnStart];
    for (i, end) in ends.into_iter().enumerate() {
        assert_eq!(end.code(), i as i32 + 1, "{end:?}");
        assert_eq!(NextRun::from_code(end.code(), 5), Some(end));
    }
    assert_eq!(NextRun::from_code(0, 5), None, "0 heisst weiter");
    assert_eq!(NextRun::from_code(5, 5), None);
}

// --- Ueberlaeufe in Folge und an Grenzen (7.3) --------------------------

/// **Jeder ueberzogene Tick faultet genau einmal, im Tick danach** (7.3):
/// Vier Ticks zu je fuenf Perioden ergeben nach vier Schritten drei Faults —
/// der vierte steht noch aus.
#[test]
fn every_overrun_faults_once_in_the_following_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0; 4], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    let mut seen = Vec::new();
    for _ in 0..4 {
        rt.step();
        seen.push(rt.program.overruns);
    }
    assert_eq!(seen, [0, 1, 2, 3], "je Folgetick genau einer");
    assert!(rt.sink.0.iter().all(|t| t.overrun));
    assert_eq!(rt.overrun().count, 4);
}

/// **Eine Nanosekunde ueber der Periode ist ein Ueberlauf**, genau die
/// Periode nicht (7.3).
#[test]
fn one_nanosecond_over_the_period_is_an_overrun() {
    for (cost, over) in [(T0, false), (T0 + 1, true)] {
        let clock = RefCell::new(Fake { now: 0, costs: vec![cost, 0], waits: Vec::new() });
        let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
        let mut rt = Runtime::new(
            program,
            Shared(&clock),
            Kicks::default(),
            Log::default(),
            Profile::LINUX_RT,
            T0,
            Policy::Fault,
        );
        assert_eq!(rt.step().overrun, over, "Schritt von {cost} ns");
        rt.step();
        assert_eq!(rt.program.overruns, u32::from(over), "Schritt von {cost} ns");
    }
}

/// Ein Programm mit Schrittkosten, das den Lauf nach dem ersten Tick beendet.
struct Ending<'a> {
    clock: &'a RefCell<Fake>,
    ticks: u64,
    overruns: u32,
}

impl Program for Ending<'_> {
    fn tick(&mut self, k: u64, _now: i64) {
        self.ticks += 1;
        let mut f = self.clock.borrow_mut();
        let cost = *f.costs.get(k as usize).or(f.costs.last()).unwrap_or(&0);
        f.now += cost;
    }

    fn raise_overrun(&mut self) {
        self.overruns += 1;
    }

    fn next_run(&self) -> Option<takt_rt_core::NextRun> {
        (self.ticks >= 1).then_some(takt_rt_core::NextRun::OnStart)
    }
}

/// **Ein Ueberlauf im Tick, der den Lauf beendet, steht im Protokoll und
/// faultet niemanden** (7.3, 12.7): Es gibt keinen naechsten Tick, in dem
/// der Fault wirken koennte; die physische Verzoegerung bleibt sichtbar.
#[test]
fn an_overrun_in_the_last_tick_is_recorded_and_raises_nothing() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0], waits: Vec::new() });
    let program = Ending { clock: &clock, ticks: 0, overruns: 0 };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    assert_eq!(rt.service().ended, Some(takt_rt_core::NextRun::OnStart));
    clock.borrow_mut().now = 100 * T0;
    rt.service();
    assert_eq!((rt.program.ticks, rt.program.overruns), (1, 0), "kein Tick mehr, der den Fault saehe");
    assert!(rt.sink.0[0].overrun, "die Ueberschreitung steht in der Zeitzeile");
    assert_eq!(rt.overrun().count, 1);
}

/// **Ein Ueberlauf vor dem Schlaf haelt die Schleife wach** (7.3, 9.9):
/// Der Fault wirkt im naechsten Tick, und ein vorgemerkter Fault ist ein
/// `raised`, das den Schlaf verbietet — sonst wirkte er erst an der Frist.
#[test]
fn a_pending_overrun_forbids_the_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    let first = rt.step();
    assert_eq!((first.overrun, first.slept), (true, 0), "kein Schlaf mit vorgemerktem Fault");
    let second = rt.step();
    assert_eq!((second.k, rt.program.overruns), (1, 1), "der Fault wirkt im Tick 1");
    assert_eq!(rt.program.advanced, second.slept, "danach darf geschlafen werden");
}

/// Unter `alert` wird nichts vorgemerkt, und der Schlaf bleibt erlaubt.
#[test]
fn an_overrun_under_alert_does_not_forbid_the_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Alert);
    assert!(rt.step().overrun);
    rt.step();
    assert_eq!(rt.sink.0[0].slept, 8);
}

// --- Watchdog: nach dem Schritt, nie nach dem Ende (12.3, 12.4) ---------

/// Ein Programm und ein Watchdog, die in dasselbe Log schreiben.
struct Logged<'a> {
    log: &'a RefCell<Vec<&'static str>>,
    ends_after: Option<u64>,
    ticks: u64,
}

impl Program for Logged<'_> {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.ticks += 1;
        self.log.borrow_mut().push("tick");
    }

    fn next_run(&self) -> Option<takt_rt_core::NextRun> {
        self.ends_after.filter(|n| self.ticks >= *n).map(|_| takt_rt_core::NextRun::Now)
    }

    fn commit(&mut self) {
        self.log.borrow_mut().push("commit");
    }

    fn end(&mut self) {
        self.log.borrow_mut().push("end");
    }
}

struct LoggedKicks<'a>(&'a RefCell<Vec<&'static str>>);

impl Watchdog for LoggedKicks<'_> {
    fn kick(&mut self) {
        self.0.borrow_mut().push("kick");
    }
}

/// **Der Watchdog wird nach dem Schritt und dem Commit bestaetigt** (12.4),
/// je Tick einmal, auch wenn ein spaeter Aufruf drei Grenzen aufholt.
#[test]
fn the_watchdog_is_kicked_after_the_step_of_every_tick() {
    let log = RefCell::new(Vec::new());
    let program = Logged { log: &log, ends_after: None, ticks: 0 };
    let mut rt = Runtime::new(program, Logical(0), LoggedKicks(&log), (), Profile::BAREMETAL, T0, Policy::Fault);
    rt.step();
    assert_eq!(*log.borrow(), ["commit", "tick", "commit", "kick"], "Tick 0 committet ohne Bestaetigung");
    log.borrow_mut().clear();
    rt.clock.0 = 3 * T0 + T0 / 2;
    rt.service();
    assert_eq!(*log.borrow(), ["tick", "commit", "kick"].repeat(3), "drei Grenzen, drei Ticks");
}

/// **Nach dem Ende des Laufs bestaetigt der Kern nichts mehr** (12.7, 12.3):
/// Der letzte Tick kickt nach seinem Schritt, dann folgen Ende und `safe`;
/// spaetere Aufrufe rechnen und bestaetigen nichts.
#[test]
fn no_kick_follows_the_end_of_a_run() {
    let log = RefCell::new(Vec::new());
    let program = Logged { log: &log, ends_after: Some(1), ticks: 0 };
    let mut rt = Runtime::new(program, Logical(0), LoggedKicks(&log), (), Profile::BAREMETAL, T0, Policy::Fault);
    assert!(rt.service().ended.is_some());
    assert_eq!(*log.borrow(), ["commit", "tick", "commit", "kick", "end", "commit"], "Tick 0 committet zuerst");
    rt.clock.0 = 50 * T0;
    rt.service();
    rt.step();
    assert_eq!(log.borrow().len(), 6, "nach dem Ende: {:?}", log.borrow());
}

// --- Extremwerte (7.1, 7.3, 9.9) ----------------------------------------

/// **Die logische Zeit beginnt bei null, wann immer die Uhr startet** (12.1):
/// Eine Uhr, die bei 5 ms steht, verschiebt die Fristen, nicht `now`.
#[test]
fn a_clock_that_starts_late_shifts_the_deadlines_only() {
    let clock = RefCell::new(Fake { now: 5 * T0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(3);
    assert_eq!(rt.program.ticks, [(0, T0), (1, 2 * T0), (2, 3 * T0)]);
    assert_eq!(clock.borrow().waits, [5 * T0, 6 * T0, 7 * T0]);
    assert!(rt.sink.0.iter().all(|t| t.drift == 0));
}

/// **Eine Frist, die kein Vielfaches von T0 ist, wird nie uebersprungen**
/// (9.9): Der erste ausgefuehrte Tick nach dem Schlaf endet spaetestens an
/// der ersten Tickgrenze, die die Frist erreicht.
#[test]
fn a_deadline_between_two_boundaries_is_never_slept_past() {
    for deadline in [10 * T0 - 1, 10 * T0, 10 * T0 + 1, 10 * T0 + T0 / 2, 11 * T0 - 1] {
        let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
        let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(deadline) };
        let mut rt = Runtime::new(
            program,
            Shared(&clock),
            Kicks::default(),
            Log::default(),
            Profile::BAREMETAL,
            T0,
            Policy::Fault,
        );
        rt.step();
        let next = rt.step();
        let first_reaching = (deadline + T0 - 1) / T0 * T0;
        assert!(next.now <= first_reaching, "Frist {deadline}: naechster Tick endet bei {}", next.now);
        assert_eq!(rt.program.advanced as i64, next.now / T0 - 2, "Frist {deadline}: nachgetragen");
    }
}

/// **Ein langer Schlaf rueckt genau um seine Ticks vor** (9.9), auch ueber
/// eine Million Perioden.
#[test]
fn a_long_sleep_advances_exactly() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(1_000_000 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    assert_eq!(rt.service().deadline, (1_000_000 - 1) * T0);
    assert_eq!(rt.tick_number(), 999_999);
    clock.borrow_mut().now = (1_000_000 - 1) * T0;
    rt.service();
    assert_eq!((rt.program.advanced, rt.sink.0[0].slept), (999_998, 999_998));
}

/// **Ohne Frist schlaeft der Kern nicht** (9.9): Einen Schlaf, den nur ein
/// Weckereignis beendet, sieht er nicht vor.
#[test]
fn without_a_deadline_the_core_does_not_sleep() {
    struct Deadlineless;
    impl Program for Deadlineless {
        fn tick(&mut self, _k: u64, _now: i64) {}
        fn sleep_allowed(&self) -> bool {
            true
        }
    }
    let mut rt =
        Runtime::new(Deadlineless, Logical(0), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    rt.step();
    assert_eq!((rt.sink.0[0].slept, rt.tick_number()), (0, 1));
}

/// Ein Programm, das nur seine Ticks zaehlt und nie schlaeft.
#[derive(Default)]
struct Plain(u64);

impl Program for Plain {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.0 += 1;
    }
}

/// **Eine Periode von einer Nanosekunde** ist die kleinste, die es gibt;
/// die Fristen wachsen um sie.
#[test]
fn a_period_of_one_nanosecond_still_counts_ticks() {
    let mut rt = Runtime::new(Plain::default(), Logical(0), Kicks::default(), (), Profile::BAREMETAL, 1, Policy::Fault);
    rt.clock.0 = 9;
    assert_eq!(rt.service().deadline, 10);
    assert_eq!(rt.program.0, 10);
}

/// **Eine Uhr am Ende ihres Bereichs haengt den Kern nicht auf** (12.11:
/// kein Einstieg blockiert): Steht sie auf `i64::MAX`, waechst die Frist
/// nicht mehr; `i64::MAX` ist das Ende der Zeitachse und keine Grenze, an
/// der `service` noch rechnet.
#[test]
fn a_clock_at_the_end_of_its_range_does_not_hang_the_service() {
    let start = i64::MAX - T0;
    let mut rt =
        Runtime::new(Plain::default(), Logical(start), Kicks::default(), (), Profile::BAREMETAL, T0, Policy::Fault);
    rt.clock.0 = i64::MAX;
    let next = rt.service();
    assert_eq!((next.deadline, rt.program.0), (i64::MAX, 1), "der Tick an der letzten Grenze davor");
    rt.service();
    assert_eq!(rt.program.0, 1, "an der gesaettigten Frist rechnet der Kern nichts mehr");
}

/// **Die logische Zeit saettigt, statt umzuschlagen** (12.1): Ab 2^63 Ticks
/// ergaebe `k as i64` eine negative Zeit.
#[test]
fn the_logical_time_saturates() {
    assert_eq!(takt_rt_core::tick_end(u64::MAX, 1), i64::MAX);
    assert_eq!(takt_rt_core::tick_end(1 << 63, T0), i64::MAX);
    assert_eq!(takt_rt_core::tick_end(41, T0), 42 * T0);
}

// --- Jobs (4.5) ---------------------------------------------------------

/// Slots mit vorgegebenem Zustand und Ergebnis.
struct Scripted {
    states: Vec<takt_rt_core::JobState>,
    results: Vec<Vec<u8>>,
    taken: Vec<u32>,
}

impl takt_rt_core::Jobs for Scripted {
    fn slots(&self) -> u32 {
        self.states.len() as u32
    }

    fn begin(&mut self, _slot: u32, _native: &str, _args: &[u8]) -> bool {
        false
    }

    fn poll(&mut self, slot: u32) -> takt_rt_core::JobState {
        self.states[slot as usize]
    }

    fn take(&mut self, slot: u32, into: &mut [u8]) -> usize {
        self.taken.push(slot);
        self.states[slot as usize] = takt_rt_core::JobState::Idle;
        let result = &self.results[slot as usize];
        if let Some(to) = into.get_mut(..result.len()) {
            to.copy_from_slice(result);
        }
        result.len()
    }

    fn cancel(&mut self, _slot: u32) {}
}

/// Schreibt mit, was der Kern als Fertigstellung zustellt.
#[derive(Default)]
struct Waiting {
    done: Vec<(u32, Option<Vec<u8>>)>,
    ticks: u64,
    ends_after: Option<u64>,
}

impl Program for Waiting {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.ticks += 1;
    }

    fn job_done(&mut self, slot: u32, result: Option<&[u8]>) {
        self.done.push((slot, result.map(<[u8]>::to_vec)));
    }

    fn next_run(&self) -> Option<takt_rt_core::NextRun> {
        self.ends_after.filter(|n| self.ticks >= *n).map(|_| takt_rt_core::NextRun::Now)
    }
}

/// **Fertige Jobs gehen vor dem Schritt ins Programm** (4.5): `Done` mit
/// Ergebnis, `Failed` als `Err(FAILED)`; ein laufender bleibt, wo er ist.
/// Ein Ergebnis, das den Puffer uebersteigt, wird verweigert statt gekuerzt —
/// gekuerzt waere es ein gueltiges, falsches Ergebnis.
#[test]
fn finished_jobs_reach_the_program_before_the_step() {
    use takt_rt_core::JobState::{Done, Failed, Idle, Running};
    let mut jobs = Scripted {
        states: vec![Done, Failed, Running, Done, Idle],
        results: vec![vec![1, 2, 3], Vec::new(), Vec::new(), vec![9; 10], Vec::new()],
        taken: Vec::new(),
    };
    let mut rt =
        Runtime::new(Waiting::default(), Logical(0), Kicks::default(), (), Profile::BAREMETAL, T0, Policy::Fault);
    let mut buf = [0u8; 4];
    rt.step_with_jobs(&mut jobs, &mut buf);
    assert_eq!(rt.program.done, [(0, Some(vec![1, 2, 3])), (1, None), (3, None)]);
    assert_eq!(jobs.taken, [0, 1, 3], "der laufende und der freie Slot bleiben");
    assert_eq!(rt.program.ticks, 1);
}

/// **Nach dem Ende des Laufs erreicht kein Ergebnis das Programm mehr**
/// (12.7): Es gibt keinen Schritt, der es saehe.
#[test]
fn no_job_result_arrives_after_the_end_of_a_run() {
    use takt_rt_core::JobState::{Done, Running};
    let mut jobs = Scripted { states: vec![Running], results: vec![vec![7]], taken: Vec::new() };
    let program = Waiting { ends_after: Some(1), ..Waiting::default() };
    let mut rt = Runtime::new(program, Logical(0), Kicks::default(), (), Profile::BAREMETAL, T0, Policy::Fault);
    let mut buf = [0u8; 4];
    rt.step_with_jobs(&mut jobs, &mut buf);
    assert!(rt.ended().is_some());
    jobs.states[0] = Done;
    rt.step_with_jobs(&mut jobs, &mut buf);
    assert!(rt.program.done.is_empty(), "{:?}", rt.program.done);
    assert_eq!(rt.program.ticks, 1);
}

/// **Uebergelaufen ist, wer nach dem nominalen Beginn des naechsten Ticks
/// endet** (7.3, RT-003): gemessen am Raster `t0 + (k+1)·T0`. Ein Beginn eine
/// halbe Periode zu spaet laesst dem Schritt eine halbe Periode; endet er
/// genau am Raster, ist es keiner, eine Nanosekunde danach einer.
#[test]
fn the_overrun_is_measured_against_the_grid() {
    for (cost, over) in [(T0 / 2, false), (T0 / 2 + 1, true)] {
        let clock = RefCell::new(Fake { now: 0, costs: vec![cost, 0], waits: Vec::new() });
        let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
        let mut rt = Runtime::new(
            program,
            Shared(&clock),
            Kicks::default(),
            Log::default(),
            Profile::LINUX_RT,
            T0,
            Policy::Fault,
        );
        clock.borrow_mut().now = T0 / 2;
        let first = rt.service();
        assert_eq!(rt.sink.0[0].drift, T0 / 2);
        assert_eq!(rt.sink.0[0].overrun, over, "Schritt von {cost} ns nach T0/2 Rueckstand");
        assert_eq!(rt.overrun().count, u64::from(over));
        clock.borrow_mut().now = first.deadline;
        rt.service();
        assert_eq!(rt.program.overruns, u32::from(over), "der Fault wirkt im naechsten Tick");
    }
}

/// **Das Raster beginnt am Ursprung der Uhr** (7.3, 12.3, FB-436): Gibt
/// die Tickquelle die Grenzen vor, liegt `t0` auf ihrem naechsten Ereignis.
/// Begaenne es beim Start, 0,47 Perioden nach einem Ereignis, truege jeder
/// Tick 0,53 Perioden `drift`, und ein Schritt von 0,6 Perioden liefe am
/// Raster ueber, obwohl er vor dem naechsten Ereignis endet.
#[test]
fn the_grid_begins_at_the_origin_of_the_clock() {
    let clock = RefCell::new(Fake { now: T0 * 47 / 100, costs: vec![T0 * 6 / 10], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Evented(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    rt.run(3);
    assert_eq!(clock.borrow().waits.first(), Some(&T0), "Tick 0 wartet auf das erste Ereignis");
    let drifts: Vec<i64> = rt.sink.0.iter().map(|t| t.drift).collect();
    assert_eq!(drifts, vec![0, 0, 0]);
    assert_eq!(rt.overrun().count, 0);
}

/// **Ein spaeter Beginn mit kurzem Schritt ist ein Ueberlauf** (7.3,
/// RT-003): 0,9 Perioden Rueckstand und 0,2 Perioden Schritt enden 0,1
/// Perioden nach dem Raster, obwohl der Schritt selbst kurz war.
#[test]
fn a_late_start_with_a_short_step_overruns() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![T0 / 5, 0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    clock.borrow_mut().now = 9 * T0 / 10;
    rt.step();
    assert!(rt.sink.0[0].overrun, "Ende bei 1,1 T0, das Raster bei 1 T0");
    assert_eq!(rt.overrun().worst, T0 / 10, "um eine Zehntelperiode ueber dem Raster");
}

/// **Ein `service` 3,5 Perioden zu spaet faultet** (7.3, 12.11, RT-003):
/// Die Ticks 0 bis 2 enden alle nach dem Beginn ihres Nachfolgers, Tick 3
/// eine halbe Periode davor. Jeder Ueberlauf wirkt einmal, im Tick danach.
#[test]
fn a_service_call_three_and_a_half_periods_late_faults() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: None };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::BAREMETAL, T0, Policy::Fault);
    clock.borrow_mut().now = 3 * T0 + T0 / 2;
    let next = rt.service();
    let over: Vec<bool> = rt.sink.0.iter().map(|t| t.overrun).collect();
    assert_eq!(over, [true, true, true, false]);
    assert_eq!(rt.overrun().count, 3);
    assert_eq!(rt.program.overruns, 3, "jeder wirkt im Tick danach, alle noch in diesem Aufruf");
    clock.borrow_mut().now = next.deadline;
    rt.service();
    assert_eq!(rt.program.overruns, 3, "Tick 3 endete vor dem Raster: kein weiterer");
}

/// Ein Programm, dessen Latch nach dem Start `0` haelt und das jeder Tick
/// neu setzt; die Treiber sehen, was `commit` ausschreibt.
struct Latched {
    latch: u64,
    driven: Vec<u64>,
}

impl Program for Latched {
    fn tick(&mut self, k: u64, _now: i64) {
        self.latch = k + 1;
    }

    fn commit(&mut self) {
        self.driven.push(self.latch);
    }
}

/// **Tick 0 committet wie jeder andere Tick** (1.5, 9.4): Was der
/// Anfangszustand in den Latch schreibt (`enter:` des Initialzustands),
/// erreicht die Treiber vor Tick 1 — sonst kaeme ein Wert, den Tick 1
/// schon aendert, nie an, waehrend der Trace ihn zeigt.
#[test]
fn tick_zero_commits_before_tick_one() {
    let mut rt = Runtime::new(
        Latched { latch: 0, driven: Vec::new() },
        Logical(0),
        Kicks::default(),
        Log::default(),
        Profile::BAREMETAL,
        T0,
        Policy::Fault,
    );
    rt.service();
    assert_eq!(rt.program.driven, [0, 1], "erst der Latch von Tick 0, dann der von Tick 1");
    rt.clock.0 = T0;
    rt.service();
    assert_eq!(rt.program.driven, [0, 1, 2], "Tick 0 committet einmal");
}
