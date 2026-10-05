//! Maschinen-Replay (12.5, A2a): Die Scheibe einer Maschine — Stimulus
//! und alles, was sie von fremden Maschinen liest — spielt sie allein ab
//! und zeigt dieselben Zeilen wie der Gesamtlauf.

use takt_interp::record::{Header, Recording, machine_lines, machine_slice};
use takt_interp::trace::LineKind;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::{MachineId, Program};

const FOLLOWS: &str = include_str!("../../../corpus-try/37_follows.takt");

fn compile(src: &str) -> Program {
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

/// Ein zweites Programm mit Input, internem Strom samt Handler, Job und
/// `persist`: `src` liest den Input, sendet und rechnet einen Hash, `dst`
/// zaehlt die Elemente und liest `src.x` mit Unit-Delay.
const MIXED: &str = "system:
    language = 1
    tick     = 10 ms

native job sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 640, duration = 25 ms, total

input  level     : int in 0..100 @ hw(\"i/level\")
output level_sim : int           @ sim(\"i/level\")
output word      : u32           @ hw(\"o/word\") with safe = 0
output seen      : int in 0..999 @ hw(\"o/seen\") with safe = 0

stream<u8> q with capacity = 8

machine src:
    pub var x : int in 0..100 = 0
    persist var runs : int in 0..99 = 0
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        enter:
            runs = min(runs + 1, 99)
        loop:
            x = level.or(0)
            send q, 1
        sequence:
            job v = sha256(msg)
            until v.done timeout 1 s
            word = v.result.or(default)[0] as u32

machine dst:
    var n : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            seen = min(n + src.x, 999)
        on q as e:
            n = min(n + 1, 999)
";

const LEVELS: &str = "t=0 in level 3\nt=2 in level 40\nt=5 in level 7\n";

#[test]
fn a_program_with_input_stream_job_and_persist_replays_per_machine() {
    let p = compile(MIXED);
    let slice = slice_reproduces(&p, "dst", LEVELS, 12);
    let text = slice.inputs.render();
    assert!(text.contains("pub src x 40"), "{text}");
    slice_reproduces(&p, "src", LEVELS, 12);
}

/// Eine veraenderte fremde Zeile in der Scheibe aendert, was die Maschine
/// allein zeigt: Der Vergleich mit dem Gesamtlauf (`takt replay
/// --machine`) bemerkt die Abweichung.
#[test]
fn a_changed_foreign_line_in_the_slice_is_noticed() {
    let p = compile(MIXED);
    let stimulus = Trace::parse(LEVELS).expect("Stimulus");
    let options = RunOptions { ticks: 12, ..Default::default() };
    let whole = run(&p, &stimulus, &options).expect("Gesamtlauf");
    let m = machine(&p, "dst");
    let recording = Recording { header: Header::of(&p, None, &whole.start_params, 12), inputs: stimulus };
    let text = machine_slice(&p, &recording, &whole.trace, m).render();
    assert!(text.contains("pub src x 40\n"), "{text}");
    let changed = Recording::parse(&text.replacen("pub src x 40\n", "pub src x 41\n", 1)).expect("lesbar");
    let alone = run(&p, &changed.inputs, &RunOptions { ticks: 12, only: Some("dst".into()), ..Default::default() })
        .expect("Scheibe");
    let (want, got) = (machine_lines(&p, &whole.trace, m).render(), machine_lines(&p, &alone.trace, m).render());
    assert_ne!(want, got, "die Abweichung bleibt unbemerkt");
}
