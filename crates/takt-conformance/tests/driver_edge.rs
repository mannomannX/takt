//! Der defensive Treiberrand (12.6) im Lauf: Jeder Verstoss gegen den
//! Treibervertrag kommt einmal aus dem Stimulus, und der Golden-Trace zeigt
//! die Reaktion aus der Tabelle (M10 Schritt 29, FB-285). Der erzeugte
//! Rahmen urteilt mit demselben Kern und kommt zu denselben Ausgaben.

mod common;

use takt_conformance::run::compare;
use takt_conformance::stimulus::Stimulus;
use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

const SRC: &str = "\
system:
    language = 1
    tick     = 10 ms

record Pair layout little:
    a : u8
    b : u8

input  p     : int in 0..100    @ hw(\"adc/p\")
input  q     : int in 0..100    @ hw(\"adc/q\")
input  k     : int in 0..100    @ hw(\"dio/k\")
input  rx    : stream<line<16>> @ hw(\"uart/rx\") with max_rate = 200 Hz, capacity = 8
input  pairs : stream<Pair>     @ hw(\"can/rx\")  with max_rate = 400 Hz, capacity = 8
output p_ok  : bool             @ sim(\"o/p_ok\")
output q_ok  : bool             @ sim(\"o/q_ok\")
output k_ok  : bool             @ sim(\"o/k_ok\")
output lines : int in 0..100    @ sim(\"o/lines\")
output sum   : int in 0..1000   @ sim(\"o/sum\")

machine watch:
    initial RUN
    state RUN:
        loop:
            p_ok = p.valid
            q_ok = q.valid
            k_ok = k.valid
        on rx as e:
            lines = min(lines + 1, 100)
        on pairs as f:
            sum = min(sum + (f.data.a as int) + (f.data.b as int), 1000)
";

/// Jeder Verstoss einmal; die Kommentare nennen die Zeile aus 12.6.
const STIMULUS: &str = "\
t=0 in p 5
t=0 in q 6
t=0 in k 7
t=0 in rx \"a\"
t=0 in pairs 0x0102
# Zeile 1: 5 ms hinter dem Fenster (0, 10 ms], in der Toleranz eines Ticks
t=1 in p 6 t=15000000
# Zeile 1, zweiter Fall: jenseits der Toleranz -> wie Zeile 2, ganz `adc`
t=2 in p 7 t=-50000000
# Erholung: `adc` liefert wieder vertragsgemaess
t=3 in q 8
t=4 in p 9
# Zeile 2: Luecke in `seq`
t=5 in rx \"x\" seq=10
t=6 in rx \"y\" seq=11
# Zeile 2: mehr als MAXPT = 200 Hz * 10 ms = 2 Elemente
t=7 in rx \"1\"
t=7 in rx \"2\"
t=7 in rx \"3\"
t=8 in rx \"z\"
# Zeile 5: ein Byte zu viel fuer `Pair`
t=9 in pairs 0x010203
t=9 in pairs 0x0304
# Zeile 2: fallender Zeitstempel
t=10 in k 1 t=95000000
t=11 in k 2 t=94000000
t=12 in k 3
";

fn program() -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(SRC, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn interpreted() -> String {
    let stimulus = Trace::parse(STIMULUS).expect("Stimulus");
    let options = RunOptions { ticks: 14, ..Default::default() };
    takt_interp::run(&program(), &stimulus, &options).expect("Lauf").trace.render()
}

/// Die Zeilen des Treiberrands, in Reihenfolge.
fn driver_lines(trace: &str) -> Vec<&str> {
    trace.lines().filter(|l| l.contains(" driver ") || l.contains(" stream ")).collect()
}

#[test]
fn every_contract_violation_shows_in_the_golden_trace() {
    let trace = interpreted();
    assert_eq!(
        driver_lines(&trace),
        vec![
            "t=1 driver adc warped p",
            "t=2 driver adc degraded window",
            "t=3 driver adc recovered",
            "t=5 driver uart degraded seq",
            "t=6 driver uart recovered",
            "t=7 driver uart degraded maxpt",
            "t=8 driver uart recovered",
            "t=9 stream pairs dropped=0 overflowed=0 malformed=1",
            "t=11 driver dio degraded timestamp",
            "t=12 driver dio recovered",
        ],
        "{trace}"
    );
}

/// Zeile 2 trifft alle Inputs des Treibers, auch den, der in diesem Tick
/// nichts geliefert hat; die Erholung bringt jeden Kanal mit seiner
/// naechsten Lieferung zurueck.
#[test]
fn a_degraded_driver_takes_all_its_inputs_down() {
    let trace = interpreted();
    let at = |tick: u32| -> Vec<&str> {
        let head = format!("t={tick} out ");
        trace.lines().filter(|l| l.starts_with(&head)).collect()
    };
    assert!(at(2).contains(&"t=2 out p_ok false"), "{trace}");
    assert!(at(2).contains(&"t=2 out q_ok false"), "q liefert nicht und faellt doch: {trace}");
    assert!(at(3).contains(&"t=3 out q_ok true"), "{trace}");
    assert!(at(4).contains(&"t=4 out p_ok true"), "{trace}");
    assert!(!trace.contains("out k_ok false\nt=2"), "`dio` bleibt unberuehrt");
}

/// Verworfene Elemente kommen nicht an: `x` (Luecke) und `1 2 3` (MAXPT)
/// fehlen, `a y z` zaehlen; vom Paar-Strom nur die beiden guten.
#[test]
fn rejected_elements_are_not_delivered() {
    let trace = interpreted();
    let last = |name: &str| {
        trace.lines().rfind(|l| l.contains(&format!(" out {name} "))).map(|l| l.rsplit(' ').next().unwrap_or(""))
    };
    assert_eq!(last("lines"), Some("3"), "{trace}");
    assert_eq!(last("sum"), Some("10"), "1+2 und 3+4: {trace}");
}

/// Ohne `t=` und `seq=` bleibt jeder bisherige Stimulus, was er war: Die
/// Voreinstellung ist die Tickgrenze und die lueckenlose Folge.
#[test]
fn a_stimulus_without_timestamps_keeps_the_contract() {
    let stimulus = Trace::parse("t=0 in rx \"a\"\nt=1 in rx \"b\"\nt=1 in rx \"c\"\nt=3 in p 5\n").expect("Stimulus");
    let options = RunOptions { ticks: 5, ..Default::default() };
    let trace = takt_interp::run(&program(), &stimulus, &options).expect("Lauf").trace.render();
    assert!(driver_lines(&trace).is_empty(), "{trace}");
}

/// Satz 9.4.4 fuer die Randfaelle: Der erzeugte Rahmen ruft den Kern ueber
/// `takt_edge_*` und schreibt dieselben Ausgaben und dieselben Zeilen
/// `driver`; die Zeile `stream` mit den Zaehlern schreibt nur der
/// Interpreter.
#[test]
fn the_native_frame_judges_like_the_interpreter() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = program();
    let inputs = Stimulus::from_trace(&Trace::parse(STIMULUS).expect("Stimulus"));
    let native = common::run_native_all_with(&clang, &p, "treiberrand", 14, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let interpreted = interpreted();
    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        diffs.len(),
        diffs.iter().take(8).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n")
    );
    let driver = |trace: &str| trace.lines().filter(|l| l.contains(" driver ")).map(str::to_string).collect::<Vec<_>>();
    assert_eq!(driver(&native), driver(&interpreted), "--- nativ ---\n{native}");
}
