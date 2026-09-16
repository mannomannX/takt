//! Die Hashkette ueber den Trace (12.5, A3): deterministisch, an die
//! Logik gebunden, empfindlich fuer jede Zeile; `seal` und `verify`.

use takt_interp::record::{Header, Recording, chain};
use takt_interp::{RunOptions, Trace, run};

const PROGRAM: &str = include_str!("../../../corpus-try/37_follows.takt");

fn compile(src: &str) -> takt_mir::Program {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

#[test]
fn the_chain_is_deterministic_and_bound_to_logic_and_lines() {
    let p = compile(PROGRAM);
    let options = RunOptions { ticks: 12, ..Default::default() };
    let a = run(&p, &Trace::default(), &options).expect("Lauf");
    let b = run(&p, &Trace::default(), &options).expect("Lauf");
    let logic = takt_mir::hash::logic_hash(&p).to_string();
    let (ha, hb) = (chain(&a.trace, &logic), chain(&b.trace, &logic));
    assert_eq!(ha, hb);
    assert_eq!(ha.len(), 64, "{ha}");
    assert_ne!(chain(&a.trace, "andere-logik"), ha);

    let mut altered = a.trace.clone();
    let text = altered.render().replacen("out level 7", "out level 8", 1);
    altered = Trace::parse(&text).expect("lesbar");
    assert_ne!(chain(&altered, &logic), ha, "eine geaenderte Zeile aendert die Kette");
    let shorter = Trace { lines: a.trace.lines[..a.trace.lines.len() - 1].to_vec() };
    assert_ne!(chain(&shorter, &logic), ha, "eine fehlende Zeile aendert die Kette");
}

#[test]
fn a_sealed_recording_verifies_its_trace_and_rejects_another() {
    let p = compile(PROGRAM);
    let options = RunOptions { ticks: 12, ..Default::default() };
    let r = run(&p, &Trace::default(), &options).expect("Lauf");
    let header = Header::of(&p, None, &r.start_params, 12);
    let sealed = Recording { header, inputs: Trace::default() }.seal(&r.trace);
    let text = sealed.render();
    assert!(text.contains("#! kette "), "{text}");
    let read = Recording::parse(&text).expect("lesbar");
    assert_eq!(read.header.chain, sealed.header.chain);
    assert!(read.verify(&r.trace).is_ok());

    let other = run(&p, &Trace::default(), &RunOptions { ticks: 11, ..Default::default() }).expect("Lauf");
    let e = read.verify(&other.trace).expect_err("anderer Trace");
    assert!(e.contains("weicht ab"), "{e}");
    let unsealed = Recording { header: Header::of(&p, None, &r.start_params, 12), inputs: Trace::default() };
    assert!(unsealed.verify(&r.trace).expect_err("ohne Kette").contains("kein Kettenende"));
}
