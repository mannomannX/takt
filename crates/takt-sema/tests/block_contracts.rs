//! Block-Vertraege (5.7, B2): `requires`/`ensures` am `step` als
//! Beweisverpflichtungen in der MIR, `result` nur im `ensures`.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::expr::ExprKind;
use takt_sema::{Build, Options};

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// Die `VarId`, unter der ein Vertrag `result` liest, und der Name, den der
/// Rahmen des Schritts an dieser Stelle fuehrt.
fn result_in_frame(p: &Program) -> String {
    let b = p.blocks.iter().find(|b| b.name == "limiter").expect("Block");
    let base = (b.params.len() + b.state_vars.len()) as u32;
    let step = &p.fns[b.step.expect("step").index()];
    let mut ids = Vec::new();
    let mut stack = vec![&b.ensures[0]];
    while let Some(x) = stack.pop() {
        if let ExprKind::Var(v) = x.kind
            && v.0 >= base + step.params.len() as u32
        {
            ids.push(v.0);
        }
        stack.extend(x.children());
    }
    assert_eq!(ids.len(), 1, "`ensures result <= hi` liest genau eine Lokale: {ids:?}");
    step.locals[(ids[0] - base) as usize].name.clone()
}

#[test]
fn result_in_ensures_is_the_local_named_result() {
    let p = compile(PROGRAM).expect("uebersetzt");
    assert_eq!(result_in_frame(&p), "result");
    // Eine Lokale im Rumpf darf den Platz von `result` nicht einnehmen.
    let with_local = PROGRAM.replace(
        "        last = x if x < hi else hi\n",
        "        var capped : float = x if x < hi else hi\n        last = capped\n",
    );
    assert_ne!(with_local, PROGRAM);
    let p = compile(&with_local).expect("uebersetzt");
    assert_eq!(result_in_frame(&p), "result", "die Lokale `capped` stand an der Stelle von `result`");
}

#[test]
fn a_contract_reads_no_name_outside_the_block() {
    // 5.7: `requires` ueber Parametern und Blockzustand; die Instanz der
    // Maschine sieht der Block nicht.
    for (what, from, to, expected) in [
        ("requires ohne bool", "requires x >= 0", "requires x", "[SC-3]"),
        ("Maschinenvariable", "requires x >= 0", "requires lim.last >= 0", "`lim`"),
    ] {
        let src = PROGRAM.replace(from, to);
        assert_ne!(src, PROGRAM, "{what}: Ersetzung griff nicht");
        let e = compile(&src).expect_err(what).join("\n");
        assert!(e.contains(expected), "{what}: {e}");
    }
    // 2.3: Ohne Rueckgabetyp gibt es keinen `step` und kein `result`.
    let no_return = PROGRAM.replace("step(x: float) -> float requires", "step(x: float) requires");
    let e = compile(&no_return).expect_err("ohne Rueckgabe").join("\n");
    assert!(e.contains("[P]"), "{e}");
}

#[test]
fn a_violated_requires_does_not_fault_at_run_time() {
    // 5.7: Vertraege sind Beweisverpflichtungen, keine Laufzeitpruefungen.
    // `level = -3` verletzt `requires x >= 0`; der Schritt rechnet trotzdem.
    use takt_interp::{RunOptions, Trace, run};
    let p = compile(PROGRAM).expect("uebersetzt");
    let stimulus = Trace::parse("t=0 in level -3.0\nt=1 in level 7.0\n").expect("Stimulus");
    let trace = run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf").trace.render();
    assert!(!trace.contains("fault"), "kein Fault: {trace}");
    assert!(trace.contains("t=0 out out -3.0\n"), "der Rumpf liefert x: {trace}");
    assert!(trace.contains("t=1 out out 5.0\n"), "und begrenzt auf hi: {trace}");
}

/// 5.7: Ein Block ist geschlossen. Rumpf und Vertrag lesen nur Parameter,
/// den Zustand des Blocks, Konstanten und `param`; ein Channel oder eine
/// Groesse einer Maschine ist SC-2.
#[test]
fn a_block_reads_nothing_outside_itself() {
    for (what, from, to, name) in [
        ("requires mit Channel", "requires x >= 0", "requires level >= 0", "`level`"),
        ("Rumpf liest Channel", "last = x if x < hi else hi", "last = level if x < hi else hi", "`level`"),
        ("Rumpf liest Output", "last = x if x < hi else hi", "last = out if x < hi else hi", "`out`"),
        ("Rumpf liest Maschinenvariable", "last = x if x < hi else hi", "last = m.seen if x < hi else hi", "`m`"),
        ("Rumpf liest die Zeit", "last = x if x < hi else hi", "last = x if now < 1 s else hi", "`now`"),
    ] {
        let src = PROGRAM.replace(from, to).replace(
            "    var lim = limiter(hi = 5)\n",
            "    pub var seen : float = 0\n    var lim = limiter(hi = 5)\n",
        );
        assert_ne!(src, PROGRAM, "{what}: Ersetzung griff nicht");
        let e = compile(&src).expect_err(what);
        assert_eq!(e.len(), 1, "{what}: {e:?}");
        assert!(
            e[0].contains("[SC-2]") && e[0].contains(name) && e[0].contains("ausserhalb des Blocks"),
            "{what}: {e:?}"
        );
    }
    // Konstanten und `param` bleiben lesbar.
    let src = PROGRAM
        .replace("input  level", "const GAIN : float = 2\nparam LIMIT : float in 0..10 = 4\n\ninput  level")
        .replace("last = x if x < hi else hi", "last = x * GAIN if x < LIMIT else hi");
    assert!(compile(&src).is_ok(), "{:?}", compile(&src).err());
}
