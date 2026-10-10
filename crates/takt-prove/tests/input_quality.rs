//! **Der Rand gehoert zum Modell** (3.5, 12.6, FB-372): Ein Input ist je
//! Tick gueltig oder nicht, ein ungueltiger faultet beim Lesen, und
//! `.or`/`.valid` fangen das ab. Ein Urteil „bewiesen" gilt damit auch fuer
//! fehlende, veraltete und verworfene Lieferungen, und ein Gegenbeispiel mit
//! ungueltigem Input bestaetigt der Interpreter. Die Tests brauchen einen
//! Solver (`takt_testkit::require`).

use takt_mir::Program;
use takt_prove::{Solver, Verdict, encode, find, prove};

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

/// Ein Programm um einen Temperaturfuehler: `input` und `loop` nach Wahl,
/// eine Maschine `m`, die in `RUN` bleiben soll. Die Eigenschaften meinen den
/// Normalbetrieb: Ein Abort oder Runtime-Fault von aussen fuehrt `m` nach
/// `FAULTED`, und die Annahme `nominal` schliesst ihn aus (13.3).
fn sensor(input: &str, body: &str, property: &str) -> Program {
    compile(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n{input}\n\
         input outer : bool @ hw(\"sys/outer_fault\")\n\
         output hot : bool @ hw(\"hot\") with safe = false\n\n\
         machine m:\n    var k : int in 0..9 = 0\n    initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           k = (k + 1) % 10\n{body}\n\n{property}\nassumption nominal: never(outer)\n"
    ))
}

const T: &str = "input t : int in 0..99 @ hw(\"adc/t\") with max_age = 100 ms";
const RUNS: &str = "property keeps_running: always(m.state == RUN)";

fn verdict(p: &Program, depth: u32) -> Option<Verdict> {
    let solver = solver()?;
    let model = encode(p).expect("kodierbar");
    let reports = prove(&model, p, depth, &solver, 60).expect("Solver laeuft");
    Some(reports.into_iter().next().expect("eine Eigenschaft").verdict)
}

/// **FB-372.** Das Programm liest `t` ohne Absicherung; ein ungueltiger
/// Input faultet (`SensorFault`) und die Maschine verlaesst `RUN`. Frueher
/// urteilte `takt prove` „bewiesen", weil das Modell jeden Input als gueltig
/// annahm; jetzt findet es das Gegenbeispiel, und der Interpreter bestaetigt
/// es.
#[test]
fn an_unguarded_read_of_an_invalid_input_violates_the_property() {
    let p = sensor(T, "            hot = t > 50", RUNS);
    let Some(v) = verdict(&p, 3) else { return };
    let Verdict::Violated { stimulus, .. } = &v else { panic!("{v:?}") };
    assert!(stimulus.contains(" bad") || stimulus.contains(" stale"), "{stimulus}");
}

/// `.or` faengt den ungueltigen Input ab; dann bleibt die Maschine in `RUN`.
#[test]
fn a_read_with_a_fallback_keeps_the_property() {
    let p = sensor(T, "            hot = t.or(99) > 50", RUNS);
    let Some(v) = verdict(&p, 3) else { return };
    assert!(matches!(v, Verdict::Proven { .. }), "{v:?}");
}

/// Eine von `.valid` dominierte Lesestelle faultet nie.
#[test]
fn a_read_under_valid_keeps_the_property() {
    let p = sensor(T, "            if t.valid:\n                hot = t > 50", RUNS);
    let Some(v) = verdict(&p, 3) else { return };
    assert!(matches!(v, Verdict::Proven { .. }), "{v:?}");
}

/// **Kurzschluss** (4.1): `k > 100 and t > 50` liest `t` nie, weil `k`
/// unter zehn bleibt. Ohne Kurzschluss im Modell galt das Lesen als
/// erreichbar, und der Interpreter haette das Gegenbeispiel verworfen.
#[test]
fn a_read_behind_a_false_operand_is_never_reached() {
    let p = sensor(T, "            hot = k > 100 and t > 50", RUNS);
    let Some(v) = verdict(&p, 3) else { return };
    assert!(matches!(v, Verdict::Proven { .. }), "{v:?}");
}

/// `Suspect` kommt vom Treiber oder unter `debounce` vom Rand (3.5, 12.6):
/// Ohne `debounce` wird `.suspect` erst wahr, wenn der Treiber seine
/// Lieferung selbst so nennt — mit einem Wert, den der Rand durchlaesst —,
/// und genau so liefert das Gegenbeispiel sie an den Interpreter. Mit
/// `debounce` kommt der Rand dazu.
#[test]
fn suspect_comes_from_the_driver_or_from_a_violation_under_debounce() {
    let never = "property calm: never(t.suspect)";
    let plain = sensor(T, "            hot = t.or(0) > 50", never);
    let Some(v) = verdict(&plain, 3) else { return };
    let Verdict::Violated { stimulus, .. } = &v else { panic!("ohne debounce: {v:?}") };
    assert!(stimulus.contains(" suspect"), "der Treiber meldet `Suspect`:\n{stimulus}");
    let debounced = format!("{T}, debounce = 2");
    let p = sensor(&debounced, "            hot = t.or(0) > 50", never);
    let Some(v) = verdict(&p, 3) else { return };
    assert!(matches!(v, Verdict::Violated { .. }), "mit debounce: {v:?}");
}

/// **Tunables** sind je Tick frei in ihrer Range (8.4): Eine Eigenschaft,
/// die nur unter dem Default haelt, ist verletzt, und das Gegenbeispiel
/// stellt den Wert ueber `tune`.
#[test]
fn a_tunable_takes_any_value_in_its_range() {
    let p = compile(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         tunable param LIMIT : int in 0..100 = 50\n\
         output hot : bool @ hw(\"hot\") with safe = false\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            hot = LIMIT > 60\n\n\
         property calm: never(hot)\n",
    );
    let Some(v) = verdict(&p, 2) else { return };
    let Verdict::Violated { stimulus, .. } = &v else { panic!("{v:?}") };
    assert!(stimulus.contains("tune LIMIT"), "{stimulus}");
}
