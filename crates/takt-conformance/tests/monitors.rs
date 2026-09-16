//! Laufzeitmonitore (13.3): Interpreter und erzeugter Code melden dieselben
//! Verletzungen im selben Tick (plan/m6.md 2.8).

use takt_conformance::stimulus::Stimulus;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

mod common;

const STIMULUS: &str = "t=2 cmd go\nt=20 cmd go\n";
const TICKS: u64 = 40;

fn corpus() -> Program {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/47_monitors.takt");
    let src = std::fs::read_to_string(path).expect("Korpus lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
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
    let lowered = takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", "monitore");
    assert!(lowered.complete(), "{:?}", lowered.skipped);
    for i in 0..3 {
        assert!(lowered.ir.contains(&format!("define void @takt_monitor_{i}(")), "Monitor {i}");
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    assert_eq!(reported, violations(&r.trace.render()));
    assert_eq!(reported.len(), 2);
}
