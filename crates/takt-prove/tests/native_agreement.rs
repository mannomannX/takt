//! Interpreter gegen erzeugten Code an kleinen Programmen (Satz 9.4.4): der
//! ganze Lauf im Wirtsrahmen der Abnahme (`takt_conformance::harness`),
//! verglichen Tick fuer Tick und bitgenau (`takt_conformance::run::compare`).

use takt_interp::{RunOptions, Trace, run};
use takt_llvm::toolchain::{Clang, find};
use takt_mir::Program;

fn compile(src: &str) -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Der Trace des Interpreters und der des erzeugten Codes ueber `ticks`
/// Ticks; `None` ohne clang (`takt_testkit::require` laesst das nur mit
/// `TAKT_ALLOW_MISSING` durch).
fn both(name: &str, p: &Program, ticks: u64) -> Option<(String, String)> {
    let clang = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let interpreted =
        run(p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render();
    let host = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let low = takt_llvm::lower::program(p, host, &takt_llvm::symbols::Prefix::default());
    assert!(low.skipped.is_empty(), "{name}: {:?}", low.skipped);
    let frame = takt_conformance::harness::build_all(p, ticks, &[]);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-prove-native-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (ll, c, exe) =
        (dir.join("programm.ll"), dir.join("rahmen.c"), dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" }));
    std::fs::write(&ll, &low.ir).expect("IR");
    std::fs::write(&c, &frame.source).expect("Rahmen");
    let natives = takt_conformance::harness::native_library().expect("Bibliothek der Natives");
    let mut cmd = std::process::Command::new(&clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg(&natives)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{name}: {}", String::from_utf8_lossy(&build.stderr));
    let out = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(out.status.success(), "{name}: der Lauf brach ab ({})", out.status);
    let native = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    assert!(native.contains(&format!("takt end {}", ticks + 1)), "{name}: der Lauf endete vorher:\n{native}");
    Some((interpreted, native))
}

/// Vergleicht beide Laeufe und liefert den Trace des Interpreters.
fn agree(name: &str, p: &Program, ticks: u64) -> Option<String> {
    let (interpreted, native) = both(name, p, ticks)?;
    let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(p));
    let diffs = takt_conformance::run::compare(&widened, &native);
    let list: Vec<String> = diffs.iter().map(|d| format!("  {d}")).collect();
    assert!(
        diffs.is_empty(),
        "{name}:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        list.join("\n")
    );
    Some(interpreted)
}

/// `repeat P` mit `P` aus einem `param`, dessen Range null erlaubt.
fn repeat_program(p: i64) -> String {
    format!(
        "system:
    language = 1
    tick     = 10 ms

param P : int in 0..5 = {p}

output n    : int in 0..9 @ hw(\"o/n\")    with safe = 0
output done : bool        @ hw(\"o/done\") with safe = false

machine m:
    var k : int in 0..9 = 0
    initial RUN
    state RUN:
        sequence:
            repeat P:
                k = min(k + 1, 9)
                n = k
                wait 10 ms
            done = true
            -> END
    state END:
        when false: -> RUN
"
    )
}

/// SYN-032 (6.2, entschieden): `repeat P` laeuft P-mal, bei null gar nicht —
/// im erzeugten Code Tick fuer Tick wie im Interpreter.
#[test]
fn a_repeat_of_zero_one_or_three_agrees_with_the_interpreter() {
    for (p, passes) in [(0, 0), (1, 1), (3, 3)] {
        let Some(trace) = agree(&format!("repeat{p}"), &compile(&repeat_program(p)), 20) else { return };
        let last = trace.lines().rev().find_map(|l| l.split_once(" out n ").and_then(|(_, v)| v.parse().ok()));
        assert_eq!(last.unwrap_or(0), passes, "P = {p}:\n{trace}");
        assert!(trace.contains("out done true"), "P = {p}:\n{trace}");
    }
}

/// Einheitenumrechnungen ueber `param`s, damit nichts zur Uebersetzungszeit
/// faltet; `{float}` ist `f64` oder `f32` (`system: float`).
fn conversions(float: &str, big: &str) -> String {
    format!(
        "system:
    language = 1
    tick     = 10 ms
    float    = {float}

param A : float[bar]  = 1.0 bar
param B : float[psi]  = {big} psi
param C : float[degC] = 20.0 degC

output o1 : float[psi] @ hw(\"o/1\") with safe = 0.0 psi
output o2 : float[bar] @ hw(\"o/2\") with safe = 0.0 bar
output o3 : float[K]   @ hw(\"o/3\") with safe = 0.0 K

machine m:
    initial RUN
    state RUN:
        loop:
            o1 = A.to(psi)
            o2 = B.to(bar)
            o3 = C.to(K)
"
    )
}

/// INT-008 (3.2): `x.to(U)` ist x mal dem exakten Faktor, korrekt gerundet
/// — im erzeugten Code bitgleich wie im Interpreter, in beiden Breiten. Ein
/// Zwischenwert laeuft nicht ueber: `1e300 psi` (f64) und `1e30 psi` (f32)
/// sind in bar endlich. Die erwarteten Werte stammen aus dem Bruch
/// (`tools/libtaktm.py`, `round_exact`).
#[test]
fn a_unit_conversion_agrees_bit_for_bit() {
    for (float, big, psi, bar) in
        [("f64", "1e300", "14.503773773021681", "6.894757293168e298"), ("f32", "1e30", "14.503774", "6.8947575e28")]
    {
        let Some(trace) = agree(&format!("to_{float}"), &compile(&conversions(float, big)), 2) else { return };
        assert!(trace.contains(&format!("out o1 {psi} psi")), "{float}:\n{trace}");
        assert!(trace.contains(&format!("out o2 {bar} bar")), "{float}:\n{trace}");
        assert!(trace.contains("out o3 293.15 K"), "{float}:\n{trace}");
    }
}

/// INT-008: Ein Ergebnis ausserhalb des Bereichs ist ein `NonFinite`-Fault,
/// in beiden Implementierungen in demselben Tick.
#[test]
fn a_unit_conversion_that_overflows_faults_in_both() {
    let src = "system:
    language = 1
    tick     = 10 ms

param D : float[bar] = 1e308 bar

output o : float[psi] @ hw(\"o/o\") with safe = 0.0 psi

machine m:
    initial RUN
    state RUN:
        loop:
            o = D.to(psi)
";
    let Some(trace) = agree("to_overflow", &compile(src), 2) else { return };
    assert!(trace.contains("fault m") && trace.contains("NonFinite"), "{trace}");
}
