//! Die Abnahme von M4: Interpreter ≡ nativ (13.8, Satz 9.4.4).
//!
//! Jedes Korpusprogramm wird zweimal ausgefuehrt — einmal vom
//! Referenzinterpreter, einmal als uebersetztes Binaerprogramm — und die
//! Outputs muessen Zeichen fuer Zeichen gleich sein.
//!
//! **Die Tests ueberspringen sich ohne clang**, wie die uebrigen
//! LLVM-Tests: Der Compiler baut ueberall, die Abnahme laeuft dort, wo
//! die Werkzeugkette steht.

use takt_conformance::compare;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

/// Die Korpusprogramme, die der Codegen vollstaendig senkt.
const KORPUS: [&str; 15] = [
    "01_minimal.takt",
    "20_native.takt",
    "19_faults.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
];

/// Wie viele Ticks verglichen werden.
///
/// Genug, dass jede `after`-Frist des Korpus feuert (die laengste ist
/// 500 ms bei 10 ms Tick), und wenig genug, dass ein Fehlschlag noch zu
/// lesen ist.
const TICKS: u64 = 60;

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Fuehrt dasselbe Programm im Interpreter aus.
fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// **Die Abnahme.** Interpreter und erzeugter Code liefern dieselben
/// Outputs (Satz 9.4.4).
#[test]
fn the_interpreter_and_the_generated_code_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut gescheitert = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        let Some(machine) = p.machines.first().map(|m| m.name.clone()) else { continue };
        let native = match common::run_native(&clang, &p, name, &machine, TICKS) {
            Ok(t) => t,
            Err(e) => {
                gescheitert.push(format!("{name}: laesst sich nicht bauen:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() {
            let liste: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            gescheitert.push(format!(
                "{name}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                liste.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(gescheitert.is_empty(), "{}", gescheitert.join("\n\n"));
}

mod common;

/// Die Grenzen der Abnahme stehen im Code, nicht nur im Plan.
///
/// Der Test gibt sie aus, damit ein gruener Lauf sie mitliefert — und er
/// prueft, dass die Liste nicht leer laeuft. Eine Grenze ohne Termin
/// waere eine Ausrede; jede traegt einen.
#[test]
fn the_limits_of_the_acceptance_are_written_down() {
    let limits = takt_conformance::LIMITS;
    assert!(limits.len() >= 5, "die Liste ist zu kurz, um vollstaendig zu sein: {}", limits.len());
    for l in limits {
        assert!(!l.was.is_empty() && !l.warum.is_empty(), "eine Grenze ohne Begruendung");
        assert!(!l.wann.is_empty(), "`{}` hat keinen Termin", l.was);
    }
    eprintln!("{}", takt_conformance::limits::report());
}

/// **Die Abnahme mit Eingaben** (12.5): Beide Seiten sehen denselben
/// Stimulus, und ihre Outputs stimmen ueberein.
///
/// Das ist die Haelfte, die bis Schritt 10 fehlte: Ein Lauf ohne
/// Eingaben prueft den Anfangszustand und seine Fortschreibung, nicht die
/// *Reaktion* auf Lieferungen. Ein Command ist die einfachste Form davon
/// (ein Puls, ein Byte, 8.5) — und die, die der Korpus benutzt.
#[test]
fn the_two_implementations_agree_on_recorded_inputs() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    // `16_timing` wartet auf `go` und faellt nach 200 ms zurueck; damit
    // laeuft jeder Uebergang mindestens einmal.
    let p = corpus("16_timing.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let stimulus = takt_interp::Trace::parse(
        "t=3 cmd go
t=40 cmd go
",
    )
    .expect("Stimulus");
    let inputs: Vec<(u64, String)> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Command { name } => Some((l.tick, name.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 2, "der Stimulus traegt zwei Commands");

    let native =
        common::run_native_with(&clang, &p, "eingaben", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Eingaben:
{}
--- Interpreter ---
{}
--- nativ ---
{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join(
            "
"
        ),
        interpreted,
        native
    );
}
