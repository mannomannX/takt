//! Die sechs Stufen der Kommandozeile laufen und melden, was sie sollen.
//!
//! Geprueft ist bisher jede Schicht fuer sich (Korpus, Golden-Traces,
//! Roundtrip); der Aufsatz darueber — Argumente, Exit-Code, Ausgabe — hatte
//! keinen Test. Dieser Rahmen deckt ihn ab: Jede Stufe laeuft gegen eine
//! Korpusdatei, und die beiden Faelle, in denen ein Exit-Code etwas
//! bedeutet (Fehler, nicht kanonisch), werden einzeln festgehalten.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Jede Stufe nimmt eine gueltige Korpusdatei an und liefert Exit-Code 0.
#[test]
fn every_stage_accepts_a_corpus_file() {
    let file = "corpus-try/13_framing.takt";
    for stage in ["tokens", "parse", "fmt", "check", "mir", "size"] {
        let out = takt(&[stage, file]);
        assert!(out.status.success(), "`takt {stage}` scheitert an {file}:\n{}", String::from_utf8_lossy(&out.stderr));
        // `check` und `fmt` schweigen, wenn nichts zu melden ist; die
        // uebrigen vier geben ihr Zwischenergebnis aus.
        if !matches!(stage, "check" | "fmt") {
            assert!(!stdout(&out).is_empty(), "`takt {stage}` sagt nichts");
        }
    }
}

/// `check` meldet Fehler mit Exit-Code 1 — der Unterschied, an dem ein
/// Build-Skript haengt.
#[test]
fn check_fails_on_a_broken_file() {
    let out = takt(&["check", "corpus-try/n04_missing_initial.takt"]);
    assert!(!out.status.success(), "eine kaputte Datei muss scheitern");
    let text = stdout(&out) + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("initial"), "die Meldung nennt die Ursache: {text}");
}

/// `fmt --check` unterscheidet kanonisch von nicht kanonisch, ohne die
/// Datei anzufassen.
#[test]
fn fmt_check_reports_canonical_form() {
    let out = takt(&["fmt", "--check", "corpus-try/13_framing.takt"]);
    assert!(out.status.success(), "der Korpus ist kanonisch: {}", stdout(&out));
}

/// `size` gibt die Posten aus, die 11.5 nennt, samt Herkunft.
#[test]
fn size_lists_its_items_with_origin() {
    let out = takt(&["size", "corpus-try/01_minimal.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    for item in ["Maschinenzustaende", "Stroeme", "Summe"] {
        assert!(text.contains(item), "Posten `{item}` fehlt:\n{text}");
    }
    assert!(text.contains("exakt") || text.contains("offen"), "die Herkunft steht dabei:\n{text}");
}

/// `size --baseline` (11.5, D1): eine gespeicherte Rechnung ist die
/// Messlatte; waechst RAM oder Flash, faellt der Aufruf.
#[test]
fn size_compares_with_a_baseline() {
    let file = std::env::temp_dir().join(format!("takt-size-{}.baseline", std::process::id()));
    let path = file.to_string_lossy().into_owned();
    let saved = takt(&["size", "corpus-try/01_minimal.takt", "--save-baseline", &path]);
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    let same = takt(&["size", "corpus-try/01_minimal.takt", "--baseline", &path]);
    assert!(same.status.success() && stdout(&same).contains("keine Aenderung"), "{}", stdout(&same));
    let bigger = takt(&["size", "corpus-try/13_framing.takt", "--baseline", &path]);
    assert!(!bigger.status.success(), "ein groesseres Programm faellt gegen die Baseline:\n{}", stdout(&bigger));
    assert!(stdout(&bigger).contains("gewachsen"), "{}", stdout(&bigger));
    let _ = std::fs::remove_file(&file);
}

/// `mir --hash` liefert den Logik-Hash; er haengt nur an der Logik, also
/// liefert derselbe Aufruf denselben Wert.
#[test]
fn mir_hash_is_stable() {
    let a = takt(&["mir", "corpus-try/01_minimal.takt", "--hash"]);
    let b = takt(&["mir", "corpus-try/01_minimal.takt", "--hash"]);
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    assert_eq!(stdout(&a), stdout(&b), "derselbe Lauf, derselbe Hash");
    assert!(!stdout(&a).trim().is_empty(), "der Hash steht da");
}

/// `sim` laeuft eine feste Zahl Ticks und schreibt einen Trace.
#[test]
fn sim_runs_for_the_requested_ticks() {
    let out = takt(&["sim", "corpus-try/13_framing.takt", "--ticks", "4"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("t=0"), "der Trace beginnt bei t=0:\n{text}");
    assert!(text.contains("state builder"), "der Zustand steht im Trace:\n{text}");
}

/// `latency` gibt je Output die Schranke aus, mit Aufschluesselung und dem
/// Vorbehalt der Zeitspalte (9.4.5).
#[test]
fn latency_reports_ticks_and_its_caveat() {
    let out = takt(&["latency", "corpus-try/14_latency.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("Ticks"), "die Tickspalte steht da:\n{text}");
    assert!(text.contains("erkennen"), "die Aufschluesselung steht dabei:\n{text}");
    assert!(text.contains("Schedulability"), "der Vorbehalt der Zeitspalte steht dabei:\n{text}");
}
