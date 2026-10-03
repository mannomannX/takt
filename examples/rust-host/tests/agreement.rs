//! Das Fuellventil laeuft wie im Interpreter (13.1).

use rust_host::{Tank, valve};

/// Zehn Sekunden: zwei Fuellungen, ab 3 s und ab 8 s.
const TICKS: u64 = 1000;

#[test]
fn the_valve_runs_like_the_interpreter() {
    let mut arena = valve::Arena::new();
    let mut tank = Tank::default();
    let trace = takt_embed::testing::run(valve::Program::init(&mut arena, &mut tank), valve::TICK_NS, TICKS);
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/takt/valve.takt");
    if let Err(e) = takt_embed::testing::same_as_interpreter(source, TICKS, &trace) {
        panic!("{e}\nTrace:\n{trace}");
    }
    assert_eq!(tank.openings, 2, "das Ventil sah die Fuellungen nicht");
    assert!(!tank.open, "nach 10 s ist das Ventil zu");
}

/// Zwei Instanzen in zwei Faeden, jede mit ihrer Arena und ihrem Tank
/// (12.11): Sie teilen den Code und sonst nichts, auch nicht den Trace.
#[test]
fn two_instances_run_side_by_side() {
    let runs: Vec<_> = (0..2)
        .map(|_| {
            std::thread::spawn(|| {
                let mut arena = valve::Arena::new();
                let mut tank = Tank::default();
                let program = valve::Program::init(&mut arena, &mut tank);
                let trace = takt_embed::testing::run(program, valve::TICK_NS, TICKS);
                (trace, tank.openings)
            })
        })
        .collect();
    let results: Vec<_> = runs.into_iter().map(|r| r.join().expect("Faden")).collect();
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0].1, 2);
}
