//! Misst die Genauigkeit der Tickquelle (12.2, 7.3).
//!
//! `cargo run -p takt-rt-linux --example jitter --release`
//!
//! Gemessen wird die Abweichung vom Sollzeitpunkt je Tick — das, was 7.3
//! `drift` nennt. Die Zahl haengt an der Umgebung, nicht am Programm:
//! Ohne `SCHED_FIFO` misst sie den Scheduler mit, und genau darum steht
//! der Befund daneben.

use takt_rt_core::{Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};
use takt_rt_linux::RealtimeClock;

#[derive(Default)]
struct Leer;

impl Program for Leer {
    fn tick(&mut self, _k: u64, _now: i64) {}
}

#[derive(Default)]
struct KeinWatchdog;

impl Watchdog for KeinWatchdog {
    fn kick(&mut self) {}
}

/// Sammelt die Abweichungen, ohne zu rechnen: 12.2 verlangt „nie
/// blockierend", und eine Statistik im Tick waere Arbeit im Tick-Thread.
#[derive(Default)]
struct Drift(Vec<i64>);

impl Sink for Drift {
    fn record(&mut self, t: &Tick) {
        self.0.push(t.drift);
    }
}

fn main() {
    let g = takt_rt_linux::prepare(None);
    const T0: i64 = 1_000_000; // 1 ms
    const N: u64 = 1000;

    let mut rt =
        Runtime::new(Leer, RealtimeClock::new(), KeinWatchdog, Drift::default(), Profile::LINUX_RT, T0, Policy::Fault);
    rt.run(N);

    let mut drift = std::mem::take(&mut rt.sink.0);
    drift.sort_unstable();
    let median = drift[drift.len() / 2];
    let p99 = drift[drift.len() * 99 / 100];
    let max = *drift.last().unwrap_or(&0);

    println!("{N} Ticks zu je {} us", T0 / 1000);
    println!("  Median  {:>8} ns", median);
    println!("  p99     {:>8} ns", p99);
    println!("  Maximum {:>8} ns", max);
    println!("  zu spaet: {} von {N}", rt.clock.late);
    println!();
    println!("Zeitgarantie (12.2): {}", if g.is_complete() { "haelt" } else { "haelt nicht" });
    for line in g.header_lines() {
        println!("  {line}");
    }
}
