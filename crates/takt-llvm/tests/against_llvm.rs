//! Die erzeugte IR gegen echtes LLVM (plan/m4.md 4.1).
//!
//! Bis hierher pruefen die Tests die IR als *Text*. Das findet, was falsch
//! geschrieben ist, aber nicht, was falsch *gemeint* ist: Eine Zeile kann
//! richtig aussehen und trotzdem nicht assemblieren, und ein Bitmuster
//! kann eine andere Zahl sein als gedacht. Diese Tests uebersetzen die IR
//! mit `clang` und fuehren sie aus.
//!
//! **Sie ueberspringen sich, wenn `clang` fehlt.** Der Compiler baut und
//! testet ohne LLVM — das ist der Gewinn der Textausgabe (4.1). Wer die
//! Abnahme fahren will, braucht es; wer am Parser arbeitet, nicht. Das
//! Ueberspringen wird gemeldet, damit es nicht unbemerkt bleibt.

use takt_llvm::emit::{Module, float_literal};
use takt_llvm::toolchain::{Clang, find};
use takt_llvm::ty::LlvmType;

/// Sucht `clang` oder meldet, dass der Test uebersprungen wird.
macro_rules! clang_or_skip {
    () => {{
        match find() {
            Clang::At(p) => Clang::At(p),
            Clang::Missing => {
                eprintln!("uebersprungen: clang nicht gefunden (TAKT_CLANG setzen oder LLVM installieren)");
                return;
            }
        }
    }};
}

