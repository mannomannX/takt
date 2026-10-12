//! Instanzen eines Programms in Faeden des Wirts, in logischer Zeit (12.11,
//! M11 Schritt 17): Jede hat ihre Arena, ihr Treiberobjekt, ihren
//! Job-Kontext und ihren Kern; sie teilen den Code und sonst nichts.

use std::collections::BTreeSet;
use std::sync::Barrier;

use rust_host::{Seed, instance};

/// Wie viele Instanzen zugleich rechnen.
const INSTANCES: usize = 8;

/// Vier Sekunden: 29 Runden je Instanz, jede mit einem Job.
const TICKS: u64 = 400;

/// Der Startwert der Instanz `i`: jede einen anderen.
fn seed_of(i: usize) -> i64 {
    17 + 29 * i64::try_from(i).expect("Index")
}

/// Der Stimulus des Interpreters zu einem Startwert: in jedem Tick frisch,
/// wie das Geraet ihn liefert.
fn stimulus(seed: i64) -> String {
    (0..TICKS).map(|k| format!("t={k} in seed {seed}\n")).collect()
}

/// **Acht Instanzen in acht Faeden rechnen jede wie der Interpreter** (12.11,
/// 13.1): Die Faeden beginnen zugleich und rechnen Frist fuer Frist
/// nebeneinander, jede Instanz mit eigenem Startwert, eigenem Job je Runde
/// und eigenem Schlaf. Teilten zwei Instanzen Zustand — Arena, Job-Slot,
/// Treiberobjekt oder die Leitung des Traces —, wiche eine vom Interpreter
/// mit ihrem Startwert ab. Jede tastete ihr eigenes Geraet ab, in ihrem
/// Faden, und die Woerter der ersten Runde sind acht verschiedene.
#[test]
fn eight_instances_in_threads_agree_with_the_interpreter() {
    let start = Barrier::new(INSTANCES);
    let runs: Vec<(String, Seed)> = std::thread::scope(|s| {
        let threads: Vec<_> = (0..INSTANCES)
            .map(|i| {
                let start = &start;
                std::thread::Builder::new()
                    .name(format!("instanz-{i}"))
                    .spawn_scoped(s, move || {
                        let mut arena = instance::Arena::new();
                        let mut seed = Seed::new(seed_of(i));
                        let program = instance::Program::init(&mut arena, &mut seed);
                        // Wie `testing::run`: Der Trace beginnt mit dem ersten Schritt.
                        takt_embed::testing::take();
                        let mut stepper = takt_embed::testing::Stepper::new(program, instance::TICK_NS, TICKS);
                        start.wait();
                        let mut trace = String::new();
                        while let Some(part) = stepper.step() {
                            trace.push_str(&part);
                            std::thread::yield_now();
                        }
                        drop(stepper);
                        (trace, seed)
                    })
                    .expect("Faden")
            })
            .collect();
        threads.into_iter().map(|t| t.join().expect("Instanz")).collect()
    });
    let source = concat!(env!("CARGO_MANIFEST_DIR"), "/takt/instance.takt");
    let mut first_words = BTreeSet::new();
    for (i, (trace, seed)) in runs.iter().enumerate() {
        if let Err(e) = takt_embed::testing::same_as_interpreter_with(source, TICKS, &stimulus(seed_of(i)), trace) {
            panic!("Instanz {i}: {e}\nTrace:\n{trace}");
        }
        assert!(trace.contains(" out count 29"), "Instanz {i} rechnete jede Runde:\n{trace}");
        assert!(seed.samples > 0, "Instanz {i} tastete ihr Geraet ab");
        assert_eq!(seed.thread.as_deref(), Some(format!("instanz-{i}").as_str()), "in ihrem Faden");
        let word = trace.lines().find(|l| l.contains(" out word ") && !l.ends_with(" 0")).expect("ein Wort");
        first_words.insert(word.split_whitespace().last().expect("Wert").to_string());
    }
    assert_eq!(first_words.len(), INSTANCES, "jede Instanz hasht ihren eigenen Startwert: {first_words:?}");
}
