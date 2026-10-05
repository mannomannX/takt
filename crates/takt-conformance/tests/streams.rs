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
    let end = c[start..].find("takt_edge_commit(a, tick);").map_or(c.len(), |e| start + e);
    &c[start..end]
}

/// Ohne Stimulus gibt es keine Lieferung; das Fenster fragt nur den Ring.
#[test]
fn without_elements_the_window_stays_empty() {
    let p = with_bounds("");
    let c = harness_of(&p, &[]);
    assert!(c.contains("return k >= 0 ? takt_int_count(a, k, cur) : 0;"), "{c}");
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
    let c = harness_of(&p, &[Stimulus::element(1, "rx", "0123456789ABCDEFXXXX")]);
    let f = feed(&c);
    // Wie auf dem Board kommt die ganze Zeile an den Rand; er kuerzt auf
    // `line<16>` und merkt es fuer `.truncated` (KON2-028).
    assert!(f.contains("\\x58\\x58\\x58\\x58\", 20, 10000000LL, "), "ungekuerzt an den Rand:\n{f}");
    assert!(c.contains("_Bool cut = v->len > 16;"), "der Rand kuerzt auf `line<16>`");
}

/// Ein Programm mit einem Textstrom `line<16>`, dessen Handler `body`
/// rechnet; die Ausgaenge `len`, `same` und `cut`.
fn line_program(body: &str) -> Program {
    program_of(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         input  rx   : stream<line<16>> @ hw(\"u/rx\") with max_rate = 100 Hz\n\
         output len  : int in 0..99     @ sim(\"o/len\")\n\
         output same : bool             @ sim(\"o/same\")\n\
         output cut  : bool             @ sim(\"o/cut\")\n\n\
         machine m:\n    initial A\n\n    state A:\n        on rx as l:\n{body}"
    ))
}

/// Laeuft `p` mit dem Element von 20 Zeichen in Tick 1 und einem kurzen in
/// Tick 3 in beiden Implementierungen; liefert den Trace des Interpreters.
fn overlong_run(p: &Program, name: &str) -> Option<String> {
    let clang = common::clang()?;
    let stimulus =
        takt_interp::Trace::parse("t=1 in rx \"0123456789ABCDEFXXXX\"\nt=3 in rx \"abc\"\n").expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    let native = common::run_native_all_with(&clang, p, name, 4, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 4, ..Default::default() };
    let interpreted = takt_interp::run(p, &stimulus, &options).expect("Lauf").trace.render();
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    Some(interpreted)
}

/// 3.9 im Lauf (KON2-028): Ein Element mit 20 Zeichen auf `line<16>` kommt
/// gekuerzt an, nicht verworfen — der Handler laeuft und sieht die ersten 16
/// Zeichen —, eines, das passt, ganz. In beiden Implementierungen gleich.
#[test]
fn an_overlong_element_arrives_truncated_like_in_the_interpreter() {
    let p = line_program("            len = l.text.len\n            same = l.text == \"0123456789ABCDEF\"\n");
    let Some(interpreted) = overlong_run(&p, "ueberlang") else { return };
    for line in ["t=1 out len 16", "t=1 out same true", "t=3 out len 3", "t=3 out same false"] {
        assert!(interpreted.contains(line), "`{line}` fehlt:\n{interpreted}");
    }
}

