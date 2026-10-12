//! `for i in range(n)` im erzeugten Code wie im Interpreter (4.1, Satz 9.4.4),
//! auch wo `n` die Grenze des Typs der Schleifenvariable ist: Sie fasst nur
//! `0..n-1`, und der Vergleich mit `n` in ihrem Typ machte aus
//! `range(32768)` ueber einem `i16` eine Schleife ohne Durchlauf (FB-529).

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::Clang;

/// Zaehlt `n` Durchlaeufe in einer Maschine und in einer Funktion.
fn program(n: u32) -> String {
    format!(
        "\
system:
    language = 1
    tick     = 100 ms

output in_machine  : int in 0..70000 @ hw(\"o/m\") with safe = 0
output in_function : int in 0..70000 @ hw(\"o/f\") with safe = 0

fn count() -> int:
    var c : int in 0..70000 = 0
    for i in range({n}):
        c = c + 1
    return c

machine m:
    var c : int in 0..70000 = 0
    initial RUN
    state RUN:
        loop:
            c = 0
            for i in range({n}):
                c = c + 1
            in_machine = c
            in_function = count()
"
    )
}

/// **Eine Schleife bis an die Grenze ihres Zaehlertyps laeuft ganz**: 128
/// und 32768 sind die Grenzen von `i8` und `i16`, 256 und 65536 die der
/// vorzeichenlosen. Beide Implementierungen zaehlen `n` Durchlaeufe.
#[test]
fn a_loop_up_to_the_limit_of_its_counter_runs_completely() {
    let Some(path) = common::clang_path() else { return };
    for n in [128u32, 256, 32768, 65536] {
        let src = program(n);
        let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
        let out = takt_sema::compile(&src, &options);
        assert!(!out.has_errors(), "{n}: {:?}", out.diagnostics);
        let p = out.program.expect("Programm");
        let interpreted = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 2, ..Default::default() })
            .expect("Lauf")
            .trace
            .render();
        assert!(interpreted.contains(&format!("out in_machine {n}")), "{n}: {interpreted}");
        let native = common::run_native_all(&Clang::At(path.clone()), &p, &format!("range_{n}"), 2).expect("nativ");
        let diffs = takt_conformance::compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{n}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    }
}
