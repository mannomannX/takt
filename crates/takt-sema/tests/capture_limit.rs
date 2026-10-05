//! Pruefung 18 (8.7): Ein Muster bindet hoechstens 16 Platzhalter; `{_}`
//! zaehlt nicht mit (SYN-038).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

/// `bound` Platzhalter `{cI:int}`, durch Kommas getrennt, dahinter `;{_}`.
fn pattern(bound: usize) -> String {
    let captures: Vec<String> = (0..bound).map(|i| format!("{{c{i}:int}}")).collect();
    format!("{};{{_}}", captures.join(","))
}

/// Ein Handler auf einem Zeilenstrom, der den letzten Platzhalter ausgibt.
fn program(bound: usize) -> String {
    format!(
        "system:
    language = 1
    tick     = 10 ms

input  rx   : stream<line<256>> @ hw(\"u/rx\") with capacity = 8, max_rate = 100 Hz
output last : int in 0..99 @ hw(\"o/last\") with safe = 0

machine m:
    initial RUN
    state RUN:
        on rx matches \"{}\" as l:
            last = min(max(l.c{}, 0), 99)
",
        pattern(bound),
        bound - 1
    )
}

fn errors(src: &str) -> Vec<String> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// Sechzehn bindende Platzhalter und ein `{_}` uebersetzen, und der
/// sechzehnte bindet seinen Wert.
#[test]
fn sixteen_captures_and_an_anonymous_one_bind() {
    let src = program(16);
    assert!(errors(&src).is_empty(), "{:?}", errors(&src));
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    let values: Vec<String> = (1..=16).map(|i| i.to_string()).collect();
    let stimulus = Trace::parse(&format!("t=1 in rx {};rest\n", values.join(","))).expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 3, ..Default::default() }).expect("Lauf").trace.render();
    assert!(t.contains("out last 16"), "{t}");
}

/// Der siebzehnte bindende Platzhalter ist genau ein Fehler der Pruefung 18.
#[test]
fn a_seventeenth_capture_is_check_18() {
    let e = errors(&program(17));
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("SC-18") && e[0].contains("17 Platzhalter"), "{e:?}");
}
