//! Gescopte Instanzen im erzeugten Code (Referenz 5.11, Satz 9.4.4).
//!
//! Der Differentialtest vergleicht nur Outputs — was ein `exit:`-Block
//! einer Instanz tut, ist dort unsichtbar, weil ihre Outputs danach
//! ohnehin auf `safe` stehen. Diese Datei prueft darum die Wirkungen, die
//! 5.11 dem Austritt zuschreibt und die im Trace stehen: `log`.

mod common;

use takt_llvm::toolchain::{Clang, find};

const PROGRAM: &str = "corpus-try/64_scoped_exit.takt";

fn corpus(name: &str) -> takt_mir::Program {
    let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.has_errors(), "{name}: {:?}", out.diagnostics);
    out.program.expect("Programm")
}

fn logs(trace: &str) -> Vec<String> {
    trace.lines().filter(|l| l.contains(" log ")).map(|l| l.trim().to_string()).collect()
}

/// 5.11: Beim Austritt laufen die `exit:`-Bloecke der Instanz — im
/// Interpreter und im erzeugten Code gleich oft.
#[test]
fn the_exit_blocks_of_a_scoped_instance_run_in_both() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus(PROGRAM);
    let ticks = 16;

    let interpreted =
        takt_interp::run(&p, &takt_interp::Trace::default(), &takt_interp::RunOptions { ticks, ..Default::default() })
            .expect("Lauf")
            .trace
            .render();
    let native = common::run_native_all(&clang, &p, "64_scoped_exit", ticks).expect("nativ");

    let a = logs(&interpreted);
    let b = logs(&native);
    assert!(!a.is_empty(), "der Interpreter meldet keinen Austritt:\n{interpreted}");
    assert_eq!(a.len(), b.len(), "Interpreter {a:?}\nnativ {b:?}");
}
