//! `o.sent` (8.8, FB-132): der im letzten Tick gesendete Ausschnitt eines
//! Ausgabestroms, mit Unit-Delay und statischer Hoechstlaenge.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const PROGRAM: &str = include_str!("../../../corpus-try/43_sent.takt");

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// Ein Ausgabestrom mit 4 Byte Puffer, den der Treiber mit einem Byte je
/// Tick leert; jedes `go` sendet drei Byte. `overflow` setzt `policy`.
fn overflowing(policy: &str) -> Program {
    compile(&format!(
        "system:
    language = 1
    tick     = 10 ms

output tx   : stream<u8> @ hw(\"uart0/tx\") with max_rate = 100 Hz, capacity = 4{policy}
output busy : bool       @ hw(\"o/busy\")   with safe = false

command go

machine dut:
    fault -> SAFE
    var msg : bytes<3> = default
    initial RUN
    state RUN:
        enter:
            msg.push(0x61)
            msg.push(0x62)
            msg.push(0x63)
        loop:
            busy = true
            if go:
                send tx, msg
    state SAFE:
        enter:
            busy = false
"
    ))
    .expect("uebersetzt")
}

/// 8.8: Zur Laufzeit ist `send` mit `len > tx.free` ein `StreamOverflow`.
/// Tick 1 legt drei Byte ab, bis Tick 2 holt der Treiber eines: zwei frei,
/// drei verlangt.
#[test]
fn a_send_beyond_the_free_space_is_a_stream_overflow() {
    let p = overflowing("");
    let stimulus = Trace::parse("t=1 cmd go\nt=2 cmd go\n").expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 4, ..Default::default() }).expect("Lauf").trace.render();
    assert!(t.contains("t=2 fault dut StreamOverflow \"Sendepuffer `tx` hat 2 Byte frei, 3 verlangt\" -> SAFE"), "{t}");
}

/// 8.8: Mit `overflow = drop` ist es ein Alert statt eines Faults; er ist
/// aktiv im Tick des Verwurfs und faellt beim naechsten gelungenen `send`.
#[test]
fn a_dropped_send_raises_an_alert_until_the_next_send_succeeds() {
    let p = overflowing(", overflow = drop");
    let stimulus = Trace::parse("t=1 cmd go\nt=2 cmd go\nt=6 cmd go\n").expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 8, ..Default::default() }).expect("Lauf").trace.render();
    let alerts: Vec<&str> = t.lines().filter(|l| l.contains(" alert dut ")).collect();
    assert_eq!(
        alerts,
        ["t=2 alert dut on \"Sendepuffer `tx` voll, 3 Byte verworfen\"", "t=6 alert dut off \"Sendepuffer `tx`\""],
        "{t}"
    );
    assert!(!t.contains("fault"), "{t}");
}

/// 8.8: Ohne `max_rate` ist die Hoechstlaenge von `sent` die Kapazitaet,
/// und der Treiber holt alles in einem Tick.
#[test]
fn sent_without_max_rate_carries_the_whole_capacity() {
    let src = PROGRAM
        .replace("with max_rate = 100 Hz, capacity = 16", "with capacity = 16")
        .replace("    var last : bytes<1>? = none", "    var last : bytes<16>? = none");
    let p = compile(&src).expect("uebersetzt");
    let stimulus = Trace::parse("t=1 cmd go\n").expect("Stimulus");
    let t = run(&p, &stimulus, &RunOptions { ticks: 4, ..Default::default() }).expect("Lauf").trace.render();
    for line in ["t=2 out got 3", "t=2 out any true", "t=3 out any false"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

/// 12.5, FB-435: Was der Treiber als `tx` meldet, gilt statt des Modells bis
/// zur naechsten Zeile, und ein `send` im Tick zieht davon ab. Das Modell
/// haette in Tick 2 zwei Byte frei; gemeldet sind vier, und sie halten auch
/// in Tick 3. Erst die Meldung von zwei Byte in Tick 4 laesst den `send`
/// ueberlaufen. Die Simulation selbst schreibt keine Zeile `tx`.
#[test]
fn a_recorded_tx_line_holds_until_the_next() {
    let p = overflowing("");
    let stimulus =
        "t=1 cmd go\nt=2 tx tx free=4 idle=false\nt=2 cmd go\nt=3 cmd go\nt=4 tx tx free=2 idle=false\nt=4 cmd go\n";
    let parsed = Trace::parse(stimulus).expect("Stimulus");
    assert_eq!(parsed.render(), stimulus, "Zeile `tx` hin und zurueck");
    let t = run(&p, &parsed, &RunOptions { ticks: 6, ..Default::default() }).expect("Lauf").trace.render();
    assert!(!t.contains("t=2 fault") && !t.contains("t=3 fault"), "{t}");
    assert!(t.contains("t=4 fault dut StreamOverflow \"Sendepuffer `tx` hat 2 Byte frei, 3 verlangt\" -> SAFE"), "{t}");
    assert!(!t.contains(" tx tx "), "{t}");
}

/// Pruefung 20, 8.3: Speist ein Ausgabestrom einen Eingabestrom aus Text,
/// ist jedes `send` ein Element und muss mit seiner statischen Hoechstlaenge
/// in dessen Kapazitaet passen; sonst saehe der Leser ein Element, das sein
/// Typ nicht fasst.
#[test]
fn a_send_must_fit_the_element_it_feeds() {
    let src = |cap: u32| {
        format!(
            "system:\n    language = 1\n    tick = 10 ms\n\n\
             input  rx     : stream<line<8>> @ hw(\"uart0/rx\") with max_rate = 100 Hz, framing = lines\n\
             output rx_sim : stream<line<{cap}>> @ sim(\"uart0/rx\") with capacity = 64\n\
             output n      : int in 0..99 @ hw(\"o/n\") with safe = 0\n\n\
             machine model:\n    initial RUN\n    state RUN:\n        enter:\n            send rx_sim, \"ok\"\n\n\
             machine reader:\n    initial RUN\n    state RUN:\n        on rx as e:\n            n = e.text.len\n"
        )
    };
    if let Err(e) = compile(&src(8)) {
        panic!("{e:?}");
    }
    let errors = compile(&src(16)).expect_err("zu lang");
    assert!(errors.iter().any(|e| e.contains("SC-20") && e.contains("fasst 8 Byte je Element")), "{errors:?}");
}
