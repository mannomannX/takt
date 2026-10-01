//! Die Tick-Periode (12.6 Zeile 7, 7.1): Haelt die Tickquelle ihre Periode
//! fuer `tick_tolerance` nicht, ist es `Runtime(Hardware)` — erst nach der
//! konfigurierten Zahl aufeinanderfolgender Verletzungen.

use takt_rt_core::{Clock, Policy, Profile, Program, Runtime, Tolerance, Watchdog};

const MS: i64 = 1_000_000;

/// Eine Tickquelle, die je Tick die naechste Periode aus ihrer Liste meldet.
struct Source {
    now: i64,
    periods: Vec<i64>,
    k: usize,
}

impl Clock for Source {
    fn now(&self) -> i64 {
        self.now
    }

    fn wait_until(&mut self, deadline: i64) {
        self.now = self.now.max(deadline);
        self.k += 1;
    }

    fn tick_period(&self) -> Option<i64> {
        self.periods.get(self.k.saturating_sub(1)).copied()
    }
}

/// Ein Watchdog, der nichts tut.
struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Ein Programm mit `2 pct for 3 ticks`, das die erhobenen Faults zaehlt.
#[derive(Default)]
struct Watched {
    hardware: Vec<u64>,
    k: u64,
}

impl Program for Watched {
    fn tick(&mut self, k: u64, _now: i64) {
        self.k = k;
    }

    fn raise_hardware(&mut self) {
        self.hardware.push(self.k + 1);
    }

    fn tick_tolerance(&self) -> Option<Tolerance> {
        Some(Tolerance { ns: MS * 2 / 100, runs: 3 })
    }
}

fn run(periods: Vec<i64>) -> Vec<u64> {
    let ticks = periods.len();
    let source = Source { now: 0, periods, k: 0 };
    let mut rt = Runtime::new(Watched::default(), source, Quiet, (), Profile::BAREMETAL, MS, Policy::Fault);
    for _ in 0..ticks {
        rt.step();
    }
    rt.program.hardware
}

#[test]
fn a_period_off_for_n_ticks_is_a_hardware_fault() {
    let slow = MS + MS / 10;
    assert_eq!(run(vec![MS, slow, slow, slow, slow, MS]), vec![3, 4], "ab der dritten Verletzung in Folge");
}

#[test]
fn an_outlier_or_jitter_within_tolerance_is_no_fault() {
    let slow = MS + MS / 10;
    assert!(run(vec![slow, slow, MS, slow, slow, MS + MS / 100, slow]).is_empty());
}

#[test]
fn a_clock_without_measurement_checks_nothing() {
    let source = Source { now: 0, periods: Vec::new(), k: 0 };
    let mut rt = Runtime::new(Watched::default(), source, Quiet, (), Profile::BAREMETAL, MS, Policy::Fault);
    for _ in 0..5 {
        rt.step();
    }
    assert!(rt.program.hardware.is_empty());
}
