//! Matrizen (3.11) im erzeugten Code: Das Korpusprogramm senkt sich
//! vollstaendig, die Skalarprodukte sind `fma`-Ketten, die LU springt bei
//! einem Pivot null in den Fault-Pfad — und die IR assembliert.

use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

fn corpus() -> Program {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/46_matrices.takt");
    let src = std::fs::read_to_string(path).expect("Korpus lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

#[test]
fn the_matrix_corpus_lowers_completely() {
    let p = corpus();
    let lowered = takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", "matrizen");
    assert!(lowered.complete(), "{:?}", lowered.skipped);
    let ir = &lowered.ir;
    assert!(ir.contains("call double @llvm.fma.f64("), "Skalarprodukte als fma-Ketten");
    assert!(ir.contains("call double @llvm.fabs.f64("), "Spaltenpivot ueber den Betrag");
    assert!(ir.contains("call double @llvm.sqrt.f64("), "Cholesky");
    assert!(ir.contains("label %fault_geometry_"), "ein Pivot null springt in den Fault-Pfad");
    // `solve` einer 2×2 (56 Byte) und `inv` einer 1×1 (20 Byte), je Maschine das Maximum.
    let scratch: Vec<Option<u32>> = p.machines.iter().map(|m| m.layout.scratch_bytes).collect();
    assert_eq!(scratch, vec![Some(56), Some(20)]);
}

#[test]
fn the_matrix_ir_assembles() {
    let clang = match find() {
        Clang::At(p) => Clang::At(p),
        Clang::Missing => {
            eprintln!("uebersprungen: clang nicht gefunden");
            return;
        }
    };
    let lowered = takt_llvm::lower::program(&corpus(), "x86_64-pc-windows-msvc", "matrizen");
    let dir = std::env::temp_dir().join("takt-llvm-matrizen");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Testverzeichnis anlegbar");
    let result = clang.assembles(&lowered.ir, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        panic!("die Matrix-IR assembliert nicht:\n{e}");
    }
}
