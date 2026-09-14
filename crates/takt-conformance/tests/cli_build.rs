//! `takt build` uebersetzt fuer jedes Ziel (11.2, 12.8; FB-138).
//!
//! **Warum das ein eigener Test ist.** Die Uebersetzung selbst prueft
//! `mcu_codegen.rs` auf der Bibliotheksebene. Was hier geprueft wird, ist
//! der Weg davor: dass ein Nutzer sie *aufrufen* kann. Das war die Luecke,
//! die FB-138 festhaelt — die Pipeline stand in einer `build.rs`, wo sie
//! niemand findet und wo Fehler zu Warnungen werden.

use std::path::Path;
use std::process::Command;

/// Der Pfad zur gebauten CLI.
///
/// `CARGO_BIN_EXE_` gibt es nur fuer Binaries desselben Crates; die CLI
/// liegt in einem anderen, also wird sie im Zielverzeichnis gesucht.
fn cli() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    for profile in ["debug", "release"] {
        let p = here.join("../../target").join(profile).join(exe);
        if p.exists() {
            return Some(p);
        }
        // Das Zielverzeichnis kann umgelenkt sein.
        if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
            let p = Path::new(&dir).join(profile).join(exe);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

fn program() -> String {
    format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/16_timing.takt"))
}

/// **Jedes Ziel der Abnahme laesst sich uebersetzen.**
///
/// Vier Ziele, zwei Zielklassen — und der Weg dorthin ist ein Kommando,
/// kein Build-Skript.
#[test]
fn every_target_builds_from_the_command_line() {
    let Some(takt) = cli() else {
        eprintln!("takt-CLI nicht gebaut; uebersprungen");
        return;
    };
    let dir = std::env::temp_dir().join("takt-cli-build");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");

    for target in ["x86_64", "aarch64", "thumbv7em", "riscv32imac"] {
        let out = dir.join(format!("{target}.o"));
        let result = Command::new(&takt)
            .args(["build", &program(), "--target", target, "--out"])
            .arg(&out)
            .output()
            .expect("takt build");
        assert!(
            result.status.success(),
            "{target}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        assert!(size > 0, "{target}: leeres Objekt");
        eprintln!("{target}: {size} Byte");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--emit ir` liefert lesbare IR mit dem Triple des Ziels im Kopf.
#[test]
fn emitting_ir_carries_the_target_triple() {
    let Some(takt) = cli() else {
        eprintln!("takt-CLI nicht gebaut; uebersprungen");
        return;
    };
    let out = std::env::temp_dir().join("takt-cli-build.ll");
    let result = Command::new(&takt)
        .args(["build", &program(), "--target", "thumbv7em", "--emit", "ir", "--out"])
        .arg(&out)
        .output()
        .expect("takt build");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));

    let ir = std::fs::read_to_string(&out).expect("IR lesbar");
    assert!(ir.contains("thumbv7em-none-eabihf"), "das Triple steht im Kopf (11.3)");
    assert!(ir.contains("_step"), "die Schrittfunktion ist da (11.2)");
    let _ = std::fs::remove_file(&out);
}

/// Ein unbekanntes Ziel wird benannt, nicht geraten.
///
/// **Die Meldung nennt die bekannten.** Ein „unknown target" ohne Liste
/// laesst den Nutzer raten — und beim RISC-V-Ziel war genau das die
/// Falle: Das Rust-Target heisst anders als das LLVM-Triple.
#[test]
fn an_unknown_target_is_named() {
    let Some(takt) = cli() else {
        eprintln!("takt-CLI nicht gebaut; uebersprungen");
        return;
    };
    let result =
        Command::new(&takt).args(["build", &program(), "--target", "gibtsnicht"]).output().expect("takt build");
    assert!(!result.status.success(), "ein unbekanntes Ziel ist ein Fehler");
    let msg = String::from_utf8_lossy(&result.stderr);
    assert!(msg.contains("gibtsnicht"), "die Meldung nennt das Ziel: {msg}");
    assert!(msg.contains("thumbv7em"), "und die bekannten: {msg}");
}

/// Ein Programm mit Fehlern bricht ab, statt ein halbes Objekt zu lassen.
#[test]
fn a_broken_program_fails_the_build() {
    let Some(takt) = cli() else {
        eprintln!("takt-CLI nicht gebaut; uebersprungen");
        return;
    };
    let src = std::env::temp_dir().join("takt-cli-kaputt.takt");
    std::fs::write(&src, "system:\n    language = 1\n\nmachine m:\n    initial FEHLT\n").expect("schreiben");
    let result = Command::new(&takt).arg("build").arg(&src).output().expect("takt build");
    assert!(!result.status.success(), "ein Programm mit Fehlern baut nicht");
    let _ = std::fs::remove_file(&src);
}
