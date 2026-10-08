//! **Zusammengesetzte Werte im Beweiser** (M11 Schritt 27a): Ein Index
//! ausserhalb seines Arrays und ein fehlender Wert faulten im Modell wie im
//! Interpreter; der Solver findet den Pfad, und der Interpreter bestaetigt
//! ihn, oder er beweist die Stelle unerreichbar, wo die Intervallanalyse sie
//! stehen laesst. Die Tests brauchen einen Solver (`takt_testkit::require`).

use takt_mir::Program;
use takt_prove::{CheckVerdict, Solver, classify, encode, find};

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

/// Eine Maschine mit einem Array und einem Optional, die je Tick
/// `o = <expr>` rechnet; `i` kommt von aussen.
fn program(expr: &str) -> Program {
    compile(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input i : int in 0..9 @ hw(\"i\")\n\
         output o : int @ hw(\"o\") with safe = 0\n\n\
         machine m:\n    var xs : [4] int = [10, 20, 30, 40]\n    var maybe : int? = none\n\
         \x20   initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           if i.or(0) > 7:\n                maybe = i.or(0)\n\
         \x20           o = {expr}\n"
    ))
}

/// Die Stelle der Art `kind`, klassifiziert bis Tiefe 2.
fn site(p: &Program, kind: &str) -> Option<CheckVerdict> {
    let solver = solver()?;
    let model = encode(p).expect("kodierbar");
    let reports = classify(&model, p, 2, &solver, 60).expect("Solver laeuft");
    let found = reports.into_iter().find(|r| r.kind == kind).unwrap_or_else(|| panic!("keine Stelle `{kind}`"));
    Some(found.verdict)
}

/// Ein Index ab vier liegt ausserhalb; der Interpreter faultet mit `Range`.
#[test]
fn an_index_outside_its_array_faults() {
    let p = program("xs[i.or(0)]");
    let Some(v) = site(&p, "index") else { return };
    let CheckVerdict::Reachable { stimulus, .. } = &v else { panic!("{v:?}") };
    assert!(stimulus.contains("in i "), "{stimulus}");
}

/// `i - i / 2 * 2` ist null oder eins; die Intervallanalyse sieht `-8..9`
/// und laesst die Pruefung stehen, der Solver beweist sie unerreichbar.
#[test]
fn an_index_only_the_solver_keeps_inside_is_unreachable() {
    let p = program("xs[i.or(0) - i.or(0) / 2 * 2]");
    let Some(v) = site(&p, "index") else { return };
    assert!(matches!(v, CheckVerdict::Unreachable { .. }), "{v:?}");
}

/// Ein Index auf der linken Seite einer Zuweisung faultet an der Anweisung.
#[test]
fn an_index_on_the_left_faults_at_its_statement() {
    let p = compile(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input i : int in 0..9 @ hw(\"i\")\n\
         output o : int @ hw(\"o\") with safe = 0\n\n\
         machine m:\n    var xs : [4] int = [10, 20, 30, 40]\n    initial RUN\n\n\
         \x20   state RUN:\n        loop:\n            xs[i.or(0)] = 1\n            o = xs[0]\n",
    );
    let Some(v) = site(&p, "index") else { return };
    assert!(matches!(v, CheckVerdict::Reachable { .. }), "{v:?}");
}

/// Ein Optional ohne Wert, unbewacht benutzt, faultet mit `MissingValue`
/// (3.8); `maybe` hat erst ab einem Input ueber sieben einen Wert.
#[test]
fn an_unguarded_missing_value_faults() {
    let p = program("maybe");
    let Some(v) = site(&p, "missing") else { return };
    assert!(matches!(v, CheckVerdict::Reachable { .. }), "{v:?}");
}
