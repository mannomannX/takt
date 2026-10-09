//! **Jede implizite Pruefung ist ein Fault-Zweig** (4.1, 3.10, FB-372):
//! Ueberlauf in der Breite des Ergebnisses, Schiebebetrag, Konversion,
//! Definitionsbereich und Division faulten im Modell, wo der Interpreter
//! faultet. Eine erreichbare Stelle bestaetigt der Interpreter mit dem
//! Gegenbeispiel; eine unerreichbare ist bewiesen, auch wo die
//! Intervallanalyse sie nicht wegbeweist. Die Tests brauchen einen Solver
//! (`takt_testkit::require`).

use takt_mir::Program;
use takt_prove::{CheckVerdict, Solver, Verdict, classify, encode, find, prove};

fn compile(src: &str) -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn solver() -> Option<Solver> {
    let found = Some(find()).filter(|s| !matches!(s, Solver::Missing));
    takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren")
}

/// Eine Maschine, die je Tick `o = <expr>` rechnet, mit den Inputs nach Wahl.
fn program(inputs: &str, output: &str, expr: &str, property: &str) -> Program {
    compile(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n{inputs}\n\
         output o : {output} @ hw(\"o\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            o = {expr}\n\n{property}\n"
    ))
}

/// Die Stelle der Art `kind`, klassifiziert bis Tiefe 2.
fn site(p: &Program, kind: &str) -> Option<CheckVerdict> {
    let solver = solver()?;
    let model = encode(p).expect("kodierbar");
    assert!(model.notes.iter().all(|n| !n.contains("nicht modelliert")), "{:?}", model.notes);
    let reports = classify(&model, p, 2, &solver, 60).expect("Solver laeuft");
    let mut of_kind = reports.into_iter().filter(|r| r.kind == kind);
    let found = of_kind.next().unwrap_or_else(|| panic!("keine Stelle `{kind}`"));
    assert!(of_kind.next().is_none(), "mehr als eine Stelle `{kind}`");
    Some(found.verdict)
}

/// Erreichbar, vom Interpreter bestaetigt; der Stimulus enthaelt `want`.
fn reachable(v: &CheckVerdict, want: &str) {
    let CheckVerdict::Reachable { stimulus, .. } = v else { panic!("{v:?}") };
    assert!(stimulus.contains(want), "{stimulus}");
}

/// **Ueberlauf in schmaler Breite.** `x + 100` in `i8` laeuft ab `x = 28`
/// ueber; ohne Range liefert der Rand nur Werte der Breite.
#[test]
fn an_i8_sum_overflows() {
    let p = program("input x : i8 @ hw(\"x\")", "i8", "x.or(0) + 100", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "x ");
}

/// `x - x / 2` liegt fuer jedes `i8` in `-64..64`; die Intervallanalyse
/// sieht nur `-191..191` und laesst die Pruefung stehen, der Solver
/// beweist sie unerreichbar.
#[test]
fn an_overflow_only_the_solver_excludes_is_unreachable() {
    let p = program("input x : i8 @ hw(\"x\")", "i8", "x.or(0) - x.or(0) / 2", "");
    let Some(v) = site(&p, "ovf") else { return };
    assert!(matches!(v, CheckVerdict::Unreachable { .. }), "{v:?}");
}

/// **Ueberlauf in 64 Bit**: Summe und Produkt ueber die Vorzeichen und das
/// exakte Produkt (`MulOverflows`).
#[test]
fn an_int_sum_and_product_overflow() {
    let p = program("input a : int @ hw(\"a\")", "int", "a.or(0) + 1", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "a 9223372036854775807");
    let p = program("input a : int in 0..4000000000 @ hw(\"a\")", "int", "a.or(0) * a.or(0)", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "a ");
}

/// `abs` des kleinsten Werts laeuft ueber wie `-x` (FB-465).
#[test]
fn abs_of_the_smallest_value_overflows() {
    let p = program("input x : i8 @ hw(\"x\")", "i8", "abs(x.or(0))", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "x -128");
}

/// **Schiebebetrag** ausserhalb `0..63` ist ein Range-Fault (3.10).
#[test]
fn a_shift_by_its_width_faults() {
    let p = program("input s : int in 0..99 @ hw(\"s\")", "int", "1 << s.or(0)", "");
    let Some(v) = site(&p, "shift") else { return };
    reachable(&v, "s ");
}

