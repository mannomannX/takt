//! `tcb_policy = reviewed(…)`: der Review-Prozess fuer Projekt-Natives
//! (4.5, v1.2).
//!
//! **Was hier belegt wird.** `allowlist` sagt, dass ein Programm von
//! seinem Projekt-Native weiss. `reviewed` verlangt zusaetzlich, dass
//! jemand hingesehen hat — und zwar auf *diese* Fassung: Der Schluessel
//! ist der Hash der Quelle, nicht ihr Name.

use takt_diag::Policy;
use takt_mir::review::{self, Entry, Review};

const SOURCE: &[u8] = b"pub fn crc_custom(_b: &[u8]) -> u16 { 0 }\n";

fn program(policy: &str) -> takt_mir::Program {
    let src = format!(
        "system:\n    language   = 1\n    tick       = 10 ms\n    {policy}\n\n\
         native fn crc_custom(b: bytes<64>) -> u16 from \"crypto.rs\" \
         with cost = {{i32: 64}}, stack = 32, total\n\n\
         output sum : u16 @ hw(\"o/sum\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            \
         sum = crc_custom(default)\n\n        after 1 s: -> RUN\n"
    );
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

fn source(_: &str) -> Option<Vec<u8>> {
    Some(SOURCE.to_vec())
}

fn reviewed_source() -> Review {
    let mut r = Review::default();
    r.insert(Entry {
        native: "crc_custom".to_string(),
        hash: review::hash_of(SOURCE),
        by: "anna.beispiel".to_string(),
        date: "2026-09-22".to_string(),
    });
    r
}

/// **Ohne `reviewed` prueft nichts.** `allowlist` bleibt, was es war.
#[test]
fn an_allowlist_does_not_ask_for_a_review() {
    let p = program("tcb_policy = allowlist(crc_custom)");
    assert!(!p.config.tcb_reviewed);
    let d = takt_sema::calibrated::reviewed(&p, &Review::default(), &source);
    assert!(d.is_empty(), "{d:?}");
}

/// **Ein ungeprueftes Native ist ein Fehler.**
#[test]
fn an_unreviewed_native_is_rejected() {
    let p = program("tcb_policy = reviewed(crc_custom)");
    assert!(p.config.tcb_reviewed);
    let d = takt_sema::calibrated::reviewed(&p, &Review::default(), &source);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "SC-31");
    assert!(format!("{}", d[0]).contains("nicht geprüft"), "{}", d[0]);
}

/// **Mit Zeile geht es durch.**
#[test]
fn a_reviewed_native_passes() {
    let p = program("tcb_policy = reviewed(crc_custom)");
    let d = takt_sema::calibrated::reviewed(&p, &reviewed_source(), &source);
    assert!(d.is_empty(), "{d:?}");
}

/// **Eine geaenderte Quelle faellt auf.** Das ist der Sinn des Hashes:
/// Eine andere Implementierung ist ein anderes Native, auch unter
/// demselben Namen — und die Meldung sagt, dass es *war*, nicht dass es
/// nie geprueft wurde.
#[test]
fn a_changed_source_loses_its_review() {
    let p = program("tcb_policy = reviewed(crc_custom)");
    let changed = |_: &str| Some(b"pub fn crc_custom(_b: &[u8]) -> u16 { 1 }\n".to_vec());
    let d = takt_sema::calibrated::reviewed(&p, &reviewed_source(), &changed);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(format!("{}", d[0]).contains("seit dem Review geändert"), "{}", d[0]);
}

/// **Eine unlesbare Quelle ist ein Fehler, keine stille Annahme.**
#[test]
fn an_unreadable_source_is_reported() {
    let p = program("tcb_policy = reviewed(crc_custom)");
    let missing = |_: &str| None;
    let d = takt_sema::calibrated::reviewed(&p, &reviewed_source(), &missing);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(format!("{}", d[0]).contains("nicht lesbar"), "{}", d[0]);
}

/// **Die Datei geht durch Schreiben und Lesen unveraendert.**
#[test]
fn the_file_round_trips() {
    let before = reviewed_source();
    let after = review::parse(&before.render()).expect("lesbar");
    let (a, b): (Vec<_>, Vec<_>) = (before.entries().collect(), after.entries().collect());
    assert_eq!(a, b);
}

/// **Eine kaputte Zeile sagt, welche.**
#[test]
fn a_malformed_line_names_its_number() {
    let e = review::parse("# Kopf\ncrc_custom zu-kurz anna 2026-09-22\n").expect_err("Fehler");
    assert_eq!(e.line, 2);
    assert!(e.message.contains("SHA-256"), "{}", e.message);
}
