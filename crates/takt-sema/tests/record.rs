//! Aufzeichnung und Wiedergabe (12.5, 11.3).
//!
//! Die Zusage aus 12.5 lautet: „`takt replay` fuehrt dasselbe Binaer im
//! Sim-Modus mit den aufgezeichneten Inputs aus und vergleicht Outputs
//! und Zustandspfade; Abweichung = Fehler in Runtime oder Treiber, nie in
//! der Logik." Diese Tests pruefen beide Haelften: dass die Wiedergabe
//! reproduziert, und dass sie ablehnt, wo der Satz nicht gilt.

use takt_interp::record::{Header, Recording};
use takt_interp::{RunOptions, Trace, run};

fn program(src: &str) -> takt_mir::Program {
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &o);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

const QUELLE: &str = "system:\n    language = 1\n    tick     = 10 ms\n\n\
     command go\n\n\
     output led : bool @ hw(\"ui/led\") with safe = false\n\n\
     machine blink:\n    var n : int in 0..100 = 0\n\n    initial OFF\n\
     \x20   state OFF:\n        loop:\n            led = false\n        when go: -> ON\n\
     \x20   state ON:\n        loop:\n            n = n + 1\n            led = true\n";

/// 12.5: Die Wiedergabe reproduziert den Lauf.
#[test]
fn replaying_a_recording_reproduces_the_run() {
    let p = program(QUELLE);
    let stimulus = Trace::parse("t=3 cmd go\n").expect("Stimulus");
    let options = RunOptions { ticks: 10, profile: None, order_seed: None };
    let erst = run(&p, &stimulus, &options).expect("Lauf");

    let recording = Recording { header: Header::of(&p, None, 10), inputs: stimulus };
    let text = recording.render();
    let gelesen = Recording::parse(&text).expect("lesbar");
    let zweit = run(&p, &gelesen.inputs, &options).expect("Wiedergabe");

    assert_eq!(erst.trace.render(), zweit.trace.render(), "die Wiedergabe weicht ab");
}

/// 11.3: Das Format ist versioniert, und der Kopf ueberlebt das Schreiben
/// und Lesen unveraendert.
#[test]
fn a_recording_survives_a_round_trip() {
    let p = program(QUELLE);
    let recording = Recording {
        header: Header::of(&p, Some("QUAL"), 42),
        inputs: Trace::parse("t=1 cmd go\nt=7 cmd go\n").expect("Stimulus"),
    };
    let text = recording.render();
    let gelesen = Recording::parse(&text).expect("lesbar");
    assert_eq!(gelesen.header, recording.header);
    assert_eq!(gelesen.render(), text, "nicht zeichengleich");
}

/// 12.5: Eine Aufzeichnung zu einem anderen Programm wird abgelehnt.
///
/// Der Satz „Abweichung = Fehler in Runtime oder Treiber, nie in der
/// Logik" gilt nur, wenn die Logik dieselbe ist. Sie zu vergleichen
/// ergaebe Abweichungen, die nichts bedeuten.
#[test]
fn a_recording_of_another_program_is_rejected() {
    let a = program(QUELLE);
    let b = program(&QUELLE.replace("n + 1", "n + 2"));
    let recording = Recording { header: Header::of(&a, None, 5), inputs: Trace::default() };
    assert!(recording.matches(&a).is_ok(), "das eigene Programm passt");
    let err = recording.matches(&b).expect_err("ein anderes Programm nicht");
    assert!(err.contains("anderen Programm"), "{err}");
}

/// 11.3: Leser akzeptieren aeltere Versionen ihres Formats, nicht neuere.
#[test]
fn a_newer_recording_is_refused() {
    let p = program(QUELLE);
    let recording = Recording { header: Header::of(&p, None, 1), inputs: Trace::default() };
    let text = recording.render().replace("takt-aufzeichnung 1", "takt-aufzeichnung 99");
    let err = Recording::parse(&text).expect_err("eine neuere Version");
    assert!(err.contains("neuer"), "{err}");
}

/// 11.3: reproduzierbare Builds — kein Zeitstempel, kein Pfad im Kopf.
///
/// Eine Aufzeichnung, die sich bei jedem Lauf unterscheidet, waere
/// schlecht zu vergleichen; *wo* etwas lief, gehoert nicht zur Semantik.
#[test]
fn the_header_carries_no_timestamp_and_no_path() {
    let p = program(QUELLE);
    let text = Header::of(&p, None, 3).render();
    assert!(!text.contains("2026"), "ein Zeitstempel: {text}");
    assert!(!text.contains(":\\") && !text.contains('/'), "ein Pfad: {text}");
    // Zweimal derselbe Kopf.
    assert_eq!(text, Header::of(&p, None, 3).render());
}

/// 12.5 und 4.5: Der Kopf nennt die nativen Funktionen, „damit die
/// erweiterte TCB sichtbar bleibt".
#[test]
fn the_header_names_the_native_functions() {
    let quelle = "system:\n    language = 1\n    tick     = 10 ms\n\n\
         native fn crc32(b: bytes<8>) -> u32 with cost = 200, stack = 16, total\n\n\
         output r : u32 @ hw(\"ui/r\") with safe = 0\n\n\
         machine m:\n    var b : bytes<8> = default\n\n    initial RUN\n\
         \x20   state RUN:\n        loop:\n            r = crc32(b)\n";
    let p = program(quelle);
    let text = Header::of(&p, None, 1).render();
    assert!(text.contains("#! native crc32"), "die native Funktion fehlt im Kopf:\n{text}");
}

/// Die Zahl der Ticks gehoert in den Kopf, nicht in die Zeilen: Ein Lauf
/// kann laenger sein als seine letzte Eingabe.
#[test]
fn the_header_carries_the_tick_count() {
    let p = program(QUELLE);
    let recording =
        Recording { header: Header::of(&p, None, 100), inputs: Trace::parse("t=1 cmd go\n").expect("Stimulus") };
    let gelesen = Recording::parse(&recording.render()).expect("lesbar");
    assert_eq!(gelesen.header.ticks, 100, "die Laenge des Laufs steht im Kopf");
}
