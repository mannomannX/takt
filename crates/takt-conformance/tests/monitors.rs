//! Laufzeitmonitore (13.3): Interpreter und erzeugter Code melden dieselben
//! Verletzungen im selben Tick (plan/m6.md 2.8).

use takt_conformance::stimulus::Stimulus;
use takt_mir::program::Program;

mod common;

const STIMULUS: &str = "t=2 cmd go\nt=20 cmd go\n";
const TICKS: u64 = 40;

fn corpus() -> Program {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/47_monitors.takt");
    let src = std::fs::read_to_string(path).expect("Korpus lesbar");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die `property`-/`assumption`-Zeilen eines Interpreter-Traces.
fn violations(trace: &str) -> Vec<String> {
    trace.lines().filter(|l| l.contains(" property ") || l.contains(" assumption ")).map(String::from).collect()
}

#[test]
fn the_monitors_lower_and_the_interpreter_finds_the_violations() {
    let p = corpus();
    let lowered = takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default());
    assert!(lowered.complete(), "{:?}", lowered.skipped);
    for i in 0..3 {
        assert!(lowered.ir.contains(&format!("define void @app_monitor_{i}(")), "Monitor {i}");
    }
    let stimulus = takt_interp::Trace::parse(STIMULUS).expect("Stimulus");
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    let r = takt_interp::run(&p, &stimulus, &options).expect("Lauf");
    assert_eq!(
        violations(&r.trace.render()),
        vec!["t=7 property disarms violated 2".to_string(), "t=20 assumption rare violated 20".to_string()]
    );
}

#[test]
fn native_monitors_report_the_same_violations_in_the_same_tick() {
    let Some(clang) = common::clang() else { return };
    let p = corpus();
    let stimulus = takt_interp::Trace::parse(STIMULUS).expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Command { name } => Some(Stimulus::cmd(l.tick, name)),
            _ => None,
        })
        .collect();
    let native = common::run_native_all_with(&clang, &p, "47_monitors.takt", TICKS, &inputs).expect("nativ");
    let mut reported = Vec::new();
    for line in native.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.len() == 4 && w[1] == "property" {
            let i: usize = w[2].parse().expect("Index");
            let prop = &p.properties[i];
            let word = if prop.assumption { "assumption" } else { "property" };
            reported.push(format!("{} {word} {} violated {}", w[0], prop.name, w[3]));
        }
    }
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    let r = takt_interp::run(&p, &stimulus, &options).expect("Lauf");
    let interpreted = r.trace.render();
    assert_eq!(reported, violations(&interpreted));
    assert_eq!(reported.len(), 2);
    // Eine Verletzung aendert am Lauf nichts (13.3: Eine Eigenschaft liest
    // nur, schreibt nie, loest keinen Fault aus): Ausgaenge und Faults
    // gleichen dem Interpreter im selben Lauf, und jeder Ausgang kommt an
    // (KON2-018).
    let names = |t: &str| -> std::collections::BTreeSet<String> {
        t.lines()
            .filter_map(|l| l.split_whitespace().nth(2).filter(|_| l.contains(" out ")).map(String::from))
            .collect()
    };
    assert_eq!(names(&interpreted), names(&native), "fehlende Ausgaenge");
    let diffs = takt_conformance::compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// Ein Programm fuer die Operatoren der Monitore (13.3, KON2-017): `stable`,
/// `never`, verschachteltes `not`/`and`/`or` mit `once`, ein Atom ueber einen
/// Input, der ungueltig wird, und eine Eigenschaft ueber die Ausgaenge, die
/// in `FAULTED` auf `safe` stehen.
const OPERATORS: &str = "system:
    language = 1
    tick     = 10 ms

command go
command stop
command probe