/// Der Handler erfaehrt, dass gekuerzt wurde (3.9: `line<N>` traegt
/// `.truncated`): wahr fuer das Element von 20 Zeichen, falsch fuer das
/// kurze — in beiden Implementierungen.
#[test]
fn the_truncation_of_a_line_element_reaches_the_handler() {
    let p = line_program("            len = l.text.len\n            cut = l.text.truncated\n");
    let Some(interpreted) = overlong_run(&p, "ueberlang_markiert") else { return };
    for line in ["t=1 out cut true", "t=3 out cut false"] {
        assert!(interpreted.contains(line), "`{line}` fehlt:\n{interpreted}");
    }
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
    let Some(clang) = common::clang() else { return };
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
        let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
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
    let Some(clang) = common::clang() else { return };
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
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
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

/// 8.6, 9.6, FB-426: `s.count` ohne Handler zaehlt den Puffer ab seinem
/// Anfang, ohne zu konsumieren — der Leser hat keinen Cursor. Ein Strom ohne
/// Konsumenten behaelt nichts, was in einem Tick sichtbar war: Was in Tick k
/// gesendet wird, sieht der Zaehler in Tick k + 1 und danach nicht mehr.
/// Nativ fehlte der Cursor-Platz, und die Maschine fiel aus; der Interpreter
/// raeumte einen Strom ohne Konsumenten nie, der Rahmen zu frueh.
#[test]
fn count_without_a_handler_reads_the_whole_buffer() {
    let Some(clang) = common::clang() else { return };
    let p = program_of(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         stream<u8> q with capacity = 4\n\n\
         output pending : int in 0..9 @ sim(\"o/pending\")\n\n\
         machine producer:\n    var k : int in 0..9 = 0\n    initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           if k < 2:\n                send q, 1\n\
         \x20           k = min(k + 1, 9)\n\n\
         machine m:\n    initial WAIT\n\n    state WAIT:\n        loop:\n            pending = q.count\n",
    );
    let native = common::run_native_all(&clang, &p, "zaehlen", 4).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 4, ..Default::default() };
    let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    for line in ["t=1 out pending 1", "t=3 out pending 0"] {
        assert!(interpreted.contains(line), "`{line}` fehlt:\n{interpreted}");
    }
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **Im Modus ENTRY ist das Fenster leer, nativ wie im Interpreter** (9.6,
/// 9.3; SEM1-049): `for x in rx` laeuft in `enter:`, im `loop:` des
/// Eintritts-Ticks und in `exit:`/`enter:` eines Wechsels nicht, und
/// `rx.count` ist dort 0. Die drei Elemente aus Tick 0 sieht darum erst
/// Tick 1, zusammen mit den zwei neuen; der Wechsel in Tick 1 laeuft wieder
/// im Modus ENTRY, und das Element aus Tick 2 sieht niemand mehr.
#[test]
fn the_window_is_empty_in_entry_mode_on_both_sides() {
    let Some(clang) = common::clang() else { return };
    let p = program_of(
        "system:\n    language = 1\n    tick     = 1 ms\n\n\
         input  rx : stream<u8> @ hw(\"u/rx\") with max_rate = 200 kHz, capacity = 256\n\
         output n  : int in 0..99 @ hw(\"o/n\") with safe = 0\n\
         output k  : int in 0..99 @ hw(\"o/k\") with safe = 0\n\
         output e  : int in 0..99 @ hw(\"o/e\") with safe = 0\n\n\
         machine m:\n    var seen : int in 0..99 = 0\n    initial RUN\n\n    state RUN:\n\
         \x20       enter:\n            for x in rx:\n                seen = (seen + 1) % 99\n\
         \x20           n = min(rx.count, 98) + 1\n\
         \x20       loop:\n            for x in rx:\n                seen = (seen + 1) % 99\n\
         \x20           k = seen + 1\n\
         \x20       when seen >= 5: -> NEXT\n\
         \x20       exit:\n            for x in rx:\n                seen = (seen + 1) % 99\n\n\
         \x20   state NEXT:\n        enter:\n            e = min(rx.count, 98) + 1\n\
         \x20           for x in rx:\n                seen = (seen + 1) % 99\n\
         \x20       loop:\n            k = seen + 1\n",
    );
    let stimulus =
        takt_interp::Trace::parse("t=0 in rx 1\nt=0 in rx 2\nt=0 in rx 3\nt=1 in rx 4\nt=1 in rx 5\nt=2 in rx 6\n")
            .expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    let native = common::run_native_all_with(&clang, &p, "entry_window", 4, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 4, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
    for line in ["t=0 out n 1", "t=0 out k 1", "t=1 out k 6", "t=1 out e 1"] {
        assert!(interpreted.contains(line), "Interpreter ohne `{line}`:\n{interpreted}");
        assert!(native.contains(line), "nativ ohne `{line}`:\n{native}");
    }
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}
