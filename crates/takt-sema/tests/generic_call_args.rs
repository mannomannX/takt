//! Argumente eines generischen Aufrufs (3.12) werden einmal gesenkt: Die
//! Loesung der Typvariablen liest ihre Typen, ohne Seiteneffekte der
//! Senkung zu hinterlassen.

use takt_diag::Policy;
use takt_mir::stmt::{Method, Stmt, StmtKind};
use takt_sema::{Build, Options};

/// Die `step`-Aufrufe einer Anweisungsliste, rekursiv.
fn steps(stmts: &[Stmt]) -> usize {
    stmts
        .iter()
        .map(|s| match &s.kind {
            StmtKind::MethodCall { method: Method::Step, .. } => 1,
            StmtKind::If { then, otherwise, .. } => steps(&then.stmts) + steps(&otherwise.stmts),
            _ => 0,
        })
        .sum()
}

/// 5.7: Eine anonyme Blockinstanz als Argument einer generischen Funktion
/// ist *eine* Instanz mit *einem* `step` je Tick, wie bei einer gewoehnlichen
/// Funktion.
#[test]
fn an_anonymous_block_in_a_generic_argument_is_one_instance() {
    let src = "system:
    language = 1
    tick     = 10 ms

fn same[type T: pod](x: T) -> T:
    return x

fn plain(x: bool) -> bool:
    return x

command go
output a : bool @ hw(\"o/a\") with safe = false
output b : bool @ hw(\"o/b\") with safe = false

machine m:
    initial RUN
    state RUN:
        loop:
            a = same(rising(go))
            b = plain(falling(go))
";
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let p = out.program.expect("Programm");
    let m = &p.machines[0];
    assert_eq!(m.layout.block_instances.len(), 2, "{:?}", m.layout.block_instances);
    let loops: usize = m.states.iter().map(|s| steps(&s.loop_block.stmts)).sum();
    assert_eq!(loops, 2, "je Aufruf ein `step`");
}
