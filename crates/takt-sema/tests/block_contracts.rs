//! Block-Vertraege (5.7, B2): `requires`/`ensures` am `step` als
//! Beweisverpflichtungen in der MIR, `result` nur im `ensures`.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::expr::ExprKind;
use takt_sema::{Build, Options};

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const PROGRAM: &str = include_str!("../../../corpus-try/48_contracts.takt");

#[test]
fn contracts_land_on_the_block_with_result_as_a_local() {
    let p = compile(PROGRAM).expect("uebersetzt");
    let b = p.blocks.iter().find(|b| b.name == "limiter").expect("Block");
    assert_eq!((b.requires.len(), b.ensures.len()), (1, 1));
    let step = &p.fns[b.step.expect("step").index()];
    // Rahmen: `hi`, `last`, `x`, dann `result`.
    assert_eq!(step.locals.last().map(|v| v.name.as_str()), Some("result"));
    let mentions_result = |e: &takt_mir::expr::Expr| {
        let mut found = false;
        let mut stack = vec![e];
        while let Some(x) = stack.pop() {
            if matches!(x.kind, ExprKind::Var(v) if v.index() == 3) {
                found = true;
            }
            stack.extend(x.children());
        }
        found
    };
    assert!(mentions_result(&b.ensures[0]) && !mentions_result(&b.requires[0]));
}

#[test]
fn a_contract_must_be_a_condition_and_result_belongs_to_ensures() {
    let bad_type = PROGRAM.replace("ensures result <= hi", "ensures result");
    let e = compile(&bad_type).expect_err("kein bool").join("\n");
    assert!(e.contains("SC-3"), "{e}");
    let misplaced = PROGRAM.replace("requires x >= 0", "requires result >= 0");
    let e = compile(&misplaced).expect_err("result im requires").join("\n");
    assert!(e.contains("`result` ist nicht definiert"), "{e}");
}
