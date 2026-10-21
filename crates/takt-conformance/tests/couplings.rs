//! Die Kopplung zweier Programme eines Kerns (12.11, M11 Schritt 16): Der
//! Leser prueft den Zeitstempel jedes gekoppelten Eingangs gegen die
//! Zeitachse. Ein falscher macht die Abtastung `Stale` und ist ein
//! Zeitfehler wie ein Ueberlauf (7.3), nie still ein anderer Wert.
//!
//! Der Schreiber ist hier der Testtreiber des Testwirts: Er liefert, was der
//! Stimulus sagt, samt Zeitstempel. Eine verspaetete Freigabe ist ein Stempel
//! der Grenze davor. Der Interpreter kennt die Kopplung nicht; er bekommt an
//! ihrer Stelle, was der Rahmen daraus macht: die Abtastung `stale` und
//! `runtime Overrun`.

mod common;

use takt_conformance::couple::{self, Writer};
use takt_conformance::run::compare;
use takt_conformance::stimulus::Stimulus;
use takt_conformance::testhost::Run;

const MS: i64 = 1_000_000;

fn program(tick: &str) -> takt_mir::Program {
    let src = format!(
        "system:\n    language = 1\n    tick     = {tick}\n\n\
         input  seen : int in 0..1000000 @ hw(\"pair/seen\") with max_age = 50 ms\n\
         output last : int in 0..1000000 @ hw(\"pair/last\") with safe = 0\n\n\
         machine reader:\n    initial RUN\n\n    state RUN:\n        loop:\n            last = seen if seen.valid else 0\n"
    );
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Kopplung von `pair/seen` an `pair/count` eines Schreibers mit Tick
/// `writer_ns`, `timing` und, bei gleichem Tick, `first`.
fn coupling(writer_ns: i64, timing: &str) -> takt_mir::hardware::Hardware {
    takt_mir::hardware::parse(&format!(
        "# takt-hw 18\n[channel pair/seen]\ndirection = input\nfrom = \"pair/count\"\nfrom_tick_ns = {writer_ns}\n\
         from_timing = {timing}\n\n[channel pair/last]\ndirection = output\n"
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

/// Faehrt den Leser im Produktrahmen mit `product` als Lieferungen des
/// Schreibers und vergleicht mit dem Interpreter auf `interpreted`.
fn agree(
    label: &str,
    p: &takt_mir::Program,
    hw: &takt_mir::hardware::Hardware,
    ticks: u64,
    product: &str,
    interpreted: &str,
) -> String {
    let Some(clang) = common::clang() else { return String::new() };
    let stimulus = takt_interp::Trace::parse(product).expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    let run = Run { ticks, stimulus: &inputs, hardware: Some(hw), ..Run::default() };
    let native = common::run_product(&clang, p, label, &run).unwrap_or_else(|e| panic!("{label}: {e}"));
    let reference = takt_interp::Trace::parse(interpreted).expect("Stimulus");
    let options = takt_interp::RunOptions { ticks, inputs: true, ..Default::default() };
    let expected = takt_interp::run(p, &reference, &options).expect("Lauf").trace.render();
    let diffs = compare(&expected, &native);
    assert!(diffs.is_empty(), "{label}: {diffs:?}\n--- Interpreter ---\n{expected}\n--- nativ ---\n{native}");
    native
}

/// Die Lieferungen eines hoeheren Schreibers mit Tick 1 ms unter `asap` an
/// einen Leser mit Tick 10 ms: je Tick der Wert mit dem faelligen Stempel
/// (`couple::due`); mit `late` an Tick 3 der Stempel des Ticks davor.
fn from_higher(late: bool) -> String {
    let w = Writer { tick_ns: MS, boundary: false, higher: true };
    (0..=6)
        .filter_map(|k: u64| {
            let due = couple::due(10 * MS, k, w)?;
            let stamp = if late && k == 3 { due - MS } else { due };
            Some(format!("t={k} in seen {} t={stamp}\n", due / MS))
        })
        .collect()
}

/// **Der festgelegte Wert** (12.11): Stimmt der Stempel, sieht der Leser den
/// Wert, und es gibt keinen Zeitfehler.
#[test]
fn a_coupled_input_with_the_expected_stamp_reads_its_value() {
    let p = program("10 ms");
    let stimulus = from_higher(false);
    let native = agree("coupled-on-time", &p, &coupling(MS, "asap"), 6, &stimulus, &stimulus);
    assert!(!native.contains("runtime Overrun"), "{native}");
}

/// **Eine verspaetete Freigabe ist ein Zeitfehler, kein anderer Wert**
/// (12.11, 7.3): Der Schreiber liefert an Tick 3 noch den Stempel des Ticks
/// davor. Der Rahmen macht die Abtastung `Stale` und meldet
/// `Runtime(Overrun)` in diesem Tick, wie der Interpreter mit `stale` und
/// `runtime Overrun` an derselben Stelle.
#[test]
fn a_late_release_is_a_timing_fault() {
    let p = program("10 ms");
    let on_time = from_higher(false);
    let line = on_time.lines().find(|l| l.starts_with("t=3 ")).expect("Tick 3");
    let reference = on_time.replace(&format!("{line}\n"), "t=3 in seen stale\nt=3 runtime Overrun\n");
    let native = agree("coupled-late", &p, &coupling(MS, "asap"), 6, &from_higher(true), &reference);
    assert!(native.contains("t=3 runtime Overrun"), "{native}");
}

/// **Die Gegenrichtung** (12.11): Ein Leser mit Tick 1 ms liest einen
/// niedrigeren Schreiber mit Tick 10 ms unter `boundary`. An seiner Grenze b
/// sieht er die Freigabe der letzten Grenze des Schreibers vor b: am Ursprung
/// keine, ab Tick 2 (b = 1 ms) die von 0, ab Tick 12 (b = 11 ms) die von
/// 10 ms. Liefert der Schreiber an Tick 11 (b = 10 ms) schon die Freigabe von
/// 10 ms, hat er vor dem Leser freigegeben: ein Zeitfehler.
#[test]
fn a_lower_writer_is_seen_one_boundary_later() {
    let p = program("1 ms");
    let hw = coupling(10 * MS, "boundary");
    let w = Writer { tick_ns: 10 * MS, boundary: true, higher: false };
    let on_time: String = (0..=22)
        .filter_map(|k| couple::due(MS, k, w).map(|stamp| format!("t={k} in seen {} t={stamp}\n", stamp / MS)))
        .collect();
    let native = agree("coupled-lower", &p, &hw, 22, &on_time, &on_time);
    assert!(!native.contains("runtime Overrun"), "{native}");
    let early = on_time.replace("t=11 in seen 0 t=0\n", "t=11 in seen 10 t=10000000\n");
    let reference = on_time.replace("t=11 in seen 0 t=0\n", "t=11 in seen stale\nt=11 runtime Overrun\n");
    assert_ne!(early, on_time, "Tick 11 erwartet die Freigabe von 0");
    let native = agree("coupled-early", &p, &hw, 22, &early, &reference);
    assert!(native.contains("t=11 runtime Overrun"), "{native}");
}
