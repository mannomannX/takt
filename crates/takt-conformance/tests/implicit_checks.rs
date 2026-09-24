//! Implizite Pruefungen im erzeugten Code (4.1, 3.4).
//!
//! Was die Intervallanalyse nicht wegbeweist, steht als Zweig in den
//! Fault-Trampolin; was sie beweist, hinterlaesst keinen. Beides liest
//! sich an der IR ab; dass die Zweige auch *richtig* faulten, prueft der
//! Differenztest an `75_implicit_checks.takt`.

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

/// Die Rumpftexte aller Funktionen einer Maschine.
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
fn every_unproven_check_becomes_a_branch_and_every_proven_one_none() {
    let p = corpus("75_implicit_checks.takt");
    let ir = common::ir_of(&p);

    let ovf = functions_of(&ir, "overflow_u8");
    assert!(ovf.contains("@llvm.uadd.with.overflow.i8("), "u8-Addition mit Ueberlaufpruefung:\n{ovf}");

    let div = functions_of(&ir, "div_zero");
    assert!(div.contains("geprueft_div_"), "Divisor gegen null geprueft:\n{div}");
    assert!(!div.contains("geprueft_ovf_"), "12 / 1..3 kann nicht ueberlaufen:\n{div}");

    let shift = functions_of(&ir, "shift_amount");
    assert!(shift.contains("geprueft_shift_"), "Schiebebetrag geprueft:\n{shift}");
    assert!(shift.contains("trunc i64"), "der Betrag kommt auf die Breite des Werts:\n{shift}");

    let conv = functions_of(&ir, "convert_loss");
    assert!(conv.contains("geprueft_conv_"), "`as u8` geprueft:\n{conv}");
    assert!(conv.contains("icmp sle i64"), "die Obergrenze 255 wird geprueft:\n{conv}");

    let idx = functions_of(&ir, "index_write");
    assert!(idx.contains("index_ok"), "der Index der Zuweisungsstelle wird geprueft:\n{idx}");

    let fine = functions_of(&ir, "proven_fine");
    for mark in ["geprueft_", "index_ok", "with.overflow", "; Range"] {
        assert!(!fine.contains(mark), "`proven_fine` traegt `{mark}`:\n{fine}");
    }
    // Lemma 3.4: `n + 1` mit `n in 0..100` rechnet in 32 Bit.
    let hot = functions_of(&ir, "proven_fine_loop");
    assert!(hot.contains("add i32"), "die Addition laeuft in i32:
{hot}");
    assert!(!hot.contains("add i64"), "keine 64-Bit-Addition im `loop:`:
{hot}");
}
