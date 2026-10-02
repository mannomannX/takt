//! Die Eingabestroeme des Rahmens gegen die Regeln aus 8.6 und 12.6.
//!
//! Jeder Eingabestrom hat einen Ring; seine Elemente bringt der
//! Treiberrand, auf dem Wirt aus dem Stimulus. Die Textpruefungen gelten
//! immer — sie lesen den erzeugten C-Text. Der Lauf gegen den Interpreter
//! braucht clang und ueberspringt sich ohne.

mod common;

use takt_conformance::harness;
use takt_conformance::run::compare;
use takt_conformance::stimulus::Stimulus;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

fn program_of(src: &str) -> Program {
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

/// Ein Programm mit einem Eingabestrom und den angegebenen Schranken.
///
/// `max_rate` steht immer dabei: 8.6 verlangt es fuer jeden Strom
/// (SC-17), weil ohne obere Rate keine Schranke fuer das Fenster folgt.
fn with_bounds(attrs: &str) -> Program {
    program_of(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input  rx : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz{attrs}\n\
         output y  : int in 0..9      @ sim(\"o\")\n\n\
         machine m:\n    initial A\n\n    state A:\n        on rx as l:\n            y = 1\n"
    ))
}

fn harness_of(p: &Program, stimulus: &[Stimulus]) -> String {
    harness::build_with(p, "m", 10, stimulus).source
}

/// Der Teil des Rahmens, der den Stimulus an den Rand gibt.
fn feed(c: &str) -> &str {
    let start = c.find("static void takt_edge_stimulus").expect("Lieferfunktion");
    let end = c[start..].find("takt_edge_commit(tick);").map_or(c.len(), |e| start + e);
    &c[start..end]
}

/// Ohne Stimulus gibt es keine Lieferung; das Fenster fragt nur den Ring.
#[test]
fn without_elements_the_window_stays_empty() {
    let p = with_bounds("");
    let c = harness_of(&p, &[]);
    assert!(c.contains("return k >= 0 ? takt_int_count(k, cur) : 0;"), "{c}");
    assert!(!feed(&c).contains("takt_edge_element("), "keine Lieferung:\n{}", feed(&c));
}

/// Ein Element geht mit Zeitstempel und Folgenummer an den Rand: ohne
/// Angabe die Tickgrenze und die lueckenlose Folge ab null (12.6).
#[test]
fn elements_reach_the_edge_with_their_numbering() {
    let p = with_bounds("");
    let c = harness_of(&p, &[Stimulus::element(2, "rx", "AB"), Stimulus::element(5, "rx", "C")]);
    let f = feed(&c);
    assert!(f.contains("\"\\x41\\x42\", 2, 20000000LL, 0LL)"), "erstes Element fehlt:\n{f}");
    assert!(f.contains("\"\\x43\", 1, 50000000LL, 1LL)"), "zweites Element fehlt:\n{f}");
}

/// 3.9: Der Rand begrenzt die Laenge; was darueber steht, ist
/// abgeschnitten und nicht verworfen.
#[test]
fn an_overlong_element_is_truncated_not_dropped() {
    let p = with_bounds("");
    let f = harness_of(&p, &[Stimulus::element(1, "rx", "0123456789ABCDEFXXXX")]);
    let f = feed(&f);
    assert!(f.contains(", 16, 10000000LL, "), "auf `line<16>` gekuerzt:\n{f}");
    assert!(!f.contains("\\x58"), "das abgeschnittene `X` geht nicht an den Rand");
}

/// 8.6: Beide Schranken stehen am Ring, `capacity` und `capacity_bytes`;
/// was nicht hineinpasst, entscheidet sich im Lauf.
#[test]
fn the_bounds_reach_the_ring() {
    let p = with_bounds(", capacity = 4, capacity_bytes = 64");
    let c = harness_of(&p, &[]);
    assert!(c.contains("static const int g_int_cap[1] = { 4 };"), "{c}");
    assert!(c.contains("static const int g_int_capb[1] = { 64 };"), "{c}");
}

/// Ein Stimulus fuer einen anderen Kanal beruehrt den Strom nicht.
#[test]
fn a_stimulus_for_another_channel_is_ignored() {
    let p = with_bounds("");
    let c = harness_of(&p, &[Stimulus::element(1, "andere", "A"), Stimulus::cmd(1, "go")]);
    assert!(!feed(&c).contains("takt_edge_element("), "kein Element fuer `rx`:\n{}", feed(&c));
}

/// 8.6, 9.6: Ein Leser, der zurueckfaellt, laesst den Ring volllaufen —
/// mit `drop_oldest` verdraengt das neue Element das aelteste, sonst
/// faultet der Ueberlauf den Leser. Der Treiber haelt dabei `MAXPT` ein;
/// den Ueberlauf macht allein der Leser, der in `WAIT` nichts abholt.
#[test]
fn a_reader_that_falls_behind_overflows_the_ring() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    for policy in ["drop_oldest", "fault"] {
        let p = program_of(&format!(
            "system:\n    language = 1\n    tick     = 10 ms\n\n\
             input  rx      : stream<line<8>> @ hw(\"u/rx\") with max_rate = 200 Hz, capacity = 2, overflow = {policy}\n\
             output pending : int in 0..9     @ sim(\"o/pending\")\n\n\
             machine m:\n    initial WAIT\n\n    state WAIT:\n        loop:\n            pending = rx.count\n\n\
             \x20   state READ:\n        on rx as l:\n            pending = 0\n"
        ));
        let stimulus =
            takt_interp::Trace::parse("t=1 in rx \"a\"\nt=1 in rx \"b\"\nt=2 in rx \"c\"\nt=3 in rx \"d\"\n")
                .expect("Stimulus");
        let inputs = Stimulus::from_trace(&stimulus);
        let native = common::run_native_all_with(&clang, &p, &format!("ueberlauf-{policy}"), 5, &inputs)
            .unwrap_or_else(|e| panic!("{e}"));
        let options = takt_interp::RunOptions { ticks: 5, ..Default::default() };
        let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
        assert!(interpreted.contains("out pending 2"), "der Ring laeuft voll ({policy}):\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty(),
            "{policy}: {} Abweichungen\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
            diffs.len()
        );
        if policy == "fault" {
            assert!(native.contains("StreamOverflow"), "der Ueberlauf faultet den Leser:\n{native}");
        }
    }
}

