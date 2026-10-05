//! `takt build` uebersetzt fuer jedes Ziel (11.2, 12.8; FB-138).
//!
//! **Warum das ein eigener Test ist.** Die Uebersetzung selbst prueft
//! `mcu_codegen.rs` auf der Bibliotheksebene. Was hier geprueft wird, ist
//! der Weg davor: dass ein Nutzer sie *aufrufen* kann. Das war die Luecke,
//! die FB-138 festhaelt — die Pipeline stand in einer `build.rs`, wo sie
//! niemand findet und wo Fehler zu Warnungen werden.

use std::path::Path;
use std::process::Command;

/// Der Pfad zur gebauten CLI, und sie ist nicht aelter als ihre Quellen.
///
/// `CARGO_BIN_EXE_` gibt es nur fuer Binaries desselben Crates; die CLI
/// liegt in einem anderen, also wird sie im Zielverzeichnis gesucht — das
/// juengste aus `debug` und `release`. Ein Binary, das aelter ist als eine
/// Quelle des Compilers, baute das Objekt von gestern (FB-193); das ist ein
/// Fehlschlag mit Namen, kein Bestehen (KON1-031).
fn cli() -> Option<std::path::PathBuf> {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| here.join("../../target"), Into::into);
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let found = ["debug", "release"]
        .iter()
        .map(|profile| target.join(profile).join(exe))
        .filter_map(|p| modified(&p).map(|t| (t, p)))
        .max_by_key(|(t, _)| *t);
    let (built, path) = found?;
    let mut sources = Vec::new();
    for krate in ["takt-syntax", "takt-diag", "takt-mir", "takt-sema", "takt-interp", "takt-llvm", "takt-cli"] {
        rust_files(&here.join("..").join(krate).join("src"), &mut sources);
    }
    if let Some(newer) = sources.iter().find(|f| modified(f).is_some_and(|t| t > built)) {
        panic!("{} ist aelter als {}; erst `cargo build -p takt-cli` (FB-193)", path.display(), newer.display());
    }
    Some(path)
}

/// Die Rust-Dateien unter `dir`.
fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

/// Der Maschinentyp eines ELF-Objekts (`e_machine`).
fn elf_machine(object: &[u8]) -> Option<u16> {
    (object.get(..4)? == b"\x7fELF").then(|| u16::from_le_bytes([object[18], object[19]]))
}

fn program() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/16_timing.takt").to_string()
}

