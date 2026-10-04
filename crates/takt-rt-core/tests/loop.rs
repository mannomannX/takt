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

/// 7.3: Der Fault wirkt im *naechsten* Tick.
#[test]
fn an_overrun_faults_in_the_following_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![5 * T0, 0, 0], waits: Vec::new() });
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
    let first = rt.step();
    assert_eq!(first.slept, 8, "von 1 ms bis 10 ms sind acht Ticks zu ueberspringen");
    // Der naechste ausgefuehrte Tick ist der an der Frist.
    let second = rt.step();
    assert_eq!(second.now, 10 * T0, "der Tick an der Frist wird ausgefuehrt");
}

/// 12.3: Auch im Schlaf sieht der Watchdog jede Tickgrenze — sonst schluege
/// er in einem langen `idle` zu, obwohl die Tickquelle lebt.
#[test]
fn the_watchdog_is_kicked_at_every_slept_tick() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 8);
    rt.step();
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
    assert_eq!(rt.step().slept, 0);
}

/// Ein Lauf, der mit einem ohne Schlaf verglichen wird (Satz 9.9.1),
/// schlaeft nicht, auch wenn das Programm duerfte.
#[test]
fn a_run_without_sleep_never_sleeps() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(10 * T0) };
    let profile = Profile { may_sleep: false, ..Profile::BAREMETAL };
    let mut rt = Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), profile, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 0);
}

/// Eine Frist im naechsten Tick ist kein Grund zu schlafen.
#[test]
fn a_deadline_in_the_next_tick_is_no_reason_to_sleep() {
    let clock = RefCell::new(Fake { now: 0, costs: vec![0], waits: Vec::new() });
    let program = Counted { clock: &clock, ticks: Vec::new(), overruns: 0, advanced: 0, sleepy: Some(2 * T0) };
    let mut rt =
        Runtime::new(program, Shared(&clock), Kicks::default(), Log::default(), Profile::LINUX_RT, T0, Policy::Fault);
    assert_eq!(rt.step().slept, 0);
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
    let first = rt.step();
    assert_eq!(first.slept, 8);
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
    assert_eq!(rt.program.log, ["trace", "tick", "commit", "advance", "trace"]);
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
    assert_eq!(rt.program.log, ["trace", "tick", "commit", "trace", "end", "commit", "trace"]);
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
