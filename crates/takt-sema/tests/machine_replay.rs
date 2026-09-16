//! Maschinen-Replay (12.5, A2a): Die Scheibe einer Maschine — Stimulus
//! und alles, was sie von fremden Maschinen liest — spielt sie allein ab
//! und zeigt dieselben Zeilen wie der Gesamtlauf.

use takt_interp::record::{Header, Recording, machine_lines, machine_slice};
use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::{MachineId, Program};

const FOLLOWS: &str = include_str!("../../../corpus-try/37_follows.takt");

fn compile(src: &str) -> Program {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn machine(p: &Program, name: &str) -> MachineId {
    MachineId(p.machines.iter().position(|m| m.name == name).expect("Maschine") as u32)
}

/// Gesamtlauf, Scheibe, Lauf der Scheibe: die Zeilen der Maschine stimmen.
fn slice_reproduces(p: &Program, name: &str, stimulus: &str, ticks: u64) -> Recording {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let options = RunOptions { ticks, ..Default::default() };
    let whole = run(p, &stimulus, &options).expect("Gesamtlauf");
    let m = machine(p, name);
    let recording = Recording { header: Header::of(p, None, &whole.start_params, ticks), inputs: stimulus };
    let slice = machine_slice(p, &recording, &whole.trace, m);
    let read = Recording::parse(&slice.render()).expect("lesbar");
    assert_eq!(read.header.machine.as_deref(), Some(name));
    let alone = RunOptions { ticks, only: read.header.machine.clone(), ..Default::default() };
    let alone = run(p, &read.inputs, &alone).expect("Scheibe");
    let (want, got) = (machine_lines(p, &whole.trace, m).render(), machine_lines(p, &alone.trace, m).render());
    assert!(!want.is_empty(), "die Maschine hat Zeilen");
    assert_eq!(want, got, "{name}: die Scheibe weicht ab");
    slice
}

#[test]
fn a_follower_reads_its_source_fresh_from_the_slice() {
    let p = compile(FOLLOWS);
    let slice = slice_reproduces(&p, "ctrl", "", 30);
    let text = slice.inputs.render();
    assert!(
        text.contains("pub plant x") && text.contains("state plant HIGH") && text.contains("signal plant ping"),
        "{text}"
    );
    assert!(!text.contains("out level"), "die eigenen Outputs stehen nicht in der Scheibe:\n{text}");
}

#[test]
fn a_watcher_reads_with_unit_delay_from_the_slice() {
    let p = compile(FOLLOWS);
    let slice = slice_reproduces(&p, "watch", "", 30);
    // Nur `pub var`, Zustand und Signal der fremden Maschinen; keine
    // Eingaben, weil das Programm keine hat.
    assert!(slice.inputs.lines.iter().all(|l| !matches!(l.kind, LineKind::Input { .. })));
}

#[test]
fn the_source_itself_replays_without_foreign_lines() {
    let p = compile(FOLLOWS);
    let slice = slice_reproduces(&p, "plant", "", 30);
    let text = slice.inputs.render();
    assert!(!text.contains("plant"), "nichts von der Maschine selbst:\n{text}");
    assert!(text.contains("state ctrl RUN") && text.contains("out level"), "{text}");
}

#[test]
fn foreign_lines_need_the_machine_replay_and_the_machine_must_run() {
    let p = compile(FOLLOWS);
    let stimulus = Trace::parse("t=1 pub plant x 7\n").expect("Stimulus");
    // Ohne `only` bleibt die Zeile eine Beobachtung und stoert nicht.
    run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).expect("Gesamtlauf");
    let e = run(&p, &stimulus, &RunOptions { ticks: 2, only: Some("plant".into()), ..Default::default() })
        .expect_err("die eigene Maschine");
    assert!(format!("{e:?}").contains("selbst"), "{e:?}");
    let e = run(&p, &Trace::default(), &RunOptions { ticks: 2, only: Some("nope".into()), ..Default::default() })
        .expect_err("unbekannt");
    assert!(format!("{e:?}").contains("gibt es nicht"), "{e:?}");
}