/// **Jedes Ziel der Abnahme laesst sich uebersetzen.**
///
/// Vier Ziele, zwei Zielklassen — und der Weg dorthin ist ein Kommando,
/// kein Build-Skript.
#[test]
fn every_target_builds_from_the_command_line() {
    let Some(takt) = takt_testkit::require("takt-cli", cli(), "`cargo build -p takt-cli`") else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-cli-build");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");

    // `e_machine` je Ziel: x86-64, AArch64, ARM, RISC-V.
    for (target, machine) in [("x86_64", 62), ("aarch64", 183), ("thumbv7em", 40), ("riscv32imac", 243)] {
        let out = dir.join(format!("{target}.o"));
        let result = Command::new(&takt)
            .args(["build", &program(), "--target", target, "--prefix", "timing", "--out"])
            .arg(&out)
            .output()
            .expect("takt build");
        assert!(
            result.status.success(),
            "{target}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let object = std::fs::read(&out).unwrap_or_default();
        assert!(!object.is_empty(), "{target}: leeres Objekt");
        assert_eq!(elf_machine(&object), Some(machine), "{target}: ein Objekt fuer eine andere Maschine");
        eprintln!("{target}: {} Byte", object.len());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--emit ir` liefert lesbare IR mit dem Triple des Ziels im Kopf.
#[test]
fn emitting_ir_carries_the_target_triple() {
    let Some(takt) = takt_testkit::require("takt-cli", cli(), "`cargo build -p takt-cli`") else { return };
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-cli-build.ll");
    let result = Command::new(&takt)
        .args(["build", &program(), "--target", "thumbv7em", "--prefix", "timing", "--emit", "ir", "--out"])
        .arg(&out)
        .output()
        .expect("takt build");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));

    let ir = std::fs::read_to_string(&out).expect("IR lesbar");
    assert!(ir.contains("thumbv7em-none-eabihf"), "das Triple steht im Kopf (11.3)");
    assert!(ir.contains("_step"), "die Schrittfunktion ist da (11.2)");
    assert!(ir.contains("define void @timing_"), "die Einstiege tragen das Praefix (12.11)");
    let _ = std::fs::remove_file(&out);
}

/// **Das Praefix kommt aus dem Dateinamen oder aus `--prefix`** (12.11).
/// Ist der Name kein C-Bezeichner — wie bei den Korpusdateien mit Ziffer
/// vorn —, baut `takt build` nicht und nennt die Option, statt still einen
/// anderen Namen zu waehlen.
#[test]
fn the_prefix_comes_from_the_file_name_or_the_option() {
    let Some(takt) = takt_testkit::require("takt-cli", cli(), "`cargo build -p takt-cli`") else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-cli-prefix");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let named = dir.join("ventil.takt");
    std::fs::copy(program(), &named).expect("kopieren");
    let out = dir.join("ventil.ll");
    let result = Command::new(&takt)
        .arg("build")
        .arg(&named)
        .args(["--target", "thumbv7em", "--emit", "ir", "--out"])
        .arg(&out)
        .output()
        .expect("takt build");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let ir = std::fs::read_to_string(&out).expect("IR lesbar");
    assert!(ir.contains("define void @ventil_"), "der Dateiname ist das Praefix");

    let result = Command::new(&takt).args(["build", &program(), "--target", "thumbv7em"]).output().expect("takt build");
    assert!(!result.status.success(), "`16_timing` ist kein Praefix");
    let msg = String::from_utf8_lossy(&result.stderr);
    assert!(msg.contains("16_timing") && msg.contains("--prefix"), "die Meldung nennt Namen und Ausweg: {msg}");

    // Ueber `--prefix` gilt dasselbe (12.11): `takt` und `takt_` gehoeren den
    // geteilten Bibliotheken, ein Bezeichner beginnt mit einem Buchstaben;
    // ein abgelehntes Praefix hinterlaesst kein Objekt.
    for prefix in ["takt", "takt_x", "9x"] {
        let out = dir.join(format!("{prefix}.o"));
        let result = Command::new(&takt)
            .args(["build", &program(), "--target", "thumbv7em", "--prefix", prefix, "--out"])
            .arg(&out)
            .output()
            .expect("takt build");
        assert!(!result.status.success(), "`{prefix}` ist kein Praefix");
        let msg = String::from_utf8_lossy(&result.stderr);
        assert!(msg.contains(&format!("`{prefix}` ist kein Praefix")), "{prefix}: {msg}");
        assert!(!out.exists(), "{prefix}: ein abgelehntes Praefix hinterlaesst ein Objekt");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ein unbekanntes Ziel wird benannt, nicht geraten.
///
/// **Die Meldung nennt die bekannten.** Ein „unknown target" ohne Liste
/// laesst den Nutzer raten — und beim RISC-V-Ziel war genau das die
/// Falle: Das Rust-Target heisst anders als das LLVM-Triple.
#[test]
fn an_unknown_target_is_named() {
    let Some(takt) = takt_testkit::require("takt-cli", cli(), "`cargo build -p takt-cli`") else { return };
    let result =
        Command::new(&takt).args(["build", &program(), "--target", "gibtsnicht"]).output().expect("takt build");
    assert!(!result.status.success(), "ein unbekanntes Ziel ist ein Fehler");
    let msg = String::from_utf8_lossy(&result.stderr);
    assert!(msg.contains("gibtsnicht"), "die Meldung nennt das Ziel: {msg}");
    assert!(msg.contains("thumbv7em"), "und die bekannten: {msg}");
}

/// Ein Programm mit Fehlern bricht ab, statt ein halbes Objekt zu lassen —
/// auch mit `--out`: Das Objekt gibt es danach nicht.
#[test]
fn a_broken_program_fails_the_build() {
    let Some(takt) = takt_testkit::require("takt-cli", cli(), "`cargo build -p takt-cli`") else { return };
    let src = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-cli-kaputt.takt");
    let out = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-cli-kaputt.o");
    let _ = std::fs::remove_file(&out);
    std::fs::write(&src, "system:\n    language = 1\n\nmachine m:\n    initial FEHLT\n").expect("schreiben");
    let result = Command::new(&takt)
        .arg("build")
        .arg(&src)
        .args(["--target", "thumbv7em", "--prefix", "kaputt", "--out"])
        .arg(&out)
        .output()
        .expect("takt build");
    assert!(!result.status.success(), "ein Programm mit Fehlern baut nicht");
    assert!(!out.exists(), "ein Programm mit Fehlern hinterlaesst ein Objekt");
    let _ = std::fs::remove_file(&src);
}