/// Ein Verzeichnis, das sich nach dem Test wieder aufraeumt.
struct Temp(std::path::PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("takt-llvm-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Testverzeichnis anlegbar");
        Temp(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Das Modul aus dem Beispiel `dump`: jede Fliesskomma-Instruktion.
fn every_float_instruction() -> String {
    let mut m = Module::new("fp", "x86_64-pc-windows-msvc");
    let d = LlvmType::F64;
    let args = m.begin("alles", &d, &[d.clone(), d.clone()]);
    let (a, b) = (args[0], args[1]);
    let mut last = a;
    for op in ["fadd", "fsub", "fmul", "fdiv", "frem"] {
        last = m.inst(&format!("{op} double {a}, {b}"));
    }
    m.inst(&format!("fneg double {a}"));
    for cc in ["olt", "ole", "ogt", "oge", "oeq", "one"] {
        m.inst(&format!("fcmp {cc} double {a}, {b}"));
    }
    m.end(Some((&d, last.to_string())));
    m.finish()
}

/// Der Grundtest: Was der Emitter schreibt, ist gueltige LLVM-IR.
///
/// Ohne ihn pruefen die Textests eine Datei, die LLVM womoeglich gar nicht
/// annimmt — und das faende erst Schritt 8.
#[test]
fn what_the_emitter_writes_is_valid_llvm_ir() {
    let clang = clang_or_skip!();
    let dir = Temp::new("gueltig");
    if let Err(e) = clang.assembles(&every_float_instruction(), &dir.0) {
        panic!("die erzeugte IR assembliert nicht:\n{e}");
    }
}

/// Integer-Arithmetik und Vergleiche ebenso.
#[test]
fn integer_instructions_assemble() {
    let clang = clang_or_skip!();
    let mut m = Module::new("int", "x86_64-pc-windows-msvc");
    let i32_ = LlvmType::Int(32);
    let args = m.begin("ganz", &i32_, &[i32_.clone(), i32_.clone()]);
    let (a, b) = (args[0], args[1]);
    let mut last = a;
    for op in ["add", "sub", "mul", "sdiv", "udiv", "srem", "urem", "and", "or", "xor", "shl", "ashr", "lshr"] {
        last = m.inst(&format!("{op} i32 {a}, {b}"));
    }
    for cc in ["slt", "sle", "sgt", "sge", "eq", "ne", "ult", "ugt"] {
        m.inst(&format!("icmp {cc} i32 {a}, {b}"));
    }
    m.end(Some((&i32_, last.to_string())));
    if let Err(e) = clang.assembles(&m.finish(), &Temp::new("ganz").0) {
        panic!("Integer-IR assembliert nicht:\n{e}");
    }
}

/// 4.2, Satz 9.4.4: Das Bitmuster eines Literals ist die Zahl, die im
/// Programm steht — nachgerechnet von LLVM, nicht von uns.
///
/// Der Textest prueft, dass `float_literal` dasselbe Muster erzeugt wie
/// `to_bits`. Er kann nicht pruefen, ob LLVM es genauso liest. Dieser
/// Test laesst LLVM die Zahl ausrechnen und ausgeben.
#[test]
fn llvm_reads_our_float_literals_as_the_intended_numbers() {
    let clang = clang_or_skip!();
    let dir = Temp::new("literale");
    // 0.1 ist der Fall, an dem sich Dezimalschreibweise verraet.
    for value in [1.0_f64, 0.1, -0.0, 2.5e-308, 1.7976931348623157e308] {
        let lit = float_literal(value, &LlvmType::F64);
        let ir = format!(
            "target triple = \"x86_64-pc-windows-msvc\"\n\
             @.fmt = private constant [7 x i8] c\"%.17g\\00\\00\"\n\
             declare i32 @printf(ptr, ...)\n\
             define i32 @main() {{\n  \
               %1 = call i32 (ptr, ...) @printf(ptr @.fmt, double {lit})\n  \
               ret i32 0\n\
             }}\n"
        );
        let got = clang.run(&ir, &dir.0).unwrap_or_else(|e| panic!("{value}: {e}"));
        let back: f64 = got.trim().parse().unwrap_or_else(|e| panic!("{value}: `{got}` ({e})"));
        assert_eq!(back.to_bits(), value.to_bits(), "LLVM liest {lit} als {back}, nicht als {value}");
    }
}

/// 4.2: Die Kontraktion von `a * b + c` zu einem `fma` bleibt verboten —
/// „was man schreibt, bekommt man".
///
/// Das ist der Test, den die Textform allein nicht leisten kann: Ob LLVM
/// *trotz* fehlender Flags kontrahiert, sagt nur LLVM. Geprueft wird an
/// einem Fall, in dem sich beide Wege im letzten Bit unterscheiden.
#[test]
fn llvm_does_not_contract_a_multiply_and_add_into_an_fma() {
    let clang = clang_or_skip!();
    let dir = Temp::new("contract");
    // Klassischer Fall: Das Produkt ist nicht exakt darstellbar, also
    // rundet der getrennte Weg einmal mehr als `fma`.
    let (a, b, c) = (1.0 + 2f64.powi(-52), 1.0 - 2f64.powi(-52), -1.0);
    let ir = format!(
        "target triple = \"x86_64-pc-windows-msvc\"\n\
         @.fmt = private constant [9 x i8] c\"%llx\\0A\\00\\00\\00\\00\"\n\
         declare i32 @printf(ptr, ...)\n\
         define i32 @main() {{\n  \
           %1 = fmul double {}, {}\n  \
           %2 = fadd double %1, {}\n  \
           %3 = bitcast double %2 to i64\n  \
           %4 = call i32 (ptr, ...) @printf(ptr @.fmt, i64 %3)\n  \
           ret i32 0\n\
         }}\n",
        float_literal(a, &LlvmType::F64),
        float_literal(b, &LlvmType::F64),
        float_literal(c, &LlvmType::F64),
    );
    let got = clang.run(&ir, &dir.0).unwrap_or_else(|e| panic!("{e}"));
    let bits = u64::from_str_radix(got.trim(), 16).unwrap_or_else(|e| panic!("`{got}`: {e}"));

    let separate = (a * b) + c;
    let fused = a.mul_add(b, c);
    assert_ne!(separate.to_bits(), fused.to_bits(), "der Testfall unterscheidet die beiden Wege nicht");
    assert_eq!(
        bits,
        separate.to_bits(),
        "LLVM hat kontrahiert: erwartet {:016x} (getrennt), bekommen {bits:016x} (fma waere {:016x})",
        separate.to_bits(),
        fused.to_bits()
    );
}

/// 11.3: Reproduzierbare Builds.
///
/// Geprueft wird der *Inhalt* der Objektdatei, nicht ihr Kopf: Das
/// COFF-Format traegt an Byte 4 bis 7 einen Zeitstempel, den clang setzt
/// und nicht der Codegen. 11.3 verlangt „keine Zeitstempel oder Pfade im
/// Binary" vom Compiler; den Stempel des Assemblers schaltet erst der
/// Linkschritt ab (`/Brepro`), und er gehoert nicht zu dem, was hier zu
/// belegen ist.
///
/// Was hier zu belegen ist: Aus derselben IR entsteht derselbe Code.
#[test]
fn the_same_ir_produces_the_same_code() {
    let clang = clang_or_skip!();
    let ir = every_float_instruction();
    let mut objekte = Vec::new();
    for run in 0..2 {
        let dir = Temp::new(&format!("reproduzierbar{run}"));
        clang.assembles(&ir, &dir.0).unwrap_or_else(|e| panic!("{e}"));
        objekte.push(std::fs::read(dir.0.join("modul.o")).expect("Objektdatei lesbar"));
    }
    assert_eq!(objekte[0].len(), objekte[1].len(), "verschiedene Groesse");
    // Byte 4 bis 7: Zeitstempel des COFF-Kopfes.
    let ohne_stempel = |o: &[u8]| {
        let mut v = o.to_vec();
        v[4..8].fill(0);
        v
    };
    assert_eq!(ohne_stempel(&objekte[0]), ohne_stempel(&objekte[1]), "zwei Laeufe, zwei verschiedene Objektdateien");
}

/// Und die IR selbst traegt ueberhaupt keinen Zeitstempel — das ist die
/// Zusage, die 11.3 dem Compiler macht.
#[test]
fn the_generated_ir_is_byte_identical_across_runs() {
    assert_eq!(every_float_instruction(), every_float_instruction());
}
