//! `follows` (Referenz 7.2, v1.1; Pruefung 33): topologische Schrittordnung,
//! frische Lesevorgaenge fuer Follower, Psi fuer alle anderen; Satz 9.4.1
//! gilt ueber den linearen Erweiterungen der Kanten.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> (Option<Program>, Vec<String>) {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let diags: Vec<String> = out.diagnostics.iter().map(|d| format!("{d}")).collect();
    let errors = out.diagnostics.iter().any(|d| d.is_error());
    (if errors { None } else { out.program }, diags)
}

fn ok(body: &str) -> Program {
    let (p, diags) = compile(body);
    p.unwrap_or_else(|| panic!("unerwartete Fehler:\n{}", diags.join("\n")))
}

fn trace(p: &Program, ticks: u64, seed: Option<u64>) -> String {
    let options = RunOptions { ticks, order_seed: seed, ..Default::default() };
    run(p, &Trace::default(), &options).expect("Lauf").trace.render()
}

/// `src` zaehlt, `fresh` folgt ihr, `stale` liest sie mit Unit-Delay.
const PROGRAM: &str = "
output y : int in 0..999 @ hw(\"o/y\") with safe = 0
output z : int in 0..999 @ hw(\"o/z\") with safe = 0

machine src:
    pub var x : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            x = x + 1

machine fresh follows src:
    initial RUN
    state RUN:
        loop:
            y = src.x

machine stale:
    initial RUN
    state RUN:
        loop:
            z = src.x
";

#[test]
fn a_follower_reads_the_fresh_value_and_everyone_else_reads_psi() {
    // 7.2: „Ein Follower liest pub var … frisch (Wert nach deren Schritt)";
    // `stale` bekommt den Wert des vorigen Ticks.
    let p = ok(PROGRAM);
    let t = trace(&p, 3, None);
    for line in ["t=0 out y 1", "t=0 out z 0", "t=1 out y 2", "t=1 out z 1", "t=2 out y 3", "t=2 out z 2"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

#[test]
fn the_declaration_order_does_not_matter() {
    // `fresh` steht vor `src`; die Ordnung ist topologisch (7.2).
    let p = ok("output y : int in 0..999 @ hw(\"o/y\") with safe = 0

machine fresh follows src:
    initial RUN
    state RUN:
        loop:
            y = src.x

machine src:
    pub var x : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            x = x + 1
");
    let t = trace(&p, 2, None);
    assert!(t.contains("t=1 out y 2"), "{t}");
}

#[test]
fn theorem_9_4_1_holds_over_the_linear_extensions() {
    // Jede zulaessige Ordnung liefert denselben Trace; ohne die Kante ist
    // der Trace ein anderer — sonst pruefte der Test nichts.
    let p = ok(PROGRAM);
    let reference = trace(&p, 20, None);
    for seed in [3, 17, 9001, 424242] {
        assert_eq!(reference, trace(&p, 20, Some(seed)), "Seed {seed}");
    }
    let without = ok(&PROGRAM.replace("machine fresh follows src:", "machine fresh:"));
    assert_ne!(reference, trace(&without, 20, None));
}

#[test]
fn a_cycle_is_an_error() {
    let (p, diags) = compile(
        "machine a follows b:
    pub var x : int = 0
    initial RUN
    state RUN:
        loop:
            x = b.y

machine b follows a:
    pub var y : int = 0
    initial RUN
    state RUN:
        loop:
            y = a.x
",
    );
    assert!(p.is_none());
    assert!(diags.iter().any(|d| d.contains("SC-33") && d.contains("Zyklus")), "{diags:?}");
}

#[test]
fn a_missing_follows_warns_only_when_both_run_in_the_same_tick() {
    // FB-18: Warnung, nicht Fehler — der Unit-Delay ist manchmal gewollt.
    let (_, diags) = compile(PROGRAM);
    assert!(
        diags.iter().any(|d| d.contains("SC-33") && d.contains("Verzoegerung") && d.contains("stale")),
        "{diags:?}"
    );
    // Mit versetzter Phase laufen die beiden nie im selben Tick (7.2).
    let (_, diags) = compile(
        "output z : int in 0..999 @ hw(\"o/z\") with safe = 0

machine src every 2 ms:
    pub var x : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            x = x + 1

machine stale every 2 ms phase 1 ms:
    initial RUN
    state RUN:
        loop:
            z = src.x
",
    );
    assert!(!diags.iter().any(|d| d.contains("SC-33")), "{diags:?}");
}

#[test]
fn a_fresh_read_on_the_fault_path_warns() {
    // FB-38: dieselbe Zeile, zwei Bedeutungen — frisch in der Schrittphase,
    // Psi in der Abort-Phase.
    let (_, diags) = compile(
        "output y : int in 0..999 @ hw(\"o/y\") with safe = 0

machine src:
    pub var x : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            x = x + 1

machine fresh follows src:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            y = src.x
            check src.x < 500, \"zu hoch\"
    state SAFE:
        enter:
            y = src.x
",
    );
    assert!(diags.iter().any(|d| d.contains("SC-33") && d.contains("Fault-Pfad")), "{diags:?}");
}
