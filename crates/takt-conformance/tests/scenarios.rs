//! Szenarien nativ (13.6, Satz 9.4.4): `takt test` fuehrt jedes Szenario
//! als eigenen Lauf im Interpreter; der Rahmen treibt dieselben Maschinen
//! samt Szenario, und die Outputs stimmen bis zum Ende des Szenarios
//! ueberein — auch das Verdikt kommt nativ.

use takt_conformance::compare;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::machine::MachineKind;
use takt_mir::program::Program;

mod common;

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

/// Der Tick einer Trace-Zeile.
fn tick_of(line: &str) -> u64 {
    line.strip_prefix("t=").and_then(|r| r.split_whitespace().next()).and_then(|t| t.parse().ok()).unwrap_or(0)
}

#[test]
fn every_scenario_runs_natively_like_takt_test() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("38_scenarios.takt");
    let scenarios: Vec<String> =
        p.machines.iter().filter(|m| m.kind == MachineKind::Scenario).map(|m| m.name.clone()).collect();
    assert!(!scenarios.is_empty(), "38_scenarios hat Szenarien");
    for name in &scenarios {
        let options = takt_interp::RunOptions { ticks: TICKS, scenario: Some(name.clone()), ..Default::default() };
        let result = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
        let interpreted = result.trace.render();
        // Der Interpreter endet mit dem Szenario (13.6); der Rahmen laeuft
        // die Ticks zu Ende, verglichen wird bis dorthin.
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        let native = common::run_native_scenario(&clang, &p, &format!("szenario_{name}"), name, TICKS)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let native: String = native.lines().filter(|l| tick_of(l) <= last).map(|l| format!("{l}\n")).collect();
        assert!(interpreted.contains("verdict"), "{name}: das Szenario hat nichts entschieden:\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty(),
            "{name}: {} Abweichungen:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
            diffs.len(),
            diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
            interpreted,
            native
        );
    }
}
