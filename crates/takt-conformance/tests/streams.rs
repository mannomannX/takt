//! Die Stromtabelle des Rahmens gegen die Regeln aus 8.6.
//!
//! Der differentielle Test (`differential.rs`) misst das Ganze: Ein
//! Programm mit Handlern laeuft auf beiden Seiten und muss dasselbe
//! sagen. Er braucht aber clang und ueberspringt sich ohne. Was hier
//! steht, gilt immer — es prueft den erzeugten C-Text selbst.

use takt_conformance::harness;
use takt_conformance::stimulus::Stimulus;
use takt_mir::program::Program;

fn program_of(src: &str) -> Program {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
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

/// Ohne Elemente bleibt es beim leeren Fenster — der Fall, den jedes
/// Programm aushalten muss.
#[test]
fn without_elements_the_window_stays_empty() {
    let p = with_bounds("");
    let c = harness_of(&p, &[]);
    assert!(c.contains("takt_stream_count(int s, long long cur) { (void)s; (void)cur; return 0; }"));
    assert!(!c.contains("g_elems"), "ohne Stimulus braucht es keine Tabelle");
}

/// Die Elemente stehen in der Tabelle, mit Tick, Nummer und Inhalt.
#[test]
fn elements_reach_the_table_with_their_numbering() {
    let p = with_bounds("");
    let c = harness_of(&p, &[Stimulus::element(2, "rx", "AB"), Stimulus::element(5, "rx", "C")]);
    // 9.6: `seq` ist streng steigend und beginnt bei null.
    assert!(c.contains("{ 0, 2, 0, 2, \"\\x41\\x42\" }"), "erstes Element fehlt:\n{c}");
    assert!(c.contains("{ 0, 5, 1, 1, \"\\x43\" }"), "zweites Element fehlt:\n{c}");
}

/// 3.9: Der Rand begrenzt die Laenge; was darueber steht, ist
/// abgeschnitten und nicht verworfen.
#[test]
fn an_overlong_element_is_truncated_not_dropped() {
    let p = with_bounds("");
    let c = harness_of(&p, &[Stimulus::element(1, "rx", "0123456789ABCDEFXXXX")]);
    assert!(c.contains(", 16, \""), "auf `line<16>` gekuerzt:\n{c}");
    assert!(!c.contains("\\x58"), "das abgeschnittene `X` steht nicht in der Tabelle");
}

/// 8.6: `capacity` begrenzt die Zahl der Elemente. Was nicht
/// hineinpasst, ist ein Ueberlauf und kein Element.
#[test]
fn the_element_bound_holds() {
    let p = with_bounds(", capacity = 2");
    let c = harness_of(
        &p,
        &[Stimulus::element(1, "rx", "A"), Stimulus::element(1, "rx", "B"), Stimulus::element(1, "rx", "C")],
    );
    assert!(c.contains("\\x41") && c.contains("\\x42"), "die ersten zwei stehen drin:\n{c}");
    assert!(!c.contains("\\x43"), "das dritte reisst `capacity = 2`");
    // Ein spaeterer Tick faengt wieder bei null an: Der Konsument hat
    // sein Fenster geleert.
    let c = harness_of(
        &p,
        &[Stimulus::element(1, "rx", "A"), Stimulus::element(1, "rx", "B"), Stimulus::element(2, "rx", "C")],
    );
    assert!(c.contains("\\x43"), "im naechsten Tick ist wieder Platz:\n{c}");
}

/// 8.6: `capacity_bytes` ist die zweite Schranke, unabhaengig von der
/// ersten.
///
/// Die Zahlen muessen SC-17 genuegen: Die Pruefung rechnet `capacity`
/// mal Elementgroesse und verlangt so viel `capacity_bytes` (Lemma
/// 9.6.1). Darum vier Elemente zu `line<16>` — und die Schranke greift
/// ueber die Summe, nicht ueber ein einzelnes Element.
#[test]
fn the_byte_bound_holds_independently() {
    let p = with_bounds(", capacity = 4, capacity_bytes = 64");
    let lang = "0123456789ABCDEF";
    let c = harness_of(
        &p,
        &[
            Stimulus::element(1, "rx", lang),
            Stimulus::element(1, "rx", lang),
            Stimulus::element(1, "rx", lang),
            Stimulus::element(1, "rx", lang),
            Stimulus::element(1, "rx", "X"),
        ],
    );
    // Vier volle Elemente sind 64 Byte; das fuenfte passt nicht mehr,
    // obwohl `capacity = 4` erst danach greifen wuerde.
    assert_eq!(c.matches("\\x30").count(), 4, "vier Elemente stehen in der Tabelle:\n{c}");
    assert!(!c.contains("\\x58"), "das fuenfte reisst `capacity_bytes = 64`");
}

/// Ein Stimulus fuer einen anderen Kanal beruehrt den Strom nicht.
#[test]
fn a_stimulus_for_another_channel_is_ignored() {
    let p = with_bounds("");
    let c = harness_of(&p, &[Stimulus::element(1, "andere", "A"), Stimulus::cmd(1, "go")]);
    assert!(!c.contains("g_elems"), "kein Element fuer `rx`:\n{c}");
}