/// **Die Zaehler eines Stroms sind die des Interpreters** (8.6, 12.6 Zeile
/// 5, FB-361): `drop_oldest` verdraengt (`dropped`), ein Record ohne
/// `decode` wird verworfen (`malformed`), ein voller Ring mit `fault`
/// laeuft ueber (`overflowed`) und faultet den Leser, der zurueckfaellt.
/// Eine zweite Maschine liest die Zaehler; beide Seiten schreiben dazu die
/// Zeilen `stream`, und der Vergleich haelt sie gegeneinander.
#[test]
fn the_counters_of_a_stream_are_the_interpreters() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = program_of(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         record Pair:\n    a : u8\n    b : u8\n\n\
         input  pairs : stream<Pair>    @ hw(\"c/rx\") with max_rate = 200 Hz, capacity = 2, overflow = drop_oldest\n\
         input  rx    : stream<line<8>> @ hw(\"u/rx\") with max_rate = 200 Hz, capacity = 2, overflow = fault\n\
         output lost  : int in 0..9     @ sim(\"o/lost\")\n\
         output over  : int in 0..9     @ sim(\"o/over\")\n\
         output bad   : int in 0..9     @ sim(\"o/bad\")\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           lost = min(pairs.dropped, 9)\n\
         \x20           over = min(rx.overflowed, 9)\n\
         \x20           bad = min(pairs.malformed, 9)\n\n\
         machine lag:\n    initial WAIT\n\n    state WAIT:\n        loop:\n            pass\n\n\
         \x20   state READ:\n        on rx as l:\n            pass\n        on pairs as q:\n            pass\n",
    );
    let stimulus = takt_interp::Trace::parse(
        "t=1 in pairs 0x0102\nt=1 in pairs 0x0304\nt=1 in rx \"a\"\nt=1 in rx \"b\"\n\
         t=2 in pairs 0x0506\nt=2 in rx \"c\"\nt=3 in pairs 0x010203\n",
    )
    .expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus);
    let native = common::run_native_all_with(&clang, &p, "zaehler", 5, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 5, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
    for line in [
        "t=2 out lost 1",
        "t=2 out over 1",
        "t=3 out bad 1",
        "t=2 stream pairs dropped=1 overflowed=0 malformed=0",
        "t=2 stream rx dropped=0 overflowed=1 malformed=0",
        "t=3 stream pairs dropped=1 overflowed=0 malformed=1",
    ] {
        assert!(interpreted.contains(line), "Interpreter ohne `{line}`:\n{interpreted}");
        assert!(native.contains(line), "nativ ohne `{line}`:\n{native}");
    }
    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        diffs.len()
    );
}
