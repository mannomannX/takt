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
    let options = RunOptions { ticks: 10, profile: None, order_seed: None, ..Default::default() };
    let erst = run(&p, &stimulus, &options).expect("Lauf");

    let recording = Recording { header: Header::of(&p, None, &[], 10), inputs: stimulus };
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
        header: Header::of(&p, Some("QUAL"), &[], 42),
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
    let recording = Recording { header: Header::of(&a, None, &[], 5), inputs: Trace::default() };
    assert!(recording.matches(&a).is_ok(), "das eigene Programm passt");
    let err = recording.matches(&b).expect_err("ein anderes Programm nicht");
    assert!(err.contains("anderen Programm"), "{err}");
}

/// 11.3: Leser akzeptieren aeltere Versionen ihres Formats, nicht neuere.
#[test]
fn a_newer_recording_is_refused() {
    let p = program(QUELLE);
    let recording = Recording { header: Header::of(&p, None, &[], 1), inputs: Trace::default() };
    let text = recording
        .render()
        .replace(&format!("takt-aufzeichnung {}", takt_interp::record::RECORDING_VERSION), "takt-aufzeichnung 99");
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
    let text = Header::of(&p, Some("QUAL"), &[("LIMIT".into(), "5".into())], 3).render();
    // Jede Kopfzeile ist eine, die `Header::render` kennt; keine traegt
    // eine Uhrzeit oder einen Ort.
    const KEYS: [&str; 14] = [
        "takt-aufzeichnung",
        "edition",
        "logik",
        "tick",
        "ticks",
        "profil",
        "target",
        "param",
        "runtime",
        "native",
        "tcb",
        "irreversibel",
        "maschine",
        "kette",
    ];
    for line in text.lines() {
        let key = line.strip_prefix("#! ").and_then(|l| l.split_whitespace().next());
        assert!(key.is_some_and(|k| KEYS.contains(&k)), "unbekannte Kopfzeile `{line}`:\n{text}");
    }
    assert!(!text.contains(":\\") && !text.contains('/'), "ein Pfad: {text}");
    // Zweimal derselbe Kopf.
    assert_eq!(text, Header::of(&p, Some("QUAL"), &[("LIMIT".into(), "5".into())], 3).render());
}

/// 12.5 und 4.5: Der Kopf nennt die nativen Funktionen, „damit die
/// erweiterte TCB sichtbar bleibt".
#[test]
fn the_header_names_the_native_functions() {
    let source = "system:\n    language = 1\n    tick     = 10 ms\n\n\
         native fn crc32(b: bytes<8>) -> u32 with cost = 200, stack = 32, total\n\n\
         output r : u32 @ hw(\"ui/r\") with safe = 0\n\n\
         machine m:\n    var b : bytes<8> = default\n\n    initial RUN\n\
         \x20   state RUN:\n        loop:\n            r = crc32(b)\n";
    let p = program(source);
    let text = Header::of(&p, None, &[], 1).render();
    assert!(text.contains("#! native crc32"), "die native Funktion fehlt im Kopf:\n{text}");
}

/// Die Zahl der Ticks gehoert in den Kopf, nicht in die Zeilen: Ein Lauf
/// kann laenger sein als seine letzte Eingabe.
#[test]
fn the_header_carries_the_tick_count() {
    let p = program(QUELLE);
    let recording =
        Recording { header: Header::of(&p, None, &[], 100), inputs: Trace::parse("t=1 cmd go\n").expect("Stimulus") };
    let gelesen = Recording::parse(&recording.render()).expect("lesbar");
    assert_eq!(gelesen.header.ticks, 100, "die Laenge des Laufs steht im Kopf");
}

/// Ein Programm mit Parameter, Profil und `persist`-Variable.
const SETTINGS: &str = "system:\n    language = 1\n    tick     = 10 ms\n\n\
     param STEP : int in 1..9 = 1\n\n\
     profile FAST:\n    STEP = 3\n\n\
     output n : int in 0..999 @ hw(\"o/n\") with safe = 0\n\n\
     machine count:\n    persist var start : int in 0..99 = 0\n    var k : int in 0..999 = 0\n\n    initial RUN\n\
     \x20   state RUN:\n        loop:\n            k = min(k + STEP, 999)\n            n = start * 100 + k\n";