/// `<<` wickelt in die Breite: `x << 1` ist fuer `x` ab 64 in `i8` negativ.
/// Ohne Wickelung hielte das Modell `o >= 0` fuer bewiesen.
#[test]
fn a_left_shift_wraps_into_its_width() {
    let solver = match solver() {
        Some(s) => s,
        None => return,
    };
    let p = program("input x : i8 in 0..127 @ hw(\"x\")", "i8", "x.or(0) << 1", "property sign: always(o >= 0)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let v = &reports[0].verdict;
    assert!(matches!(v, Verdict::Violated { .. }), "{v:?}");
}

/// **FB-386**, das Beispiel des Befunds: `(255 << 4) >> 4` ist in `u8`
/// fuenfzehn. Das Modell rechnete ohne Wickelung 255 und urteilte
/// „bewiesen".
#[test]
fn the_shift_of_fb_386_is_violated() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         output o : u8 @ hw(\"o\") with safe = 0\n\n\
         machine m:\n    var x : u8 = 255\n    initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           o = (x << 4) >> 4\n\nproperty y_is_large: always(o > 100)\n",
    );
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let v = &reports[0].verdict;
    assert!(matches!(v, Verdict::Violated { .. }), "{v:?}");
}

/// **Konversion**: `as u8` eines Werts ueber 255 ist ein Range-Fault.
#[test]
fn a_narrowing_conversion_faults() {
    let p = program("input x : int in 0..999 @ hw(\"x\")", "u8", "x.or(0) as u8", "");
    let Some(v) = site(&p, "conv") else { return };
    reachable(&v, "x ");
}

/// **Definitionsbereich**: `sqrt` unter null. Der Knoten steht am
/// Argument, der Interpreter meldet den Fault am Aufruf.
#[test]
fn a_square_root_below_zero_faults() {
    let p = program("input f : float in -10.0..10.0 @ hw(\"f\")", "float", "sqrt(f.or(0.0))", "");
    let Some(v) = site(&p, "dom") else { return };
    reachable(&v, "f -");
}

/// **Division durch null**: Der Knoten steht am Divisor, der Fault an der
/// Division.
#[test]
fn a_division_by_zero_faults() {
    let p = program("input d : int in 0..9 @ hw(\"d\")", "int", "100 / d.or(1)", "");
    let Some(v) = site(&p, "div") else { return };
    reachable(&v, "d 0");
}

/// **`u64` ohne Vorzeichen** (3.10): Die Summe laeuft erst ueber 2^64 − 1
/// ueber, die Differenz unter null, das Produkt mit dem exakten Wert. Mit
/// Vorzeichen gerechnet waere schon 2^63 ein Ueberlauf und null minus eins
/// keiner.
#[test]
fn a_u64_overflows_beyond_its_width_and_below_zero() {
    let p = program("input x : u64 @ hw(\"x\")", "u64", "x.or(1) + 1", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "x 18446744073709551615");
    let p = program("input x : u64 @ hw(\"x\")", "u64", "x.or(1) - 1", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "x 0");
    let p = program("input x : u64 @ hw(\"x\")", "u64", "x.or(0) * 3", "");
    let Some(v) = site(&p, "ovf") else { return };
    reachable(&v, "x ");
}

/// Ein `u64` ab 2^63 passt in kein `int`; nach `>> 1`, das Nullen
/// nachschiebt, passt jeder.
#[test]
fn a_u64_from_two_to_the_63_does_not_fit_an_int() {
    let p = program("input x : u64 @ hw(\"x\")", "int", "x.or(0) as int", "");
    let Some(v) = site(&p, "conv") else { return };
    reachable(&v, "x ");
    let p = program("input x : u64 @ hw(\"x\")", "int", "(x.or(0) >> 1) as int", "");
    let Some(v) = site(&p, "conv") else { return };
    assert!(matches!(v, CheckVerdict::Unreachable { .. }), "{v:?}");
}

/// Ein `u64` vergleicht ohne Vorzeichen: Ueber 2^63 − 1 liegt fuer ihn
/// ein Wert, mit Vorzeichen hielte das Modell `never` fuer bewiesen.
#[test]
fn a_u64_compares_without_sign() {
    let Some(solver) = solver() else { return };
    let p = program("input x : u64 @ hw(\"x\")", "u64", "x.or(0)", "property small: never(o > 9223372036854775807)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let Verdict::Violated { stimulus, .. } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    assert!(stimulus.contains("x "), "{stimulus}");
}
