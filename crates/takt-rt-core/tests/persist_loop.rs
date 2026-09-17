//! Das Journal in der Tickschleife (5.9, 12.1).
//!
//! Ein Programm, das eine Zahl persistiert; die Schleife schreibt sie ins
//! Journal, ein Neustart liest sie zurueck. Dazu die Zusage, die FB-149
//! gebraucht haette: Ein langsames Geraet haelt den Tick nicht auf.

use takt_rt_core::journal::{FakeNvm, Journal, Loaded, Persist};
use takt_rt_core::{Clock, Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};

const SLOT: usize = 256;
const T0: i64 = 1_000_000;
const HASH: u64 = 0x1234;

/// Eine Uhr, die dem Tick ohne Wartezeit folgt.
#[derive(Default)]
struct Instant(i64);

impl Clock for Instant {
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

impl Sink for Quiet {
    fn record(&mut self, _: &Tick) {}
}

/// Zaehlt Ticks und persistiert den Zaehler als vier Bytes.
struct Counter {
    value: u32,
    restored: Option<u32>,
}

impl Program for Counter {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.value += 1;
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        4
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        if bytes.len() != 4 {
            return 0;
        }
        self.value = u32::from_le_bytes(bytes.try_into().expect("vier Byte"));
        self.restored = Some(self.value);
        1
    }
}

fn runtime(program: Counter) -> Runtime<Counter, Instant, Quiet, Quiet> {
    Runtime::new(program, Instant::default(), Quiet, Quiet, Profile::LINUX_RT, T0, Policy::Fault)
}

#[test]
fn the_loop_writes_the_journal_and_a_restart_reads_it_back() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    assert_eq!(persist.load(&mut rt.program).0, Loaded::Empty);
    rt.run_persisting(50, &mut persist);
    assert!(persist.journal().writes() > 0, "die Schleife hat nie geschrieben");
    assert!(persist.flush(&mut rt.program), "flush scheiterte");
    let value = rt.program.value;

    // Neustart mit demselben Geraet.
    let nvm = persist.into_journal().into_inner();
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(nvm, HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    let (found, applied) = persist.load(&mut rt.program);
    assert!(matches!(found, Loaded::Found { .. }), "nichts gefunden");
    assert_eq!(applied, 1);
    assert_eq!(rt.program.restored, Some(value), "der Zaehler kam nicht zurueck");
}

#[test]
fn min_interval_holds_in_the_loop() {
    // 5.9: hoechstens alle min_interval — bei 10 s und 1-ms-Tick sind das
    // hoechstens ein Schreibvorgang je 10 000 Ticks.
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let ten_s = 10_000 * T0;
    let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, ten_s), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    persist.load(&mut rt.program);
    rt.run_persisting(25_000, &mut persist);
    let writes = persist.journal().writes();
    assert!(writes <= 3, "{writes} Schreibvorgaenge in 25 s bei 10 s Abstand");
    assert!(writes >= 2, "{writes} Schreibvorgaenge: das Journal schreibt nicht mehr");
}

#[test]
fn a_slow_device_costs_the_tick_nothing() {
    // Ein Geraet, dessen Vorgaenge 200 Ticks dauern: Die Schleife laeuft
    // in logischer Zeit weiter, kein Tick wartet auf das Journal.
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let slow = FakeNvm::<SLOT>::new().with_latency(200);
    let mut persist = Persist::new(Journal::new(slow, HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    persist.load(&mut rt.program);
    for _ in 0..1000 {
        let tick = rt.step_persisting(&mut persist);
        assert!(!tick.overrun, "Tick {} lief ueber", tick.k);
    }
    assert!(persist.journal().writes() >= 1);
}

/// Zaehlt Ticks, persistiert den Zaehler und schlaeft nach jedem Schritt
/// `window` Ticks (9.9), wenn `window > 0`.
struct Sleeper {
    value: u32,
    now: i64,
    window: i64,
}

impl Program for Sleeper {
    fn tick(&mut self, _k: u64, now: i64) {
        self.value += 1;
        self.now = now;
    }

    fn sleep_allowed(&self) -> bool {
        self.window > 0
    }

    fn next_deadline(&self) -> Option<i64> {
        Some(self.now + self.window * T0)
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        4
    }
}

fn sleeping_runtime(window: i64, policy: Policy) -> Runtime<Sleeper, Instant, Quiet, Quiet> {
    let program = Sleeper { value: 0, now: 0, window };
    Runtime::new(program, Instant::default(), Quiet, Quiet, Profile::BAREMETAL, T0, policy)
}

/// Ein Geraet, das den Kern fuenf Ticks lang anhaelt (12.3).
fn blocking() -> FakeNvm<SLOT> {
    FakeNvm::<SLOT>::new().with_blocking_ns(5 * T0)
}

/// **Ein blockierendes Geraet schreibt nur im Schlaffenster** (12.3, 9.9):
/// Mit zehn Ticks Schlaf je Schritt deckt das Fenster den Vorgang, mit
/// zwei nicht.
#[test]
fn a_blocking_device_writes_only_in_a_sleep_window() {
    for (window, expect_writes) in [(10, true), (2, false)] {
        let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
        let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
        let mut rt = sleeping_runtime(window, Policy::Fault);
        persist.load(&mut rt.program);
        rt.run_persisting(100, &mut persist);
        assert_eq!(persist.journal().writes() > 0, expect_writes, "Fenster von {window} Ticks");
    }
}

/// Unter `overrun = alert` schreibt es sofort — das Programm traegt die
/// Ueberlaeufe (7.3).
#[test]
fn under_alert_a_blocking_device_writes_at_once() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
    let mut rt = sleeping_runtime(0, Policy::Alert);
    persist.load(&mut rt.program);
    rt.run_persisting(20, &mut persist);
    assert!(persist.journal().writes() > 0);
}

/// Ohne Fenster und unter `fault` schreibt nur der Flush (5.9: vor
/// `reboot`, `boot_jump`, Deep Sleep).
#[test]
fn without_a_window_only_the_flush_writes() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
    let mut rt = sleeping_runtime(0, Policy::Fault);
    persist.load(&mut rt.program);
    rt.run_persisting(100, &mut persist);
    assert_eq!(persist.journal().writes(), 0);
    assert!(persist.flush(&mut rt.program));
    assert_eq!(persist.journal().writes(), 1);
}