/// Wie `takt replay` (takt-cli `replay`) aus der Aufzeichnung allein laeuft.
fn replay(p: &takt_mir::Program, text: &str) -> String {
    let rec = Recording::parse(text).expect("lesbar");
    rec.matches(p).expect("dasselbe Programm");
    let options = RunOptions {
        ticks: rec.header.ticks,
        profile: rec.header.profile.clone(),
        overrides: rec.header.overrides(),
        nvm: rec.header.store(p).expect("Speicher des Kopfs"),
        ..Default::default()
    };
    run(p, &rec.inputs, &options).expect("Wiedergabe").trace.render()
}

/// 12.5: Die Wiedergabe wendet Profil und Parametervektor des Kopfs an —
/// ohne sie liefe ein anderer Lauf.
#[test]
fn replay_applies_the_profile_and_parameters_of_the_header() {
    let p = program(SETTINGS);
    let options = RunOptions {
        ticks: 4,
        profile: Some("FAST".into()),
        overrides: vec![("STEP".into(), "5".into())],
        ..Default::default()
    };
    let first = run(&p, &Trace::default(), &options).expect("Lauf");
    let text =
        Recording { header: Header::of(&p, Some("FAST"), &first.start_params, 4), inputs: Trace::default() }.render();
    assert!(text.contains("#! profil FAST\n") && text.contains("#! param STEP 5\n"), "{text}");
    let want = first.trace.render();
    assert!(want.contains("t=1 out n 10"), "{want}");
    assert_eq!(replay(&p, &text), want);
    let plain = run(&p, &Trace::default(), &RunOptions { ticks: 4, ..Default::default() }).expect("Lauf");
    assert_ne!(plain.trace.render(), want, "ohne Kopf ein anderer Lauf");
}

/// 12.5, 5.9: Ein Lauf zeichnet s0 der `persist`-Variablen auf; die
/// Wiedergabe aus der Aufzeichnung allein reproduziert einen Lauf, der mit
/// gefuelltem Speicher begann.
#[test]
fn replay_reproduces_a_run_that_started_from_a_filled_store() {
    let p = program(SETTINGS);
    let key = p.machines.iter().flat_map(|m| &m.persist).map(|pv| pv.type_hash).next().expect("persist");
    let mut nvm = takt_interp::nvm::Nvm::new();
    nvm.put(key, takt_interp::Value::Int(7));
    let first =
        run(&p, &Trace::default(), &RunOptions { ticks: 3, nvm: nvm.clone(), ..Default::default() }).expect("Lauf");
    let want = first.trace.render();
    assert!(want.contains("t=0 out n 701"), "{want}");
    // `Header::of` kennt nur das Programm; den Speicher traegt der Aufrufer ein.
    let header = Header::of(&p, None, &first.start_params, 3).with_store(&p, &nvm);
    let text = Recording { header, inputs: Trace::default() }.render();
    assert_eq!(replay(&p, &text), want, "die Aufzeichnung traegt s0 nicht:\n{text}");
}

/// 12.5: Verglichen wird die Logik, nicht die Bindung — dieselbe Logik an
/// einer anderen Adresse passt zur Aufzeichnung (8.3).
#[test]
fn the_same_logic_with_another_binding_matches() {
    let a = program(QUELLE);
    let b = program(&QUELLE.replace("hw(\"ui/led\")", "hw(\"panel/lamp\")"));
    let recording = Recording { header: Header::of(&a, None, &[], 5), inputs: Trace::default() };
    assert!(recording.matches(&b).is_ok(), "{:?}", recording.matches(&b));
}

/// 11.3: Eine aeltere Formatversion wird gelesen; Version 1 nannte die
/// Defaults, die ohnehin gelten, und ueberlagert darum nichts. Eine
/// abgeschnittene Aufzeichnung ohne Pflichtzeile wird abgelehnt.
#[test]
fn an_older_version_reads_and_a_truncated_one_is_refused() {
    let p = program(SETTINGS);
    let text = Header::of(&p, None, &[("STEP".into(), "4".into())], 3).render();
    let old = text
        .replace(&format!("#! takt-aufzeichnung {}", takt_interp::record::RECORDING_VERSION), "#! takt-aufzeichnung 1");
    let read = Header::parse(&old).expect("Version 1");
    assert_eq!((read.version, read.overrides()), (1, Vec::new()));
    assert_eq!(Header::parse(&text).expect("aktuell").overrides(), [("STEP".to_string(), "4".to_string())]);

    let cut = &text[..text.find("#! tick ").expect("tick")];
    let e = Header::parse(cut).expect_err("abgeschnitten");
    assert_eq!(e, "Kopfzeile `#! tick` fehlt");
}
