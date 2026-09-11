//! Strikte FP-Semantik (4.2): Die erzeugte IR traegt keine Fast-Math-Flags.
//!
//! `plan/m4.md` Abschnitt 8 nennt diesen Test als Gegenmassnahme gegen das
//! Risiko „Strikte FP wird von LLVM stillschweigend gelockert". Er ist
//! zugleich die Absicherung der Entscheidung aus 4.1: Der Textausgeber
//! schreibt die Flags von Hand — also muss ihre *Abwesenheit* geprueft
//! werden, und Abwesenheit ist vollstaendig pruefbar.

use takt_llvm::emit::{Module, float_literal};
use takt_llvm::ty::LlvmType;

/// Die sieben Flags, die LLVM an Fliesskomma-Instruktionen erlaubt.
///
/// `contract` ist der wichtigste: Er erlaubt LLVM, `a * b + c` zu einem
/// `fma` zusammenzuziehen. 4.2 verbietet das ausdruecklich — „was man
/// schreibt, bekommt man" —, und `fma(a, b, c)` ist die explizite
/// Primitive.
const FLAGS: [&str; 7] = ["fast", "nnan", "ninf", "nsz", "arcp", "contract", "reassoc"];

/// Baut ein Modul mit jeder Fliesskomma-Instruktion, die der Codegen
/// erzeugen kann.
fn module_with_every_float_instruction() -> String {
    let mut m = Module::new("fp", "x86_64-unknown-linux-gnu");
    let d = LlvmType::F64;
    let args = m.begin("alles", &d, &[d.clone(), d.clone()]);
    let (a, b) = (args[0], args[1]);
    for op in ["fadd", "fsub", "fmul", "fdiv", "frem"] {
        m.inst(&format!("{op} double {a}, {b}"));
    }
    m.inst(&format!("fneg double {a}"));
    for cc in ["olt", "ole", "ogt", "oge", "oeq", "one"] {
        m.inst(&format!("fcmp {cc} double {a}, {b}"));
    }
    m.end(Some((&d, a.to_string())));
    m.finish()
}

#[test]
fn no_float_instruction_carries_a_fast_math_flag() {
    let ir = module_with_every_float_instruction();
    for line in ir.lines() {
        let Some(op) = line.split_whitespace().find(|w| w.starts_with('f')) else { continue };
        if !["fadd", "fsub", "fmul", "fdiv", "frem", "fneg", "fcmp"].contains(&op) {
            continue;
        }
        for flag in FLAGS {
            assert!(!line.split_whitespace().any(|w| w == flag), "Zeile traegt `{flag}`: {line}");
        }
    }
}

/// Der Test oben prueft nur, was der Codegen heute erzeugt. Dieser prueft
/// die ganze Datei — auch Zeilen, die spaeter dazukommen.
#[test]
fn the_whole_module_is_free_of_fast_math_flags() {
    let ir = module_with_every_float_instruction();
    for (i, line) in ir.lines().enumerate() {
        // Kommentare duerfen die Woerter nennen; sie tun es im Kopf.
        let code = line.split(';').next().unwrap_or("");
        for flag in FLAGS {
            assert!(!code.split_whitespace().any(|w| w == flag), "Zeile {}: `{flag}` in `{code}`", i + 1);
        }
    }
}

/// 4.2, Satz 9.4.4: Ein Fliesskommaliteral steht als Bitmuster, nicht
/// dezimal. `0.1` dezimal waere eine andere Zahl als `0.1` im Programm,
/// und die Abweichung faende niemand.
#[test]
fn float_literals_are_written_as_bit_patterns() {
    assert_eq!(float_literal(1.0, &LlvmType::F64), "0x3FF0000000000000");
    assert_eq!(float_literal(0.1, &LlvmType::F64), format!("0x{:016X}", 0.1f64.to_bits()));
    // Bei `float` erwartet LLVM das Muster des zugehoerigen `double`.
    assert_eq!(float_literal(0.1, &LlvmType::F32), format!("0x{:016X}", f64::from(0.1f32).to_bits()));
}

/// 11.3: reproduzierbare Builds — dieselbe Eingabe ergibt dieselbe Datei,
/// ohne Zeitstempel und ohne Pfade.
#[test]
fn the_same_input_produces_the_same_text() {
    assert_eq!(module_with_every_float_instruction(), module_with_every_float_instruction());
}

#[test]
fn the_module_header_carries_no_timestamp_or_path() {
    let ir = module_with_every_float_instruction();
    let head: Vec<&str> = ir.lines().take_while(|l| !l.starts_with("define")).collect();
    let text = head.join("\n");
    assert!(!text.contains("2026"), "ein Zeitstempel steht im Kopf: {text}");
    assert!(!text.contains(":\\") && !text.contains("/takt/"), "ein Pfad steht im Kopf: {text}");
    assert!(text.contains("target triple"), "das Target gehoert in den Kopf (11.3)");
}
