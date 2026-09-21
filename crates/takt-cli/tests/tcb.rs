//! `tcb_policy = reviewed(…)` durch das Werkzeug (Referenz 4.5, v1.2):
//! `takt check` lehnt ein ungepruefetes Projekt-Native ab, `takt tcb
//! review` schreibt die Zeile, und eine geaenderte Quelle verliert sie
//! wieder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SOURCE: &str = "pub fn crc_custom(_b: &[u8]) -> u16 { 0 }\n";

const PROGRAM: &str = "\
system:
    language   = 1
    tick       = 10 ms
    tcb_policy = reviewed(crc_custom)

native fn crc_custom(b: bytes<64>) -> u16 from \"crypto.rs\" with cost = {i32: 64}, stack = 32, total

output sum : u16 @ hw(\"o/sum\") with safe = 0

machine m:
    initial RUN

    state RUN:
        loop:
            sum = crc_custom(default)

        after 1 s: -> RUN
";

/// Ein eigenes Verzeichnis je Test: `natives.review` liegt neben dem
/// Programm, und zwei Tests duerfen sich nicht dieselbe Datei teilen.
fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("takt-tcb-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    std::fs::write(dir.join("p.takt"), PROGRAM).expect("Programm");
    std::fs::write(dir.join("crypto.rs"), SOURCE).expect("Quelle");
    dir
}

fn takt(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(dir).args(args).output().expect("takt startet")
}

#[test]
fn an_unreviewed_native_is_refused_under_reviewed_policy() {
    let dir = project("unreviewed");
    let out = takt(&dir, &["check", "p.takt"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("SC-31"), "{text}");
    assert!(text.contains("nicht geprüft"), "{text}");
}

#[test]
fn a_review_line_lets_the_check_pass() {
    let dir = project("reviewed");
    let wrote =
        takt(&dir, &["tcb", "review", "p.takt", "--native", "crc_custom", "--by", "anna", "--date", "2026-09-22"]);
    assert!(wrote.status.success(), "{}", String::from_utf8_lossy(&wrote.stderr));

    let file = std::fs::read_to_string(dir.join("natives.review")).expect("natives.review");
    assert!(file.contains("crc_custom"), "{file}");
    assert!(file.contains("anna 2026-09-22"), "{file}");

    let out = takt(&dir, &["check", "p.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn a_changed_source_needs_a_new_review() {
    let dir = project("changed");
    let wrote =
        takt(&dir, &["tcb", "review", "p.takt", "--native", "crc_custom", "--by", "anna", "--date", "2026-09-22"]);
    assert!(wrote.status.success(), "{}", String::from_utf8_lossy(&wrote.stderr));

    std::fs::write(dir.join("crypto.rs"), "pub fn crc_custom(_b: &[u8]) -> u16 { 1 }\n").expect("Quelle");
    let out = takt(&dir, &["check", "p.takt"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("seit dem Review geändert"), "{text}");
}

/// **Ohne `reviewed` fragt niemand nach der Datei.** `allowlist` bleibt,
/// was es war — der Prozess ist eine Zusatzforderung, keine neue Pflicht
/// fuer bestehende Programme.
#[test]
fn an_allowlist_program_needs_no_review_file() {
    let dir = project("allowlist");
    let program = PROGRAM.replace("reviewed(crc_custom)", "allowlist(crc_custom)");
    std::fs::write(dir.join("p.takt"), program).expect("Programm");
    let out = takt(&dir, &["check", "p.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    assert!(!dir.join("natives.review").exists());
}
