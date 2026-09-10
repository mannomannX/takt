//! Das statische Gate (Referenz 3.4, 9.4.3, 11.5; plan/m3.md Abschnitt 6).
//!
//! Geprueft wird an der MIR, nicht am Trace: Eine bewiesene Zuweisung laesst
//! keine Pruefung uebrig, eine unbewiesene schon, und gewarnt wird nur, wo
//! 3.4 es verlangt. Lemma 3.4 prueft `theorems.rs`.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::analysis::Report;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";
const OUT: &str = "output n : int in 0..99 @ hw(\"o/n\") with safe = 0\n\n";

/// Uebersetzt und liefert Programm samt Kennzahlen.
fn compile(body: &str) -> (Program, Report, Vec<String>) {
    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let warnings: Vec<String> = out.diagnostics.iter().filter(|d| !d.is_error()).map(|d| format!("{d}")).collect();
    (out.program.expect("Programm"), out.report, warnings)
}

/// Zahl der impliziten Pruefungen einer Ursache.
fn count(r: &Report, cause: &str) -> u32 {
    r.checks.get(cause).copied().unwrap_or(0)
}

#[test]
fn a_provable_assignment_needs_no_check() {
    // 3.4: „Ist das Intervall des Ausdrucks enthalten → keine Pruefung."
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            var b : int in 0..99 = a + 1
            n = b
",
    );
    assert_eq!(count(&r, "Declared"), 0, "0..9 plus 1 liegt in 0..99: {:?}", r.checks);
}

#[test]
fn an_unprovable_assignment_keeps_its_check() {
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 1, "0..999 passt nicht in 0..99: {:?}", r.checks);
}

#[test]
fn a_comparison_refines_the_branch() {
    // 3.4: „Vergleiche verfeinern Intervalle in Zweigen."
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            if big < 100:
                n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "im Zweig ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn a_passed_check_refines_what_follows() {
    // 5.6: ein bestandener `check` gilt fuer das Folgende.
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            check big < 100
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "nach dem `check` ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn only_a_check_in_a_loop_warns() {
    // 3.4, Warnpolitik: „Warnungen im engeren Sinn entstehen nur bei
    // impliziten Pruefungen in `for`-Schleifen und in Aktionsbloecken."
    let (_, outside, w_outside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(outside.warned, 0, "ausserhalb einer Schleife wird nicht gewarnt");
    assert!(!w_outside.iter().any(|w| w.contains("SC-24")), "{w_outside:?}");

    let (_, inside, w_inside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                n = big
",
    );
    assert_eq!(inside.warned, 1, "in der Schleife wird gewarnt");
    assert!(w_inside.iter().any(|w| w.contains("SC-24")), "{w_inside:?}");
}

#[test]
fn the_metric_names_the_cause() {
    // 3.4: die Kennzahl ist nach Ursache aufgeschluesselt (FB-19).
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    for cause in ["Declared", "Index", "Convert", "Arith"] {
        assert!(r.checks.contains_key(cause), "`{cause}` fehlt in der Kennzahl: {:?}", r.checks);
    }
    let text = r.lines().join("\n");
    assert!(text.contains("Declared 1"), "die Ursache steht im Report: {text}");
}

#[test]
fn a_proven_expression_is_narrowed_to_i32() {
    // 3.4, Lemma 3.4: „32 Bit, wo moeglich".
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a
",
    );
    assert!(r.narrowed > 0, "etwas wurde verengt: {} von {}", r.narrowed, r.integer_exprs);
    assert_eq!(r.narrowed, r.integer_exprs, "hier ist alles beweisbar schmal");
}

#[test]
fn every_machine_gets_a_cost_budget() {
    // 9.4.3: `B_m` und `F_m` als Vektoren ueber sieben Klassen.
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a + 1
",
    );
    let m = &p.machines[0];
    let b = m.budget.expect("Budget aus M3");
    assert!(b.activation.i32 > 0 || b.activation.i64 > 0, "die Addition zaehlt: {:?}", b.activation);
}

#[test]
fn the_memory_report_separates_reliable_from_open() {
    // 11.5: jeder Posten traegt seine Herkunft, summiert wird nur
    // Belastbares.
    let (p, _, _) = compile(
        "\
machine m:
    var x : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            n = x
",
    );
    let s = takt_mir::analysis::size::size(&p);
    assert!(s.total() > 0, "der Maschinenzustand zaehlt");
    assert!(s.has_open(), "Profilreserven fehlen noch (8.10, 13.8)");
    let text = s.lines().join("\n");
    assert!(text.contains("exakt") && text.contains("offen"), "beide Herkuenfte stehen da: {text}");
}
