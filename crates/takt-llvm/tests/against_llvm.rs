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

/// Sucht `clang`; fehlt er, scheitert der Test, es sei denn,
/// `TAKT_ALLOW_MISSING` erlaubt das Fehlen (FB-392).
macro_rules! clang_or_skip {
    () => {{
        let Some(path) =
            takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
        else {
            return;
        };
        Clang::At(path)
    }};
}

/// Ein Verzeichnis, das sich nach dem Test wieder aufraeumt.
struct Temp(std::path::PathBuf);

impl Temp {
    fn new(name: &str) -> Temp {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-{name}"));
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
    for value in [1.0_f64, 0.1, -0.0, 2.5e-308, 1.7976931348623157e308, f64::from_bits(1), f64::MIN_POSITIVE] {
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

/// GEN-002: Dasselbe fuer `float`: LLVM nimmt das Muster des `double`,
/// das den `f32` genau traegt. Gelesen werden die Bits des `float` selbst,
/// ohne Umweg ueber `printf`, das `float` zu `double` erweitert.
#[test]
fn llvm_reads_our_f32_literals_as_the_intended_numbers() {
    let clang = clang_or_skip!();
    let dir = Temp::new("literale32");
    for value in [0.1_f32, -0.0, f32::MIN_POSITIVE, f32::from_bits(1), f32::MAX, 3.0, 1e-36] {
        let lit = float_literal(f64::from(value), &LlvmType::F32);
        let ir = format!(
            "target triple = \"x86_64-pc-windows-msvc\"\n\
             @.fmt = private constant [4 x i8] c\"%x\\00\\00\"\n\
             declare i32 @printf(ptr, ...)\n\
             define i32 @main() {{\n  \
               %1 = bitcast float {lit} to i32\n  \
               %2 = call i32 (ptr, ...) @printf(ptr @.fmt, i32 %1)\n  \
               ret i32 0\n\
             }}\n"
        );
        let got = clang.run(&ir, &dir.0).unwrap_or_else(|e| panic!("{value}: {e}"));
        let bits = u32::from_str_radix(got.trim(), 16).unwrap_or_else(|e| panic!("{value}: `{got}` ({e})"));
        assert_eq!(bits, value.to_bits(), "LLVM liest {lit} als {:e}, nicht als {value:e}", f32::from_bits(bits));
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

/// 4.2: Die Pruefung auf Endlichkeit bleibt auf einem Kern ohne FPU ein
/// Vergleich der Bits (FB-299).
///
/// LLVM erkennt Endlichkeitstests und schreibt sie als `fcmp` um; fuer
/// einen Wert, den es als nichtnegativ kennt, fiel dabei der Betrag weg,
/// und ohne Doppel-FPU wurden aus der Pruefung zwei Bibliotheksaufrufe.
/// Geprueft wird der Fall, der das ausloeste — eine vorzeichenlose Zahl als
/// `f64` mal eine Konstante —, dazu `f32` und ein Wert mit unbekanntem
/// Vorzeichen. Eine neue LLVM-Version muss ihn bestehen.
#[test]
fn a_finiteness_check_stays_a_bit_test_without_an_fpu() {
    let Clang::At(clang) = clang_or_skip!() else { return };
    let dir = Temp::new("finite");
    let src = "\
system:
    language = 1
    tick     = 10 ms

output digest : float @ hw(\"digest\") with safe = 0.0

machine m:
    var seed : u32 = 7
    var x : float = 1.5
    var y : f32 = 2.5
    initial RUN
    state RUN:
        loop:
            seed = ((seed as int) * 1664525 + 1013904223).wrap_u32()
            x = 300.0 + ((seed >> 20) as float) * 0.025
            y = ((seed >> 24) as f32) * 0.5
            x = x * (x - 301.0)
            digest = x + y as float
";
    let o = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let p = takt_sema::compile(src, &o).program.expect("uebersetzt");
    // GEN-003: beide Kerne ohne Doppel-FPU; der Thumb-Kern rechnet `f32` in
    // Hardware und `f64` ueber `__aeabi_*`.
    let cases: [(takt_llvm::Target, &str, &[&str]); 2] = [
        (
            takt_llvm::Target::RISCV32IMAC,
            "__muldf3",
            &["__unorddf2", "__eqdf2", "__nedf2", "__unordsf2", "__eqsf2", "__nesf2"],
        ),
        (
            takt_llvm::Target::THUMBV7EM,
            "__aeabi_dmul",
            &["__aeabi_dcmpun", "__aeabi_dcmpeq", "__aeabi_fcmpun", "__aeabi_fcmpeq", "__unorddf2", "__eqdf2"],
        ),
    ];
    for (target, software, forbidden) in cases {
        let instrument = takt_llvm::Instrument::default_for(p.config.runtime_profile(), target);
        let ir =
            takt_llvm::lower::program_with(&p, target.triple, &takt_llvm::symbols::Prefix::default(), instrument).ir;
        let (ll, asm) =
            (dir.0.join(format!("finite_{}.ll", target.name)), dir.0.join(format!("finite_{}.s", target.name)));
        std::fs::write(&ll, &ir).expect("IR schreibbar");
        let mut cmd = std::process::Command::new(&clang);
        cmd.args(["-S", "-Wno-override-module", "-ffreestanding", "-nostdlib"])
            .arg(format!("--target={}", target.triple));
        if !target.march.is_empty() {
            cmd.arg(format!("-march={}", target.march));
        }
        let ok = cmd
            .args(takt_llvm::toolchain::object_flags(target.triple))
            .arg(&ll)
            .arg("-o")
            .arg(&asm)
            .status()
            .is_ok_and(|s| s.success());
        assert!(ok, "{}: clang schlug fehl", target.name);
        let text = std::fs::read_to_string(&asm).expect("Assembler lesbar");
        assert!(text.contains(software), "{}: die Arithmetik laeuft nicht in Software:\n{text}", target.name);
        for compare in forbidden {
            assert!(!text.contains(compare), "{}: eine Pruefung ruft `{compare}`:\n{text}", target.name);
        }
    }
}

/// 11.3: Reproduzierbare Builds — aus derselben IR entsteht dasselbe
/// Objekt, Byte fuer Byte. Das COFF-Format traegt an Byte 4 bis 7 einen
/// Zeitstempel; `Clang::deterministic` schaltet ihn ab (FB-165), und
/// genau das wird hier verlangt statt maskiert.
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
    if cfg!(windows) {
        assert_eq!(objekte[0][4..8], [0, 0, 0, 0], "der COFF-Kopf traegt einen Zeitstempel");
    }
    assert!(objekte[0] == objekte[1], "zwei Laeufe, zwei verschiedene Objektdateien");
}

/// Und die IR selbst traegt ueberhaupt keinen Zeitstempel — das ist die
/// Zusage, die 11.3 dem Compiler macht: zweimal dieselbe MIR durch
/// `lower::program`, zweimal derselbe Text.
#[test]
fn the_generated_ir_is_byte_identical_across_runs() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/23_patterns.takt"))
        .expect("lesbar");
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let ir = || {
        let p = takt_sema::compile(&src, &options).program.expect("Programm");
        takt_llvm::lower::program(&p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default()).ir
    };
    assert_eq!(ir(), ir());
}
