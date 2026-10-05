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
    let options = takt_sema::Options {
        policy: Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
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

fn compile_errors(src: &str) -> Vec<String> {
    let out = takt_sema::compile(src, &takt_sema::Options::default());
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// **Jedes genannte Native braucht seine Zeile.** Bei `reviewed(a, b)`
/// und einer Zeile nur fuer `a` meldet genau `b`.
#[test]
fn each_named_native_needs_its_own_line() {
    let src = "system:\n    language   = 1\n    tick       = 10 ms\n    tcb_policy = reviewed(crc_custom, crc_other)\n\n\
         native fn crc_custom(b: bytes<64>) -> u16 from \"crypto.rs\" with cost = {i32: 64}, stack = 32, total\n\
         native fn crc_other(b: bytes<64>) -> u16 from \"other.rs\" with cost = {i32: 64}, stack = 32, total\n\n\
         output sum : u16 @ hw(\"o/sum\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            \
         sum = crc_custom(default) ^ crc_other(default)\n\n        after 1 s: -> RUN\n";
    let out = takt_sema::compile(src, &takt_sema::Options::default());
    let p = out.program.expect("Programm");
    let d = takt_sema::calibrated::reviewed(&p, &reviewed_source(), &source);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(format!("{}", d[0]).contains("crc_other") && format!("{}", d[0]).contains("nicht geprüft"), "{}", d[0]);
}

/// **Der Schluessel ist Name und Hash.** Dieselbe Quelle unter einem
/// anderen Namen geprueft gilt nicht fuer `crc_custom`.
#[test]
fn a_line_under_another_name_does_not_count() {
    let p = program("tcb_policy = reviewed(crc_custom)");
    let mut r = Review::default();
    r.insert(Entry {
        native: "crc_renamed".to_string(),
        hash: review::hash_of(SOURCE),
        by: "anna.beispiel".to_string(),
        date: "2026-09-22".to_string(),
    });
    let d = takt_sema::calibrated::reviewed(&p, &r, &source);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(format!("{}", d[0]).contains("nicht geprüft"), "{}", d[0]);
}

/// **`reviewed(…)` ist zugleich die Liste** (4.5): Ein Projekt-Native, das
/// sie nicht nennt, ist ein Fehler wie unter `allowlist`.
#[test]
fn a_native_missing_from_reviewed_is_an_error() {
    let src = "system:\n    language   = 1\n    tick       = 10 ms\n    tcb_policy = reviewed(other)\n\n\
         native fn crc_custom(b: bytes<64>) -> u16 from \"crypto.rs\" with cost = {i32: 64}, stack = 32, total\n\n\
         output sum : u16 @ hw(\"o/sum\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            sum = crc_custom(default)\n\n        \
         after 1 s: -> RUN\n";
    let e = compile_errors(src);
    assert!(e.iter().any(|e| e.contains("tcb_policy") && e.contains("crc_custom")), "{e:?}");
}

/// **Doppelte Zeilen sind eine**: Dieselbe Fassung zweimal geprueft ist
/// ein Eintrag (`Review::insert`), und das Native gilt als geprueft.
#[test]
fn a_repeated_line_is_one_entry() {
    let hash = review::hash_of(SOURCE);
    let text = format!("crc_custom {hash} anna 2026-09-22\ncrc_custom {hash} anna 2026-09-22\n");
    let r = review::parse(&text).expect("lesbar");
    assert_eq!(r.entries().count(), 1);
    let p = program("tcb_policy = reviewed(crc_custom)");
    assert!(takt_sema::calibrated::reviewed(&p, &r, &source).is_empty());
}

/// **Das Datum hat die Form `JJJJ-MM-TT`** (takt-mir `review::Entry`): Ein
/// anderes ist eine kaputte Zeile mit ihrer Nummer — die Datei ist das
/// Audit, und ein unlesbares Datum belegt nichts.
#[test]
fn an_invalid_date_names_its_line() {
    let hash = review::hash_of(SOURCE);
    for date in ["22.09.2026", "2026-13-01", "gestern"] {
        let text = format!("# Kopf\ncrc_custom {hash} anna {date}\n");
        let e = review::parse(&text).expect_err(date);
        assert_eq!(e.line, 2, "{date}: {}", e.message);
    }
}
