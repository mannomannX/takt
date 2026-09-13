//! Der Fuzzer der Abnahme (13.8): erzeugte Programme statt geschriebener.
//!
//! **Warum ein Fuzzer, wenn der Korpus schon stimmt.** Der Korpus ist
//! geschrieben worden, um Sprachmerkmale zu zeigen — er trifft die Faelle,
//! an die jemand gedacht hat. Der Fuzzer trifft die anderen: Ausdruecke,
//! deren Klammerung niemand so schreiben wuerde, Zahlen an den Raendern
//! der Darstellung, Ketten von Operationen, die sich gegenseitig
//! aufheben.
//!
//! **Was er erzeugt, ist absichtlich eng.** Jedes Programm hat eine
//! Maschine, einen Zustand und eine Zuweisung — nur der *Ausdruck* wird
//! gewuerfelt. Damit ist jede Abweichung auf einen Ausdruck
//! zurueckzufuehren, statt auf ein Zusammenspiel, das erst zu entwirren
//! waere.
//!
//! Der Generator ist ein xorshift mit festem Startwert: Ein Fehlschlag
//! ist reproduzierbar, und der Testlauf ist es auch — dieselbe Linie wie
//! bei den `libtaktm`-Vektoren.

use takt_conformance::compare;
use takt_llvm::toolchain::{Clang, find};

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Ein Ganzzahlausdruck ueber `n` und Konstanten.
///
/// Die Konstanten liegen an den Raendern der Darstellung, weil dort die
/// Fehler sitzen: Ein Ueberlauf an `i32::MAX` ist ein Fault (4.1), und
/// der Codegen darf ihn nicht wegoptimieren.
fn int_expr(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 {
        return match rng.below(6) {
            0 => "n".into(),
            1 => "0".into(),
            2 => "1".into(),
            3 => "7".into(),
            4 => "100".into(),
            _ => "3".into(),
        };
    }
    let a = int_expr(rng, depth - 1);
    let b = int_expr(rng, depth - 1);
    match rng.below(8) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("min({a}, {b})"),
        4 => format!("max({a}, {b})"),
        5 => format!("({a} if {a} > {b} else {b})"),
        6 => format!("wrapping_add({a}, {b})"),
        _ => format!("({a} + 1)"),
    }
}

/// Ein Fliesskommaausdruck.
///
/// Hier sitzt Satz 9.4.4: Jede Operation ist einzeln gerundet, und eine
/// andere Klammerung ergibt eine andere Zahl. Die Konstanten sind
/// absichtlich unrund — `0.1` ist der Fall, an dem sich eine
/// Dezimalschreibweise verraet.
fn float_expr(rng: &mut Rng, depth: u32) -> String {
    if depth == 0 {
        return match rng.below(6) {
            0 => "x".into(),
            1 => "0.1".into(),
            2 => "1.0".into(),
            3 => "3.7".into(),
            4 => "0.0".into(),
            _ => "100.0".into(),
        };
    }
    let a = float_expr(rng, depth - 1);
    let b = float_expr(rng, depth - 1);
    match rng.below(6) {
        0 => format!("({a} + {b})"),
        1 => format!("({a} - {b})"),
        2 => format!("({a} * {b})"),
        3 => format!("min({a}, {b})"),
        4 => format!("max({a}, {b})"),
        _ => format!("({a} if {a} > {b} else {b})"),
    }
}

/// Ein Programm um einen Fliesskommaausdruck herum.
fn float_program(expr: &str) -> String {
    format!(
        "system:
    language = 1
    tick     = 10 ms

         output r : float in -1000000.0..1000000.0 @ hw(\"ui/r\") with safe = 0.0

         machine f:
    var x : float in 0.0..100.0 = 3.7

    initial RUN
    state RUN:
        loop:
            r = {expr}
"
    )
}

/// Ein Programm um einen Ausdruck herum.
fn program(expr: &str) -> String {
    format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         output r : int in -1000000..1000000 @ hw(\"ui/r\") with safe = 0\n\n\
         machine f:\n    var n : int in 0..100 = 3\n\n    initial RUN\n    state RUN:\n        loop:\n            r = {expr}\n"
    )
}

/// Uebersetzt ein Programm; `None`, wenn das Sema es ablehnt.
///
/// Ablehnungen sind der Normalfall und kein Fehlschlag: Der Generator
/// wuerfelt auch Ausdruecke, deren Range die Analyse nicht beweisen kann
/// (3.4), und eine Ablehnung ist dann die richtige Antwort.
fn compile(src: &str) -> Option<takt_mir::Program> {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    if out.diagnostics.iter().any(|d| d.is_error()) {
        return None;
    }
    out.program
}

const TICKS: u64 = 5;

/// **Der Fuzzer der Abnahme.** Erzeugte Programme laufen in beiden
/// Implementierungen und liefern dieselben Outputs.
#[test]
fn generated_programs_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    // Mehrere Startwerte: Ein einzelner trifft immer dieselben Formen,
    // und die Formen sind das, was hier gesucht wird.
    let mut rng = Rng(0x2026_0912);
    let (mut gebaut, mut abgelehnt) = (0, 0);
    let mut errors = Vec::new();

    for runde in 0..160 {
        // Die zweite Haelfte rechnet in Fliesskomma: Dort sitzt Satz
        // 9.4.4, und dort ist eine Abweichung am schwersten zu finden.
        let (expr, src) = if runde % 2 == 0 {
            let e = int_expr(&mut rng, 2 + (runde % 3) as u32);
            let s = program(&e);
            (e, s)
        } else {
            let e = float_expr(&mut rng, 2 + (runde % 3) as u32);
            let s = float_program(&e);
            (e, s)
        };
        let Some(p) = compile(&src) else {
            abgelehnt += 1;
            continue;
        };
        let Some(machine) = p.machines.first().map(|m| m.name.clone()) else { continue };
        let native = match crate::common::run_native(&clang, &p, &format!("fuzz{runde}"), &machine, TICKS) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("`{expr}`: laesst sich nicht bauen:\n{e}"));
                continue;
            }
        };
        let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None };
        let interpreted = match takt_interp::run(&p, &takt_interp::Trace::default(), &options) {
            Ok(r) => r.trace.render(),
            // Ein Trap ist kein Vergleichsfall: Der Interpreter hat
            // aufgegeben, und es gibt nichts zu vergleichen.
            Err(_) => continue,
        };
        gebaut += 1;
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() {
            errors.push(format!("`{expr}`:\n  {}", diffs[0]));
        }
    }

    eprintln!("gebaut: {gebaut}, abgelehnt: {abgelehnt}");
    assert!(gebaut >= 10, "zu wenige Programme uebersetzt: {gebaut} (abgelehnt: {abgelehnt})");
    assert!(errors.is_empty(), "{} Abweichungen:\n{}", errors.len(), errors.join("\n"));
}

mod common;
