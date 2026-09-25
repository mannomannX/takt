//! Endlichkeit im erzeugten Code (4.2): Jede Gleitkommaoperation prueft
//! ihr Ergebnis, `sqrt` sein Argument. Dass die Zweige richtig faulten,
//! prueft der Differenztest an `77_float_faults.takt`.

use takt_mir::program::Program;

mod common;

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

fn functions_of(ir: &str, machine: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for line in ir.lines() {
        if line.starts_with("define") {
            inside = line.contains(&format!("@{machine}_"));
        }
        if inside {
            out.push_str(line);
            out.push('\n');
        }
        if line == "}" {
            inside = false;
        }
    }
    out
}

#[test]
fn every_float_result_is_checked_for_finiteness() {
    let p = corpus("77_float_faults.takt");
    let ir = common::ir_of(&p);

    // Auf den Bits: ohne Vorzeichen, verglichen mit dem Exponenten aus
    // lauter Einsen.
    let overflow = functions_of(&ir, "overflow");
    assert!(overflow.contains("bitcast double"), "das Produkt wird geprueft:\n{overflow}");
    assert!(overflow.contains("shl i64"), "das Vorzeichen faellt heraus:\n{overflow}");
    assert!(overflow.contains("icmp ult i64"), "gegen unendlich und NaN:\n{overflow}");

    let root = functions_of(&ir, "below_zero");
    assert!(root.contains("fcmp oge double"), "`sqrt` prueft sein Argument:\n{root}");

    let narrowing = functions_of(&ir, "narrowing");
    assert!(narrowing.contains("icmp ult i32"), "`as f32` prueft das Ergebnis:\n{narrowing}");

    // Das Matrixprodukt prueft jedes seiner vier Elemente. Kein Vergleich
    // in Gleitkomma: Ohne FPU waere das je Element ein Bibliotheksaufruf.
    let matrix = functions_of(&ir, "matrix");
    assert!(matrix.matches("bitcast double").count() >= 4, "je Element ein Vergleich:\n{matrix}");
    assert!(!ir.contains("@llvm.maximum") && !ir.contains("fcmp one"), "kein Vergleich in Gleitkomma");
}
