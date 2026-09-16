//! `o.sent` (8.8, FB-132): der im letzten Tick gesendete Ausschnitt eines
//! Ausgabestroms, mit Unit-Delay und statischer Hoechstlaenge.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const PROGRAM: &str = include_str!("../../../corpus-try/43_sent.takt");

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

#[test]
fn a_model_reads_what_the_driver_took_one_tick_later() {
    let p = compile(PROGRAM).expect("uebersetzt");
    let stimulus = Trace::parse("t=1 cmd go\n").expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 6, ..Default::default() }).expect("Lauf").trace.render();
    // Tick 1: `send`, der Treiber holt ein Byte; Tick 2 bis 4 sehen je eines, Tick 5 nichts.
    for line in ["t=2 out got 1", "t=2 out any true", "t=5 out any false", "t=5 out got 0"] {
        assert!(
            t.contains(line),
            "{line} fehlt:
{t}"
        );
    }
    assert!(
        !t.contains("t=1 out any true"),
        "Unit-Delay:
{t}"
    );
}

#[test]
fn the_bytes_arrive_in_send_order() {
    let src = PROGRAM.replace(
        "            got = last.or(default).len
",
        "            got = 0
            for x in last.or(default):
                got = x as int
",
    );
    let p = compile(&src).expect("uebersetzt");
    let stimulus = Trace::parse(
        "t=1 cmd go
",
    )
    .expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 6, ..Default::default() }).expect("Lauf").trace.render();
    for line in ["t=2 out got 97", "t=3 out got 98", "t=4 out got 99", "t=5 out got 0"] {
        assert!(
            t.contains(line),
            "{line} fehlt:
{t}"
        );
    }
}

#[test]
fn sent_is_only_for_output_streams() {
    let e = compile(
        "system:\n    language = 1\n
input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz, framing = lines, overflow = fault
output any : bool @ hw(\"o/any\") with safe = false
machine m:
    initial RUN
    state RUN:
        loop:
            any = rx.sent.valid
",
    )
    .expect_err("Fehler erwartet");
    assert!(e.join("\n").contains("nur an einem Ausgabestrom"), "{e:?}");
}
