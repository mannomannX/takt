//! Instrumentierung (11.2, 12.8): `pc` im Zustand bekommt je Anweisung
//! oder je Zustandswechsel, wo die Maschine steht; der Default folgt dem
//! Profil und dem Ziel.

use takt_llvm::{Instrument, Target};
use takt_mir::program::RuntimeProfile;

fn program(src: &str) -> takt_mir::Program {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

const SRC: &str = "system:\n    language = 1\n    tick = 1 ms\n
output y : int in 0..9 @ hw(\"o/y\") with safe = 0
machine m:
    var n : int in 0..9 = 0
    initial A
    state A:
        loop:
            n = 1
            y = n
        when n == 1: -> B
    state B:
        loop:
            y = 2
";

fn stores(ir: &str) -> usize {
    ir.lines().filter(|l| l.trim_start().starts_with("store i32 ")).count()
}

#[test]
fn statements_store_more_than_states_and_off_stores_nothing_extra() {
    let p = program(SRC);
    let triple = Target::X86_64_WINDOWS.triple;
    let off = takt_llvm::lower::program_with(&p, triple, "t", Instrument::Off).ir;
    let states = takt_llvm::lower::program_with(&p, triple, "t", Instrument::States).ir;
    let statements = takt_llvm::lower::program_with(&p, triple, "t", Instrument::Statements).ir;
    assert_eq!(off, takt_llvm::lower::program(&p, triple, "t").ir, "ohne Angabe: keine Instrumentierung");
    assert!(stores(&states) > stores(&off), "Zustandswechsel schreiben pc");
    assert!(stores(&statements) > stores(&states), "jede Anweisung schreibt pc");
}

#[test]
fn the_default_follows_profile_and_target() {
    assert_eq!(Instrument::default_for(None, Target::X86_64_WINDOWS), Instrument::Statements);
    assert_eq!(Instrument::default_for(None, Target::THUMBV7EM), Instrument::States);
    assert_eq!(Instrument::default_for(Some(RuntimeProfile::Rtos), Target::X86_64_WINDOWS), Instrument::States);
    assert_eq!(Instrument::default_for(Some(RuntimeProfile::Boot), Target::THUMBV7EM), Instrument::Off);
    assert_eq!(Instrument::parse("states"), Some(Instrument::States));
    assert_eq!(Instrument::parse("alles"), None);
}
