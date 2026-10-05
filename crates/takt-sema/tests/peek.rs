//! `s.peek() -> E?` (8.6, FB-15): das naechste Element, untersucht, nicht
//! konsumiert — ein Handler desselben Ticks sieht es noch, im naechsten Tick
//! ist es fort.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn peek_shows_the_element_the_handler_still_gets() {
    let p = compile(
        "input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz, framing = lines, overflow = fault
output peeked : bool @ hw(\"o/peeked\") with safe = false
output seen : int in 0..100 @ hw(\"o/seen\") with safe = 0
machine m:
    var n : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            var next : line<16>? = rx.peek()
            peeked = next.valid
        on rx as e:
            n = n + 1
            seen = n
",
    );
    let t = trace(&p, "t=1 in rx \"a\"\n", 3);
    assert!(t.contains("t=1 out peeked true"), "{t}");
    assert!(t.contains("t=1 out seen 1"), "{t}");
    assert!(t.contains("t=2 out peeked false"), "{t}");
}

#[test]
fn peek_alone_consumes_at_the_end_of_the_tick() {
    // Lemma 9.6.1: Wer nur peekt, laesst das Fenster nicht wachsen. Zwei
    // Elemente in einem Tick verlangen `MAXPT >= 2` (12.6, Zeile 2).
    let p = compile(
        "input rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 200 Hz, framing = lines, overflow = fault
output pending : int in 0..100 @ hw(\"o/pending\") with safe = 0
machine m:
    var next : line<16>? = none
    initial RUN
    state RUN:
        loop:
            next = rx.peek()
            pending = rx.count
",
    );
    let t = trace(&p, "t=1 in rx \"a\"\nt=1 in rx \"b\"\n", 4);
    assert!(t.contains("t=1 out pending 2"), "{t}");
    assert!(t.contains("t=2 out pending 1"), "{t}");
    assert!(t.contains("t=3 out pending 0"), "{t}");
}

const BYTES: &str = "input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 200 Hz, capacity = 4, overflow = fault\n";

/// Der Inhalt des untersuchten Elements, zweimal im selben Tick gelesen:
/// `peek` veraendert nichts, beide sehen dasselbe Element.
#[test]
fn peek_twice_in_a_tick_shows_the_same_element() {
    let p = compile(&format!(
        "{BYTES}output a : u8 @ hw(\"o/a\") with safe = 0
output b : u8 @ hw(\"o/b\") with safe = 0
machine m:
    initial RUN
    state RUN:
        loop:
            a = rx.peek().or(0)
            b = rx.peek().or(0)
"
    ));
    let t = trace(&p, "t=1 in rx 7\nt=1 in rx 8\n", 3);
    for line in ["t=1 out a 7", "t=1 out b 7", "t=2 out a 8", "t=2 out b 8", "t=3 out a 0"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

/// 9.6: Jeder Leser hat seinen Cursor; der Puffer verdraengt erst, was der
/// langsamste gelesen hat. Ein Handler, der alles nimmt, nimmt dem, der nur
/// peekt, nichts weg.
#[test]
fn a_second_reader_does_not_take_what_the_peeker_has_not_seen() {
    let p = compile(&format!(
        "{BYTES}output a : u8 @ hw(\"o/a\") with safe = 0
output total : int in 0..99 @ hw(\"o/total\") with safe = 0
machine peeker:
    initial RUN
    state RUN:
        loop:
            a = rx.peek().or(0)
machine eater:
    var n : int in 0..99 = 0
    initial RUN
    state RUN:
        on rx as e:
            n = min(n + 1, 99)
            total = n
"
    ));
    let t = trace(&p, "t=1 in rx 7\nt=1 in rx 8\n", 4);
    for line in ["t=1 out a 7", "t=1 out total 2", "t=2 out a 8", "t=3 out a 0"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}

/// Lemma 9.6.1: Wer nur peekt, verbraucht ein Element je Tick. Kommen
/// zwei je Tick in einen Puffer fuer vier, waechst sein Fenster, bis es
/// ueberlaeuft — `overflow = fault` meldet `StreamOverflow` dem Leser.
#[test]
fn a_peek_only_reader_overflows_when_elements_come_faster() {
    let p = compile(&format!(
        "{BYTES}output a : u8 @ hw(\"o/a\") with safe = 0
machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            a = rx.peek().or(0)
    state SAFE:
        when false: -> RUN
"
    ));
    let stimulus: String = (1..=4).map(|t| format!("t={t} in rx {t}\nt={t} in rx {}\n", t + 10)).collect();
    let t = trace(&p, &stimulus, 5);
    // Fenster am Tickende 1, 2, 3; in Tick 4 kaemen fuenf in vier Plaetze.
    for line in ["t=3 out a 2", "t=4 fault m StreamOverflow \"Stream `rx` uebergelaufen\" -> SAFE"] {
        assert!(t.contains(line), "`{line}` fehlt:\n{t}");
    }
}
