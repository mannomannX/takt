//! Reine Funktionen im erzeugten Code gegen die Werte des Interpreters
//! (4.1, 3.9, 3.10, Satz 9.4.4).
//!
//! Der Interpreter ist die Spezifikation; seine Werte fuer dieselben
//! Ausdruecke halten `takt-interp/tests/values.rs` und
//! `takt-interp/tests/stimulus_values.rs` fest. Hier laeuft dasselbe als
//! Maschinencode: Das Programm geht durch Sema und `lower::program`, ein
//! C-Treiber ruft die Funktionen und schreibt Ergebnis und Fault-Flag.
//! Ein Absturz (SIGFPE bei `MIN % -1`) ist ein Befund, kein Testfehler.
//!
//! Die Funktionen sind im Modul `internal`; der Test macht sie fuer den
//! Treiber sichtbar, sonst aendert er an der IR nichts.

use takt_llvm::toolchain::{Clang, find};
use takt_mir::Program;
use takt_mir::machine::{ArithKind, FaultKind};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn program(body: &str) -> Program {
    program_with_head(HEAD, body)
}

fn program_with_head(head: &str, body: &str) -> Program {
    let src = format!("{head}{body}");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn host() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

/// Uebersetzt Programm und Treiber, laeuft und liefert die Ausgabe des
/// Treibers; `Err` traegt den Uebersetzungsfehler oder den Abbruch.
fn run(p: &Program, name: &str, driver: &str) -> Option<Result<String, String>> {
    let path = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let lowered = takt_llvm::lower::program(p, host(), &takt_llvm::symbols::Prefix::default());
    let ir = lowered.ir.replace("define internal ", "define ");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-native-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (ll, c, exe) =
        (dir.join("fns.ll"), dir.join("treiber.c"), dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" }));
    std::fs::write(&ll, &ir).expect("IR");
    let head = format!(
        "#include <stdio.h>\n#include <string.h>\nstatic _Alignas(8) unsigned char arena[{}];\n\
         static int flag(void) {{ int f; memcpy(&f, arena + {}, 4); memset(arena, 0, sizeof arena); return f; }}\n",
        takt_llvm::arena::fault::BYTES,
        takt_llvm::arena::fault::FLAG
    );
    std::fs::write(&c, format!("{head}{driver}")).expect("Treiber");
    let mut cmd = std::process::Command::new(&path);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    if !build.status.success() {
        return Some(Err(format!("uebersetzt nicht:\n{}", String::from_utf8_lossy(&build.stderr))));
    }
    let out = std::process::Command::new(&exe).output().expect("Lauf");
    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    if !out.status.success() {
        return Some(Err(format!("Abbruch ({}) nach:\n{text}", out.status)));
    }
    Some(Ok(text))
}

fn code(kind: FaultKind) -> u32 {
    takt_llvm::abi::fault_code(kind)
}

/// INT-005, GEN-009: `MIN % -1` ist null (4.1, Rust-Semantik), auch wenn
/// die Analyse den Ueberlaufknoten wegbeweist, weil der Rest in die
/// Breite passt; ein nacktes `srem` waere dort undefiniert und auf x86
/// ein Absturz.
#[test]
fn the_remainder_of_min_by_minus_one_is_zero() {
    let p = program(
        "fn rem_wide(x: int, y: int in -3..3) -> int:
    return x % y

fn rem_i32(x: i32, y: i32 in -5..5) -> i32:
    return x % y

fn rem_i8(x: i8, y: i8) -> i8:
    return x % y
",
    );
    let driver = "long long takt_fn_rem_wide(long long, long long, void *);
int takt_fn_rem_i32(int, int, void *);
signed char takt_fn_rem_i8(signed char, signed char, void *);
int main(void) {
    long long a = takt_fn_rem_wide(-9223372036854775807LL - 1, -1, arena);
    printf(\"%lld %d\\n\", a, flag());
    int b = takt_fn_rem_i32(-2147483647 - 1, -1, arena);
    printf(\"%d %d\\n\", b, flag());
    signed char c = takt_fn_rem_i8(-128, -1, arena);
    printf(\"%d %d\\n\", c, flag());
    printf(\"%lld %d\\n\", takt_fn_rem_wide(-7, 2, arena), flag());
    return 0;
}
";
    let Some(out) = run(&p, "rem", driver) else { return };
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), "0 0\n0 0\n0 0\n-1 0\n");
}

/// INT-005, INT-009: `-MIN` passt in keine Breite und faultet `Overflow`
/// wie im Interpreter, statt still auf sich selbst zu wickeln; ebenso die
/// kleinste Dauer.
#[test]
fn negating_the_minimum_faults() {
    let p = program(
        "fn neg(x: int) -> int:
    return -x

fn neg8(x: i8) -> i8:
    return -x

fn negd(d: Duration) -> Duration:
    return -d
",
    );
    let driver = "long long takt_fn_neg(long long, void *);
signed char takt_fn_neg8(signed char, void *);
long long takt_fn_negd(long long, void *);
int main(void) {
    takt_fn_neg(-9223372036854775807LL - 1, arena);
    printf(\"%d\\n\", flag());
    takt_fn_neg8(-128, arena);
    printf(\"%d\\n\", flag());
    takt_fn_negd(-9223372036854775807LL - 1, arena);
    printf(\"%d\\n\", flag());
    printf(\"%lld %d\\n\", takt_fn_neg(5, arena), flag());
    return 0;
}
";
    let Some(out) = run(&p, "neg", driver) else { return };
    let o = code(FaultKind::Arithmetic(ArithKind::Overflow));
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), format!("{o}\n{o}\n{o}\n-5 0\n"));
}

/// INT-006: `rotl`/`rotr` auf jeder Breite, der Betrag als `int` und
/// modulo der Breite (3.10), gegen die Rotation des Bitmusters.
#[test]
fn rotations_work_at_every_width() {
    let p = program(
        "fn l8(w: u8, n: int) -> u8:
    return rotl(w, n)

fn r16(w: u16, n: int) -> u16:
    return rotr(w, n)

fn l32(w: i32, n: int) -> i32:
    return rotl(w, n)

fn l64(w: u64, n: int) -> u64:
    return rotl(w, n)
",
    );
    let mut driver = String::from(
        "unsigned char takt_fn_l8(unsigned char, long long, void *);
unsigned short takt_fn_r16(unsigned short, long long, void *);
int takt_fn_l32(int, long long, void *);
unsigned long long takt_fn_l64(unsigned long long, long long, void *);
int main(void) {
",
    );
    let mut want = String::new();
    for n in [-1i64, 0, 1, 3, 7, 8, 9, 17, 63, 64, 65] {
        driver.push_str(&format!("    printf(\"%u \", (unsigned)takt_fn_l8(0x81, {n}LL, arena));\n"));
        driver.push_str(&format!("    printf(\"%u \", (unsigned)takt_fn_r16(0x8001, {n}LL, arena));\n"));
        driver.push_str(&format!("    printf(\"%d \", takt_fn_l32(-2147483647 - 1, {n}LL, arena));\n"));
        driver.push_str(&format!("    printf(\"%llu\\n\", takt_fn_l64(0x8000000000000001ULL, {n}LL, arena));\n"));
        let k = |w: i64| n.rem_euclid(w) as u32;
        want.push_str(&format!(
            "{} {} {} {}\n",
            0x81u8.rotate_left(k(8)),
            0x8001u16.rotate_right(k(16)),
            i32::MIN.rotate_left(k(32)),
            0x8000_0000_0000_0001u64.rotate_left(k(64))
        ));
    }
    driver.push_str("    return 0;\n}\n");
    let Some(out) = run(&p, "rot", &driver) else { return };
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), want);
}

/// INT-010: Am rechten Rand liefert `interp` genau den letzten
/// Stuetzwert (der Interpreter klemmt fuer `x >= x_last`), nicht die
/// Formel des letzten Segments, die dort um ein Bit danebenliegt.
#[test]
fn interp_returns_the_last_point_at_the_right_edge() {
    let p = program(
        "const CURVE : table<float, float> = [(0.0, 0.0), (3.0, 0.1)]

fn ip(x: float) -> float:
    return interp(CURVE, x)
",
    );
    let driver = "double takt_fn_ip(double, void *);
int main(void) {
    double v[4] = { takt_fn_ip(3.0, arena), takt_fn_ip(4.0, arena), takt_fn_ip(0.0, arena), takt_fn_ip(1.5, arena) };
    for (int i = 0; i < 4; i++) { unsigned long long b; memcpy(&b, &v[i], 8); printf(\"%llx\\n\", b); }
    return 0;
}
";
    let Some(out) = run(&p, "interp", driver) else { return };
    let want: String = [0.1f64, 0.1, 0.0, 0.0 + (0.1 - 0.0) * (1.5 - 0.0) / (3.0 - 0.0)]
        .iter()
        .map(|x| format!("{:x}\n", x.to_bits()))
        .collect();
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), want);
}

/// INT-010: Eine Range auf einer vorzeichenlosen Breite, deren Grenze
/// nicht in die vorzeichenbehaftete passt (`u8 in 0..200`), prueft der
/// Code wie der Interpreter: 250 faultet `Range`.
#[test]
fn an_unsigned_range_is_checked() {
    let p = program(
        "fn r8(x: u8) -> u8 in 0..200:
    return x

fn r16(x: u16) -> u16 in 10..40000:
    return x
",
    );
    let driver = "unsigned char takt_fn_r8(unsigned char, void *);
unsigned short takt_fn_r16(unsigned short, void *);
int main(void) {
    takt_fn_r8(250, arena); printf(\"%d\\n\", flag());
    printf(\"%u %d\\n\", (unsigned)takt_fn_r8(200, arena), flag());
    takt_fn_r16(50000, arena); printf(\"%d\\n\", flag());
    takt_fn_r16(9, arena); printf(\"%d\\n\", flag());
    printf(\"%u %d\\n\", (unsigned)takt_fn_r16(40000, arena), flag());
    return 0;
}
";
    let Some(out) = run(&p, "range", driver) else { return };
    let r = code(FaultKind::Range);
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), format!("{r}\n200 0\n{r}\n{r}\n40000 0\n"));
}

/// KOR-004: `case a..b` vergleicht nach der Vorzeichenart des Subjekts;
/// `u8` 200 liegt in `100..250` (der Interpreter vergleicht mathematisch),
/// als `i8` gelesen waere es -56. `match` steht nur in Maschinen, darum
/// die IR: Die Grenzen eines `u8`-Subjekts vergleicht `uge`/`ule`.
#[test]
fn a_case_range_on_an_unsigned_subject_compares_unsigned() {
    let p = program(
        "output o : int in 0..2 @ hw(\"o/o\") with safe = 0

machine m:
    var b : u8 = 200
    var s : i8 = -3
    initial RUN
    state RUN:
        loop:
            match b:
                case 100..250:
                    o = 1
                case _:
                    o = 0
            match s:
                case -5..5:
                    o = 2
                case _:
                    pass
",
    );
    let lowered = takt_llvm::lower::program(&p, host(), &takt_llvm::symbols::Prefix::default());
    assert!(lowered.complete(), "{:?}", lowered.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>());
    let compares: Vec<&str> =
        lowered.ir.lines().filter(|l| l.contains("icmp") && (l.contains(", 100") || l.contains(", 250"))).collect();
    assert!(
        compares.len() == 2 && compares.iter().all(|l| l.contains("icmp uge i8") || l.contains("icmp ule i8")),
        "{compares:?}"
    );
    let signed: Vec<&str> =
        lowered.ir.lines().filter(|l| l.contains("icmp") && (l.contains(", -5") || l.contains(", 5"))).collect();
    assert!(
        signed.iter().any(|l| l.contains("icmp sge i8")) && signed.iter().any(|l| l.contains("icmp sle i8")),
        "{signed:?}"
    );
}

/// GEN-009: In einer Maschine beweist die Analyse den Ueberlaufknoten um
/// `%` weg, sobald der Divisor beschraenkt ist; jedes `srem` muss seinen
/// Divisor trotzdem gegen -1 schuetzen.
#[test]
fn every_remainder_guards_its_divisor() {
    let p = program(
        "output o : int @ hw(\"o/o\") with safe = 0
output n : i32 @ hw(\"o/n\") with safe = 0

machine m:
    var x : int = -9223372036854775807 - 1
    var y : int in -3..3 = -1
    var a : i32 = -2147483647 - 1
    var b : i32 in -5..5 = -1
    initial RUN
    state RUN:
        loop:
            o = x % y
            n = a % b
",
    );
    let lowered = takt_llvm::lower::program(&p, host(), &takt_llvm::symbols::Prefix::default());
    assert!(lowered.complete(), "{:?}", lowered.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>());
    let lines: Vec<&str> = lowered.ir.lines().collect();
    let rems: Vec<usize> = (0..lines.len()).filter(|i| lines[*i].contains(" = srem ")).collect();
    assert!(rems.len() >= 2, "{} srem", rems.len());
    for i in rems {
        let divisor = lines[i].rsplit(", ").next().unwrap_or("").trim();
        // Register gelten je Funktion: die letzte Definition davor.
        let defined = lines[..i].iter().rev().find(|l| l.trim_start().starts_with(&format!("{divisor} = "))).copied();
        let defined = defined.unwrap_or("");
        assert!(defined.contains("select i1") && defined.contains(" 1, "), "`{}`: Divisor `{defined}`", lines[i]);
    }
}

/// Eine Variable, die ein Parameter der Testfunktion ist.
struct Param(takt_llvm::expr::Lowered);

impl takt_llvm::expr::Vars for Param {
    fn var(&self, _: takt_mir::VarId, _: &mut takt_llvm::emit::Module) -> Option<takt_llvm::expr::Lowered> {
        Some(self.0.clone())
    }
}

/// INT-011: Ein Wert im Formatstring steht wie im Interpreter
/// (`takt_interp::format::display`): vorzeichenlose Breiten als
/// vorzeichenlose Zahl, eine Dauer in ihrer groessten ganzzahligen
/// Einheit, ein Enum ohne Felder als Name seiner Variante.
///
/// Formatstrings stehen in `send` und Meldungen von Maschinen; die
/// Funktion je Fall baut der Test darum selbst um `format::render`.
#[test]
fn a_formatted_value_reads_like_the_interpreter() {
    use takt_llvm::ty::LlvmType;
    use takt_mir::expr::{Expr, ExprKind};
    use takt_mir::pattern::{Format, FormatPiece};
    use takt_mir::types::{IntWidth, Type};

    let Some(path) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let mut p = program("enum Mode: IDLE, RUN, STOP\n");
    let mut int = |w| p.types.intern(Type::Int { width: w, unit: None, range: None });
    let (u8_, i8_, u64_) = (int(IntWidth::U8), int(IntWidth::I8), int(IntWidth::U64));
    let dur = p.types.intern(Type::Duration { range: None });
    let mode_id = p.enums.iter().position(|e| e.name == "Mode").expect("Mode");
    let mode = p.types.intern(Type::Enum(takt_mir::EnumId(mode_id as u32)));
    // (Typ, LLVM-Typ, C-Typ, Argument, Angabe, erwarteter Text)
    type Case<'a> = (takt_mir::TypeId, LlvmType, &'a str, &'a str, Option<&'a str>, &'a str);
    let cases: [Case<'_>; 14] = [
        (u8_, LlvmType::Int(8), "unsigned char", "200", None, "200"),
        (u8_, LlvmType::Int(8), "unsigned char", "200", Some("hex"), "c8"),
        (u8_, LlvmType::Int(8), "unsigned char", "200", Some("04"), "0200"),
        (i8_, LlvmType::Int(8), "signed char", "-2", None, "-2"),
        (i8_, LlvmType::Int(8), "signed char", "-2", Some("hex"), "fffffffffffffffe"),
        (i8_, LlvmType::Int(8), "signed char", "-2", Some("04"), "-002"),
        (u64_, LlvmType::Int(64), "unsigned long long", "18446744073709551615ULL", None, "18446744073709551615"),
        (
            u64_,
            LlvmType::Int(64),
            "unsigned long long",
            "18446744073709551615ULL",
            Some("022"),
            "0018446744073709551615",
        ),
        (dur, LlvmType::Int(64), "long long", "1500000LL", None, "1500 us"),
        (dur, LlvmType::Int(64), "long long", "-2000000000LL", None, "-2 s"),
        (dur, LlvmType::Int(64), "long long", "0LL", None, "0 ns"),
        (dur, LlvmType::Int(64), "long long", "7LL", None, "7 ns"),
        (mode, LlvmType::Int(32), "int", "0", None, "IDLE"),
        (mode, LlvmType::Int(32), "int", "2", None, "STOP"),
    ];
    let text_ty = LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), 32)]);
    let mut m = takt_llvm::emit::Module::new("format", host());
    let mut driver = String::from("struct s { int len; char bytes[32]; };\n");
    let mut calls = String::new();
    for (i, (ty, llvm, c_ty, arg, spec, _)) in cases.iter().enumerate() {
        let regs = m.begin_with("", &format!("fmt{i}"), &LlvmType::Void, &[LlvmType::Ptr, llvm.clone()], &[], "");
        let vars = Param(takt_llvm::expr::Lowered { value: regs[1].to_string(), ty: llvm.clone() });
        let expr = Expr::new(ExprKind::Var(takt_mir::VarId(0)), *ty, takt_diag::Span::default());
        let f = Format { pieces: vec![FormatPiece::Expr { expr, spec: spec.map(str::to_string) }], len_max: 32 };
        takt_llvm::format::render(&f, regs[0], &text_ty, 32, &p, &mut m, &vars).expect("gesenkt");
        m.end(None);
        driver.push_str(&format!("void fmt{i}(struct s *, {c_ty}, void *);\n"));
        calls.push_str(&format!(
            "    memset(&t, 0, sizeof t); fmt{i}(&t, {arg}, arena); printf(\"%.*s\\n\", t.len, t.bytes);\n"
        ));
    }
    let want: String = cases.iter().map(|c| format!("{}\n", c.5)).collect();
    let ir = m.finish();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-llvm-native-format");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (ll, c, exe) =
        (dir.join("fmt.ll"), dir.join("treiber.c"), dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" }));
    std::fs::write(&ll, &ir).expect("IR");
    let source = format!(
        "#include <stdio.h>\n#include <string.h>\nstatic _Alignas(8) unsigned char arena[{}];\n{driver}\
         int main(void) {{\n    struct s t;\n{calls}    return 0;\n}}\n",
        takt_llvm::arena::fault::BYTES
    );
    std::fs::write(&c, source).expect("Treiber");
    let mut cmd = std::process::Command::new(&path);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}\n--- IR ---\n{ir}", String::from_utf8_lossy(&build.stderr));
    let out = std::process::Command::new(&exe).output().expect("Lauf");
    assert_eq!(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"), want);
}

/// GEN-006: Ein Initialisierer einer Maschinenvariablen, der eine reine
/// Funktion ruft, hat einen Fault-Pfad (ihr Flag); die Marke dafuer muss
/// in der Initialisierung entstehen, sonst assembliert die IR nicht.
#[test]
fn an_initializer_that_calls_a_function_assembles() {
    let Some(path) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let p = program(
        "output o : float @ hw(\"o/o\") with safe = 0.0

fn half(x: float) -> float:
    return x / 2.0

machine m:
    var g : float = half(3.0)
    initial RUN
    state RUN:
        loop:
            o = g
",
    );
    let lowered = takt_llvm::lower::program(&p, host(), &takt_llvm::symbols::Prefix::default());
    assert!(lowered.complete(), "{:?}", lowered.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>());
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-llvm-native-init");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    if let Err(e) = Clang::At(path).assembles(&lowered.ir, &dir) {
        panic!("die IR assembliert nicht:\n{e}");
    }
}

/// INT-030: Die Matrizen des erzeugten Codes tragen dieselben Bits wie
/// `libtaktm::mat` im Interpreter (`takt-interp/tests/values.rs`,
/// `matrices_are_bit_exact`): Inverse einer 3×3 und einer 4×4,
/// Determinante mit Pivottausch, und dieselbe 3×3 in `f32`.
#[test]
fn matrices_carry_the_bits_of_the_interpreter() {
    let fns = "fn inv3(a: mat<3, 3>) -> mat<3, 3>:
    return a.inv()

fn inv4(a: mat<4, 4>) -> mat<4, 4>:
    return a.inv()

fn det2(a: mat<2, 2>) -> float:
    return a.det()
";
    let driver = |ty: &str, fmt: &str, bits: &str| {
        format!(
            "void takt_fn_inv3({ty} *, const {ty} *, void *);
void takt_fn_inv4({ty} *, const {ty} *, void *);
{ty} takt_fn_det2(const {ty} *, void *);
static void show(const {ty} *v, int n) {{ for (int i = 0; i < n; i++) {{ {bits} b; memcpy(&b, &v[i], sizeof b); printf(\"{fmt} \", b); }} printf(\"\\n\"); }}
int main(void) {{
    {ty} a3[9] = {{4, 7, 2, 3, 6, 1, 2, 5, 3}}, a4[16] = {{2, 1, 0, 0, 1, 3, 1, 0, 0, 1, 4, 1, 0, 0, 1, 5}}, p[4] = {{0, 1, 1, 0}};
    {ty} r3[9], r4[16], d;
    takt_fn_inv3(r3, a3, arena); show(r3, 9);
    takt_fn_inv4(r4, a4, arena); show(r4, 16);
    d = takt_fn_det2(p, arena); show(&d, 1);
    printf(\"%d\\n\", flag());
    return 0;
}}
"
        )
    };
    let hex = |v: &[u64]| v.iter().map(|b| format!("{b:x} ")).collect::<String>();
    let want64 = format!("{}\n{}\n{}\n0\n", hex(&INV_A3), hex(&INV_A4), hex(&[(-1f64).to_bits()]));
    let p = program(fns);
    let Some(out) = run(&p, "mat64", &driver("double", "%llx", "unsigned long long")) else { return };
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), want64);
    let p32 = program_with_head("system:\n    language = 1\n    tick = 10 ms\n    float = f32\n\n", fns);
    let Some(out) = run(&p32, "mat32", &driver("float", "%x", "unsigned int")) else { return };
    let first = out.unwrap_or_else(|e| panic!("{e}"));
    let line = first.lines().next().unwrap_or("");
    assert_eq!(line, INV_A3_F32.iter().map(|b| format!("{b:x} ")).collect::<String>(), "f32");
}

const INV_A3: [u64; 9] = [
    0x3ff7_1c71_c71c_71c6,
    0xbff3_8e38_e38e_38e3,
    0xbfe1_c71c_71c7_1c72,
    0xbfe8_e38e_38e3_8e38,
    0x3fec_71c7_1c71_c71c,
    0x3fcc_71c7_1c71_c71d,
    0x3fd5_5555_5555_5555,
    0xbfe5_5555_5555_5555,
    0x3fd5_5555_5555_5555,
];
const INV_A4: [u64; 16] = [
    0x3fe3_9393_9393_9394,
    0xbfcc_9c9c_9c9c_9c9d,
    0x3fae_1e1e_1e1e_1e1e,
    0xbf88_1818_1818_1818,
    0xbfcc_9c9c_9c9c_9c9d,
    0x3fdc_9c9c_9c9c_9c9d,
    0xbfbe_1e1e_1e1e_1e1e,
    0x3f98_1818_1818_1818,
    0x3fae_1e1e_1e1e_1e1f,
    0xbfbe_1e1e_1e1e_1e1f,
    0x3fd2_d2d2_d2d2_d2d3,
    0xbfae_1e1e_1e1e_1e1e,
    0xbf88_1818_1818_1818,
    0x3f98_1818_1818_1818,
    0xbfae_1e1e_1e1e_1e1e,
    0x3fcb_1b1b_1b1b_1b1b,
];
const INV_A3_F32: [u32; 9] = [
    0x3fb8_e390,
    0xbf9c_71c7,
    0xbf0e_38e3,
    0xbf47_1c73,
    0x3f63_8e39,
    0x3e63_8e38,
    0x3eaa_aaab,
    0xbf2a_aaab,
    0x3eaa_aaab,
];

/// Setzt um den Rueckgabewert jeder Funktion `Checked{NonFinite}`, wie das
/// Sema es um ein Ergebnis aus Gleitkommawerten setzt (4.1).
fn finite_returns(p: &mut Program) {
    use takt_mir::expr::{CheckedKind, Expr, ExprKind};
    for f in &mut p.fns {
        for s in &mut f.body.stmts {
            if let takt_mir::stmt::StmtKind::Return(e) = &mut s.kind {
                let (ty, span) = (e.ty, e.span);
                let inner = std::mem::replace(e, Expr::new(ExprKind::Bool(false), ty, span));
                *e = Expr::new(ExprKind::Checked { expr: Box::new(inner), kind: CheckedKind::NonFinite }, ty, span);
            }
        }
    }
}

/// INT-023: `Checked{NonFinite}` um ein `[N] float` prueft jedes Element,
/// wie der Interpreter (`takt-interp/tests/values.rs`,
/// `a_non_finite_element_of_an_array_faults`): ein Inf an letzter Stelle
/// faultet, ein endliches Array kommt unveraendert zurueck.
#[test]
fn every_element_of_a_float_array_is_checked() {
    let mut p = program("fn same(x: [4] float) -> [4] float:\n    return x\n");
    finite_returns(&mut p);
    let driver = "void takt_fn_same(double *, const double *, void *);
int main(void) {
    double fine[4] = {1, 2, 3, 4}, bad[4] = {1, 2, 3, 1e308 * 10.0}, r[4];
    takt_fn_same(r, fine, arena);
    printf(\"%g %g %g %g %d\\n\", r[0], r[1], r[2], r[3], flag());
    takt_fn_same(r, bad, arena);
    printf(\"%d\\n\", flag());
    return 0;
}
";
    let Some(out) = run(&p, "finite_array", driver) else { return };
    let want = format!("1 2 3 4 0\n{}\n", code(FaultKind::Arithmetic(ArithKind::NonFinite)));
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), want);
}

/// INT-023: Das Ergebnis eines Natives als `[256] float` geht durch
/// `Checked{NonFinite}`, das Sema setzt den Knoten; der Codegen senkt ihn
/// ohne `NotYet` und prueft jedes Element.
#[test]
fn a_native_array_result_can_be_checked_for_finiteness() {
    use takt_mir::expr::{CheckedKind, ExprKind};
    let p = program(
        "native fn fft256(x: [256] float) -> [256] float with cost = 8400, stack = 9000, total

machine m:
    var x : [256] float = default
    var y : [256] float = default
    initial RUN
    state RUN:
        loop:
            y = fft256(x)
",
    );
    let checked = p.machines[0]
        .states
        .iter()
        .flat_map(|s| &s.loop_block.stmts)
        .filter(|s| {
            matches!(&s.kind, takt_mir::stmt::StmtKind::Assign { value, .. }
                if matches!(&value.kind, ExprKind::Checked { expr, kind: CheckedKind::NonFinite }
                    if matches!(expr.kind, ExprKind::NativeCall { .. })))
        })
        .count();
    assert_eq!(checked, 1, "der Aufruf steht in `Checked{{NonFinite}}`");
    let lowered = takt_llvm::lower::program(&p, host(), &takt_llvm::symbols::Prefix::default());
    assert!(lowered.skipped.is_empty(), "{:?}", lowered.skipped);
    let checks = lowered.ir.lines().filter(|l| l.contains("shl i64") && l.contains(", 1")).count();
    assert!(checks >= 256, "jedes der 256 Elemente geprueft, gefunden {checks}");
}

/// INT-025 (3.7): `decode` eines `layout`-Records liefert `none`, wenn ein
/// Gleitkommafeld NaN oder Inf traegt, wie der Interpreter
/// (`takt-prove/tests/interpreter_runs.rs`, `a_non_finite_float_field_is_no_value`);
/// endliche Felder, auch -0.0 und Subnormale, decodieren.
#[test]
fn decode_refuses_a_non_finite_float_field() {
    let p = program(
        "record Sample layout little:
    a : f32
    b : f64

fn ok(raw: bytes<16>) -> bool:
    return Sample.decode(raw).valid
",
    );
    let driver = "struct b16 { int len; unsigned char b[16]; };
_Bool takt_fn_ok(const struct b16 *, void *);
static void put(struct b16 *r, unsigned a, unsigned long long b) {
    r->len = 12; memset(r->b, 0, 16); memcpy(r->b, &a, 4); memcpy(r->b + 4, &b, 8);
}
int main(void) {
    struct b16 r;
    unsigned fa[] = {0x3f800000u, 0x80000000u, 0x00000001u, 0x7fc00000u, 0x7f800000u, 0xff800000u, 0x3f800000u, 0x3f800000u, 0x3f800000u};
    unsigned long long fb[] = {0x4000000000000000ull, 0x8000000000000000ull, 1ull, 0x4000000000000000ull, 0x4000000000000000ull, 0x4000000000000000ull, 0x7ff8000000000000ull, 0x7ff0000000000000ull, 0xfff0000000000000ull};
    for (int i = 0; i < 9; i++) { put(&r, fa[i], fb[i]); printf(\"%d\", takt_fn_ok(&r, arena)); }
    printf(\" %d\\n\", flag());
    return 0;
}
";
    let Some(out) = run(&p, "decode_finite", driver) else { return };
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), "111000000 0\n");
}

/// KON2-028 (3.9): `starts_with` und `contains` auf Text, bytewise wie im
/// Interpreter (`str::starts_with`, `str::contains`): leer trifft immer,
/// ein Text trifft sich selbst, ein laengerer nie, Mehrbyte-Zeichen als
/// Bytes.
#[test]
fn text_search_agrees_with_the_interpreter() {
    let p = program(
        "fn sw(s: str<32>, t: str<16>) -> bool:
    return s.starts_with(t)

fn inside(s: str<32>, t: str<16>) -> bool:
    return s.contains(t)

fn lit(s: str<32>) -> bool:
    return s.contains(\"OK\") and not s.starts_with(\"ERR\")
",
    );
    let cases = [
        ("", ""),
        ("abc", ""),
        ("abc", "a"),
        ("abc", "abc"),
        ("abc", "abcd"),
        ("xabc", "abc"),
        ("xab", "abc"),
        ("aab", "ab"),
        ("\u{e4}b", "\u{e4}"),
        ("b\u{e4}", "\u{e4}"),
        ("ERR OK", "OK"),
        ("all OK", "K"),
    ];
    let mut driver = String::from(
        "struct s32 { int len; unsigned char b[32]; };
struct s16 { int len; unsigned char b[16]; };
_Bool takt_fn_sw(const struct s32 *, const struct s16 *, void *);
_Bool takt_fn_inside(const struct s32 *, const struct s16 *, void *);
_Bool takt_fn_lit(const struct s32 *, void *);
int main(void) {
    struct s32 s; struct s16 t;
",
    );
    let mut want = String::new();
    for (s, t) in cases {
        let bytes = |x: &str| x.bytes().map(|b| format!("\\x{b:02x}")).collect::<String>();
        driver.push_str(&format!(
            "    memset(&s, 0, sizeof s); memset(&t, 0, sizeof t); s.len = {}; t.len = {};\n",
            s.len(),
            t.len()
        ));
        if !s.is_empty() {
            driver.push_str(&format!("    memcpy(s.b, \"{}\", {});\n", bytes(s), s.len()));
        }
        if !t.is_empty() {
            driver.push_str(&format!("    memcpy(t.b, \"{}\", {});\n", bytes(t), t.len()));
        }
        driver.push_str("    printf(\"%d%d%d\\n\", takt_fn_sw(&s, &t, arena), takt_fn_inside(&s, &t, arena), takt_fn_lit(&s, arena));\n");
        want.push_str(&format!(
            "{}{}{}\n",
            u8::from(s.starts_with(t)),
            u8::from(s.contains(t)),
            u8::from(s.contains("OK") && !s.starts_with("ERR"))
        ));
    }
    driver.push_str("    printf(\"%d\\n\", flag());\n    return 0;\n}\n");
    want.push_str("0\n");
    let Some(out) = run(&p, "text_search", &driver) else { return };
    assert_eq!(out.unwrap_or_else(|e| panic!("{e}")), want);
}
