//! Die Prüfungen mit Kalibrierung (12, 32; 7.2, 9.4.3, 13.8).
//!
//! **Was hier belegt wird.** Bis zur Kalibrierung war die zentrale
//! Zeitzusage der Sprache unbelegt: `takt cost` rechnete Operationen,
//! aber was sie dauern, konnte niemand sagen. Prüfung 12 meldete darum
//! einen Hinweis, Prüfung 32 gab es nicht, und `wcet` wurde abgelehnt
//! statt geprüft.
//!
//! Diese Tests zeigen beides — dass ohne Tabelle nichts behauptet wird,
//! und dass mit Tabelle geurteilt wird.

use takt_diag::{Policy, Span};
use takt_mir::hardware::{self, Target};

const HW: &str = "\
# takt-hw 1
[target.probe]
core_hz = 84000000
i32 = 11905
i64 = 35715
f32 = 11905
f64 = 1190500
mem = 23810
call = 47620
native = 0
t_io = 120000
";

fn ziel() -> Target {
    hardware::parse(HW).expect("lesbar").target("probe").expect("Ziel").clone()
}

fn compile(src: &str) -> takt_mir::Program {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

const KOPF: &str = "system:\n    language = 1\n    tick = 10 ms\n    tick_source = hw(\"sys/clock\")\n\n\
                    output led : bool @ hw(\"ui/led\") with safe = false\n\n";

/// **Eine Last, die passt, meldet nichts.**
#[test]
fn a_program_that_fits_produces_no_diagnostic() {
    let p =
        compile(&format!("{KOPF}machine m:\n    initial S\n\n    state S:\n        enter:\n            led = true\n"));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    assert!(d.is_empty(), "{d:?}");
}

/// **Eine Last, die nicht passt, ist ein Fehler mit Zahlen.**
///
/// 7.2: „Verletzung ist ein Compile-Fehler mit Vorschlag (Periode
/// erhöhen, Phase verschieben, Fault-Pfade verkleinern)."
#[test]
fn a_program_that_does_not_fit_is_an_error() {
    let p = compile(&format!(
        "{}machine m:\n    var acc : float = 0.0\n\n    initial S\n\n    state S:\n        loop:\n            \
         for i in range(2000):\n                acc = acc + 1.5\n            led = acc > 0.0\n",
        KOPF.replace("tick = 10 ms", "tick = 100 us")
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let fehler = d.iter().find(|d| d.is_error()).expect("ein Fehler");
    assert!(fehler.code == "SC-32", "{}", fehler.code);
    assert!(format!("{fehler}").contains("passt nicht"), "{fehler}");
    assert!(fehler.suggestion.is_some(), "7.2 verlangt einen Vorschlag");
}

/// **`wcet` wird geprüft, nicht mehr abgelehnt** (Prüfung 12).
#[test]
fn a_declared_wcet_is_checked_against_the_calibration() {
    let p = compile(&format!(
        "{KOPF}machine m with budget = {{wcet = 1 us}}:\n    var acc : float = 0.0\n\n    initial S\n\n    \
         state S:\n        loop:\n            for i in range(500):\n                acc = acc + 1.5\n            \
         led = acc > 0.0\n"
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let fehler = d.iter().find(|d| d.code == "SC-12").expect("SC-12 urteilt");
    assert!(fehler.is_error(), "{fehler}");
    assert!(format!("{fehler}").contains("je Aktivierung"), "{fehler}");
}

/// Ein eingehaltenes `wcet` meldet nichts.
#[test]
fn a_generous_wcet_passes() {
    let p = compile(&format!(
        "{KOPF}machine m with budget = {{wcet = 1 ms}}:\n    initial S\n\n    state S:\n        enter:\n            \
         led = true\n"
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    assert!(d.iter().all(|d| d.code != "SC-12"), "{d:?}");
}

/// **Ohne `tick_source` meldet Prüfung 32 das ausdrücklich.**
///
/// Sie verlangt zweierlei — die Ungleichung *und* dass die Tickquelle in
/// der Hardware-Konfiguration steht (7.1). Das Zweite ist unabhängig von
/// der Kalibrierung.
#[test]
fn a_missing_tick_source_is_reported() {
    let p = compile(&KOPF.replace("    tick_source = hw(\"sys/clock\")\n", ""));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let w = d.iter().find(|d| d.code == "SC-32").expect("SC-32 meldet");
    assert!(format!("{w}").contains("tick_source"), "{w}");
}

/// **Eine unvollständige Tabelle urteilt nicht, sondern nennt die Lücke.**
///
/// Ein „passt" auf zu kleiner Grundlage wäre schlimmer als kein Urteil:
/// Es sähe aus wie eine bestandene Prüfung.
#[test]
fn an_incomplete_calibration_names_what_it_misses() {
    let mut t = ziel();
    t.c_target.set(takt_mir::fns::CostClass::Mem, 0);
    let p =
        compile(&format!("{KOPF}machine m:\n    initial S\n\n    state S:\n        enter:\n            led = true\n"));
    let d = takt_sema::calibrated::check(&p, &t, Span::new(0, 0));
    let w = d.iter().find(|d| d.code == "SC-32").expect("SC-32 meldet");
    assert!(format!("{w}").contains("mem"), "die Meldung nennt die fehlende Klasse: {w}");
    assert!(!w.is_error(), "eine fehlende Messung ist kein Programmfehler");
}
