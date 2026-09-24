//! `persist` im erzeugten Code (5.9, Satz 9.4.4).
//!
//! Zwei Richtungen: `_persist_snapshot` liefert dieselben Bytes wie der
//! Interpreter, und `_persist_restore` fuehrt eine Journal-Nutzlast zu
//! denselben Outputs. Beides ueber `35_persist.takt`, das drei Typen
//! persistiert: Skalar, Record, Array.

mod common;

use takt_conformance::run::compare;
use takt_interp::nvm::Nvm;
use takt_interp::{RunOptions, Trace, Value, run};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::Program;

const TICKS: u64 = 40;

fn program() -> Program {
    corpus("35_persist.takt")
}

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die `persist`-Zeile eines Traces, ohne Tick.
fn persist_line(trace: &str) -> Option<String> {
    trace.lines().find_map(|l| l.split_once(" persist ").map(|(_, hex)| hex.trim().to_string()))
}

fn interpreted(p: &Program, nvm: Nvm) -> String {
    run(p, &Trace::default(), &RunOptions { ticks: TICKS, nvm, ..Default::default() }).expect("Lauf").trace.render()
}

/// Eine Nutzlast mit Werten, die keinem Default gleichen.
fn foreign_payload(p: &Program) -> Vec<u8> {
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let by_name = |name: &str| {
        let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == name).expect(name);
        (pv.type_hash, m.vars[pv.var.index()].ty)
    };
    let (h_cycles, t_cycles) = by_name("cycles");
    let (h_health, t_health) = by_name("health");
    let (h_trials, t_trials) = by_name("trials");
    let cycles = Value::Int(500);
    let health = Value::Record(vec![Value::Bool(false), Value::Int(7)]);
    let trials = Value::Array(vec![Value::Int(4), Value::Int(5)]);
    Nvm::payload(p, &[(h_cycles, &cycles, t_cycles), (h_health, &health, t_health), (h_trials, &trials, t_trials)])
        .expect("kodierbar")
}

#[test]
fn the_snapshot_matches_the_interpreter_byte_for_byte() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = program();
    let native =
        common::run_native_persist(&clang, &p, "35_persist_snapshot", TICKS, &[]).unwrap_or_else(|e| panic!("{e}"));
    let interp = interpreted(&p, Nvm::new());
    let want = persist_line(&interp).expect("Interpreter schreibt keine persist-Zeile");
    let got = persist_line(&native).expect("Rahmen schreibt keine persist-Zeile");
    assert_eq!(got, want, "Snapshot weicht ab\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}");
    assert!(compare(&interp, &native).is_empty());
}

#[test]
fn a_restored_payload_drives_both_sides_to_the_same_outputs() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = program();
    let payload = foreign_payload(&p);

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out count 500"), "Interpreter hat nicht geladen:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "35_persist_restore", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen: {diffs:?}\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}",
        diffs.len()
    );
    assert_eq!(persist_line(&native), persist_line(&interp), "Snapshot nach dem Laden weicht ab");
}

#[test]
fn a_payload_with_an_out_of_range_value_is_rejected_on_both_sides() {
    // 5.9: ungueltige Werte ergeben den Default. Der erzeugte Code prueft
    // die Range ebenso wie der Interpreter, sonst driftete s0 (9.4.4).
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = program();
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == "cycles").expect("cycles");
    let ty = m.vars[pv.var.index()].ty;
    // `cycles : int in 0..1000` — 5000 liegt ausserhalb.
    let payload = Nvm::payload(&p, &[(pv.type_hash, &Value::Int(5000), ty)]).expect("kodierbar");

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out count 3"), "Interpreter nahm den Default nicht:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "35_persist_range", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(diffs.is_empty(), "{} Abweichungen: {diffs:?}\n--- nativ ---\n{native}", diffs.len());
}

/// 11.2: Ein Enum mit Feldern liegt im Journal als Diskriminante und
/// Felder in kanonischer Form, dahinter Nullen — bytegleich auf beiden
/// Seiten.
#[test]
fn a_variant_with_fields_is_persisted_byte_for_byte() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("81_persist_variants.takt");
    let native = common::run_native_persist(&clang, &p, "81_snapshot", TICKS, &[]).unwrap_or_else(|e| panic!("{e}"));
    let interp = interpreted(&p, Nvm::new());
    let want = persist_line(&interp).expect("Interpreter schreibt keine persist-Zeile");
    let got = persist_line(&native).expect("Rahmen schreibt keine persist-Zeile");
    assert_eq!(got, want, "Snapshot weicht ab\n--- Interpreter ---\n{interp}\n--- nativ ---\n{native}");
    assert!(compare(&interp, &native).is_empty());
}

/// Ein geladener Stand mit `MOVE(-7, 3)` treibt beide Seiten gleich, und
/// der Schnappschuss danach ist wieder bytegleich.
#[test]
fn a_restored_variant_with_fields_drives_both_sides_the_same() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("81_persist_variants.takt");
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let by_name = |name: &str| {
        let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == name).expect(name);
        (pv.type_hash, m.vars[pv.var.index()].ty)
    };
    let (h_last, t_last) = by_name("last");
    let (h_count, t_count) = by_name("count");
    let last = Value::Enum { variant: 1, fields: vec![Value::Int(-7), Value::Int(3)] };
    let count = Value::Int(500);
    let payload = Nvm::payload(&p, &[(h_last, &last, t_last), (h_count, &count, t_count)]).expect("kodierbar");

    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let interp = interpreted(&p, nvm);
    assert!(interp.contains("out cmd MOVE(-7, 3)") && interp.contains("out seen 500"), "nicht geladen:\n{interp}");

    let native =
        common::run_native_persist(&clang, &p, "81_restore", TICKS, &payload).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interp, &native);
    assert!(diffs.is_empty(), "{} Abweichungen: {diffs:?}\n--- nativ ---\n{native}", diffs.len());
    assert_eq!(persist_line(&native), persist_line(&interp), "Snapshot nach dem Laden weicht ab");
}