input  level : int in 0..100 @ hw(\"adc/level\") with max_age = 100 ms

output valve : bool @ hw(\"o/valve\") with safe = false
output busy  : bool @ hw(\"o/busy\") with safe = false

machine v:
    initial CLOSED

    state CLOSED:
        enter:
            valve = false
            busy = true
        when go: -> OPEN
        when stop: -> BROKEN

    state OPEN:
        enter:
            valve = true
        after 20 ms: -> CLOSED

    state BROKEN:
        loop:
            check false, \"broken\"

property chatter: always(valve implies stable[30 ms](valve)) with monitor = true
property exclusive: never(valve and not busy) with monitor = true
property nested: always(not (valve and not busy) or once[20 ms](go)) with monitor = true
property sensor: always(level > 10) with monitor = true
property fed: always(probe implies level > 10) with monitor = true
property running: always(busy) with monitor = true
property waits: always(go implies eventually[100 ms](valve)) with monitor = true
";

/// **Jeder Operator meldet nativ dieselben Verletzungen im selben Tick**
/// (13.3, KON2-017). Der Stimulus deckt die Raender: Der Input ist in Tick 0
/// ungueltig (das Atom zaehlt als falsch, Verletzung in Tick 0), das Ventil
/// schliesst vor Ende seines `stable`-Fensters, die Maschine faultet und
/// steht in `FAULTED` (Verletzung waehrend `FAULTED`), der Input wird im
/// letzten Tick ungueltig (Verletzung im letzten Tick), und ein `go` in
/// `FAULTED` laesst ein Fenster ueber das Laufende offen.
#[test]
fn every_operator_reports_alike() {
    let Some(clang) = common::clang() else { return };
    const RUN: u64 = 20;
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(OPERATORS, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let p = out.program.expect("Programm");
    let stimulus = takt_interp::Trace::parse(
        "t=1 in level 50\nt=3 cmd go\nt=10 cmd stop\nt=18 cmd go\nt=20 in level bad reason=Driver\nt=20 cmd probe\n",
    )
    .expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    let r =
        takt_interp::run(&p, &stimulus, &takt_interp::RunOptions { ticks: RUN, ..Default::default() }).expect("Lauf");
    let interpreted = r.trace.render();
    let native = common::run_native_all_with(&clang, &p, "monitor_operators", RUN, &inputs).expect("nativ");
    let diffs = takt_conformance::compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    assert!(
        r.properties.iter().any(|x| x.name == "waits" && x.outcome.text().starts_with("offen")),
        "das Fenster ueber das Laufende bleibt offen: {:?}",
        r.properties.iter().map(|x| (x.name.clone(), x.outcome.text())).collect::<Vec<_>>()
    );
    // Die Verletzungen selbst, exakt (13.3): Position und Tick, in dem ihr
    // Fenster schliesst. Eine Eigenschaft meldet nur ihre erste Verletzung;
    // `sensor` ist darum nur in Tick 0 verletzt (KON2-017 widerlegt), und den
    // letzten Tick prueft `fed`: Dort ist `level` ungueltig, und das Atom
    // zaehlt als falsch, auch wenn ein alter Wert daneben liegt.
    for (what, line) in [
        ("Tick 0, ungueltiges Atom", "t=0 property sensor violated 0"),
        ("stable", "t=6 property chatter violated 3"),
        ("FAULTED", "t=10 property running violated 10"),
        ("letzter Tick, ungueltiges Atom", "t=20 property fed violated 20"),
    ] {
        assert!(interpreted.contains(line), "{what}: `{line}` fehlt:\n{interpreted}");
    }
    assert_eq!(interpreted.matches(" property sensor violated").count(), 1, "nur die erste Verletzung:\n{interpreted}");
}

/// **Eine Teilformel in Klammern ist eine Eigenschaft** (13.3, Grammatik
/// `tprop_atom := … | "(" tprop ")"`): `once` unter `or` in Klammern ist
/// eine erlaubte Verschachtelung, kein Temporaloperator ausserhalb einer
/// Eigenschaft.
#[test]
fn a_parenthesized_temporal_formula_is_a_property() {
    let src = OPERATORS.replace(
        "property waits: always(go implies eventually[100 ms](valve)) with monitor = true",
        "property waits: always(valve implies (busy or once[20 ms](go))) with monitor = true",
    );
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}
