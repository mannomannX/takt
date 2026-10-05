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

/// Eine Arena voller Muell, wie Speicher, den niemand beschrieben hat.
fn dirty_arena() -> valve::Arena {
    let mut arena = valve::Arena::new();
    // SAFETY: `Arena` ist ein Byteblock ohne Invarianten vor `init`.
    unsafe { core::ptr::write_bytes(core::ptr::from_mut(&mut arena).cast::<u8>(), 0xA5, size_of::<valve::Arena>()) };
    arena
}

/// **Eine Huelle vor `init` liest die Arena nie** (12.11, GEN-026): Wer
/// `Program::new` ohne Journal faehrt, bekommt denselben Lauf wie mit
/// `Program::init` — jede schreibende Methode beginnt das Programm, jede
/// lesende antwortet vorher ohne die Arena.
#[test]
fn a_program_that_was_never_initialized_starts_on_first_use() {
    let reference = {
        let mut arena = valve::Arena::new();
        let mut tank = Tank::default();
        takt_embed::testing::run(valve::Program::init(&mut arena, &mut tank), valve::TICK_NS, 400)
    };
    let mut arena = dirty_arena();
    let mut tank = Tank::default();
    let program = valve::Program::new(&mut arena, &mut tank);
    assert!(!takt_embed::rt::Program::sleep_allowed(&program), "vor `init` schlaeft nichts");
    assert_eq!(takt_embed::rt::Program::next_deadline(&program), None);
    assert_eq!(takt_embed::rt::Program::next_run(&program), None);
    let trace = takt_embed::testing::run(program, valve::TICK_NS, 400);
    assert_eq!(trace, reference);
}

/// **Der Job-Kontext rechnet nie auf einer Arena vor `init`** (4.5,
/// GEN-026): Den Griff gibt es erst, wenn das Programm begonnen hat.
#[test]
fn the_job_context_never_runs_before_init() {
    let mut arena = dirty_arena();
    let mut tank = Tank::default();
    let mut program = valve::Program::new(&mut arena, &mut tank);
    let mut jobs = takt_embed::Program::jobs(&mut program).expect("Griff");
    takt_embed::Jobs::work(&mut jobs);
    assert!(!takt_embed::rt::Program::dispatch_job(&mut program), "kein Auftrag aus Muell");
}
