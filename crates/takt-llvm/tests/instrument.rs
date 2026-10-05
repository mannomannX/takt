//! Instrumentierung (11.2, 12.8): `pc` im Zustand bekommt je Anweisung
//! oder je Zustandswechsel, wo die Maschine steht; der Default folgt dem
//! Profil und dem Ziel.

use takt_llvm::{Instrument, Target};
use takt_mir::program::RuntimeProfile;

fn program(src: &str) -> takt_mir::Program {
    let o = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
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
    let off = takt_llvm::lower::program_with(&p, triple, &takt_llvm::symbols::Prefix::default(), Instrument::Off).ir;
    let states =
        takt_llvm::lower::program_with(&p, triple, &takt_llvm::symbols::Prefix::default(), Instrument::States).ir;
    let statements =
        takt_llvm::lower::program_with(&p, triple, &takt_llvm::symbols::Prefix::default(), Instrument::Statements).ir;
    assert_eq!(
        off,
        takt_llvm::lower::program(&p, triple, &takt_llvm::symbols::Prefix::default()).ir,
        "ohne Angabe: keine Instrumentierung"
    );
    assert!(stores(&states) > stores(&off), "Zustandswechsel schreiben pc");
    assert!(stores(&statements) > stores(&states), "jede Anweisung schreibt pc");
}

#[test]
fn the_default_follows_profile_and_target() {
    assert_eq!(Instrument::default_for(None, Target::X86_64_WINDOWS), Instrument::Statements);
    assert_eq!(Instrument::default_for(None, Target::THUMBV7EM), Instrument::States);
    assert_eq!(Instrument::default_for(Some(RuntimeProfile::Shared), Target::X86_64_WINDOWS), Instrument::States);
    assert_eq!(Instrument::default_for(Some(RuntimeProfile::Baremetal), Target::X86_64_WINDOWS), Instrument::States);
    assert_eq!(Instrument::parse("states"), Some(Instrument::States));
    assert_eq!(Instrument::parse("alles"), None);
}

/// GEN-016: Unter `statements` traegt `pc` den Byte-Offset einer
/// Anweisung (11.2) — auch nach einem Zustandswechsel. Die Nummer des
/// betretenen Blatts gehoert in die Stufe `states`; im selben Feld waere
/// sie ein Offset, der auf keine Anweisung zeigt.
#[test]
fn under_statements_every_pc_store_is_a_statement_offset() {
    let p = program(SRC);
    let m = &p.machines[0];
    let st = takt_llvm::machine::state_struct(m, &p).expect("Struct");
    let pc = st.index_of(takt_llvm::machine::Role::Pc, 0).expect("pc");
    let mut offsets = std::collections::BTreeSet::new();
    for b in m.blocks() {
        b.walk(&mut |s| {
            offsets.insert(s.span.start);
        });
    }
    let triple = Target::X86_64_WINDOWS.triple;
    let ir =
        takt_llvm::lower::program_with(&p, triple, &takt_llvm::symbols::Prefix::default(), Instrument::Statements).ir;
    let field = format!("getelementptr inbounds %m_state, ptr %0, i32 0, i32 {pc}");
    let mut regs: Vec<String> = Vec::new();
    let mut stored = Vec::new();
    for line in ir.lines() {
        if line.starts_with("define ") {
            regs.clear();
        }
        let l = line.trim();
        if let Some((reg, rest)) = l.split_once(" = ")
            && rest == field
        {
            regs.push(reg.to_string());
        }
        if let Some(rest) = l.strip_prefix("store i32 ")
            && let Some((value, ptr)) = rest.split_once(", ptr ")
            && regs.iter().any(|r| r == ptr)
        {
            stored.push(value.parse::<u32>().unwrap_or_else(|_| panic!("pc mit `{value}`")));
        }
    }
    assert!(!stored.is_empty(), "kein Store auf pc gefunden");
    let foreign: Vec<u32> = stored.into_iter().filter(|v| !offsets.contains(v)).collect();
    assert!(foreign.is_empty(), "pc traegt Werte, die keine Anweisung sind: {foreign:?} (Anweisungen: {offsets:?})");
}

/// GEN-016: Jede Stufe hat ihren Namen, und der Default kennt jedes
/// Profil auf jedem Ziel.
#[test]
fn every_level_parses_and_every_profile_has_a_default() {
    for level in [Instrument::Statements, Instrument::States, Instrument::Off] {
        assert_eq!(Instrument::parse(level.name()), Some(level));
    }
    for target in Target::ALL {
        assert_eq!(Instrument::default_for(Some(RuntimeProfile::LinuxRt), target), Instrument::Statements);
        assert_eq!(Instrument::default_for(Some(RuntimeProfile::Baremetal), target), Instrument::States);
        assert_eq!(Instrument::default_for(Some(RuntimeProfile::Shared), target), Instrument::States);
        let bare = if target.is_bare_metal() { Instrument::States } else { Instrument::Statements };
        assert_eq!(Instrument::default_for(None, target), bare, "{}", target.name);
    }
}
