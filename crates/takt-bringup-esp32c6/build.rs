//! Baut das Takt-Programm fuer den ESP32-C6 und bindet es ein.
//!
//! Dieselbe Konstruktion wie beim F401-Bring-up: `takt build` erzeugt das
//! Objekt fuer `riscv32imac`, `takt_conformance::mcu` den C-Rahmen, `clang`
//! uebersetzt ihn fuer RV32IMAC, `llvm-ar` packt beides in ein Archiv.
//! Die Linker-Argumente stehen hier und nicht in `.cargo/config.toml`: Die
//! Konfigurationsdatei gilt nur, wenn `cargo` aus diesem Verzeichnis laeuft,
//! und ein Bau von aussen erzeugte sonst still ein Binary ohne Speicherkarte.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // `linkall.x` liefert `esp-hal`: Speicherkarte, Vektortabelle, Cache-Mapping.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    build_takt_program(&out);
}

fn build_takt_program(out: &Path) {
    let program = program_path();
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    println!("cargo:rerun-if-changed={program}");
    if !Path::new(&program).exists() {
        panic!("Takt-Programm nicht gefunden: {program}");
    }
    let Some(p) = compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let rahmen = out.join("takt_rahmen.c");
    if let Err(e) = fs::write(&rahmen, takt_conformance::mcu::build(&p).source) {
        panic!("Rahmen nicht schreibbar: {e}");
    }
    let obj = out.join("takt_programm.o");
    run_takt_build(&program, &["--emit", "obj"], &obj);
    run_takt_build(&program, &["--emit", "consts-rs"], &out.join("takt_consts.rs"));
    let obj_rahmen = out.join("takt_rahmen.o");
    compile_c(&rahmen, &obj_rahmen);
    archive(out, &[&obj, &obj_rahmen]);
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=taktprogramm");
}

/// Der Pfad des Programms aus `takt.toml`; `TAKT_PROGRAM` sticht fuer
/// einen einmaligen Versuch (FB-141 erklaert, warum die Datei der Normalfall ist).
fn program_path() -> String {
    if let Ok(p) = env::var("TAKT_PROGRAM") {
        return p;
    }
    let here = env!("CARGO_MANIFEST_DIR");
    let config = format!("{here}/takt.toml");
    println!("cargo:rerun-if-changed={config}");
    let text = fs::read_to_string(&config).unwrap_or_else(|e| panic!("{config}: {e}"));
    let value = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| {
            l.strip_prefix("program")?.trim_start().strip_prefix('=')?.trim().strip_prefix('"')?.strip_suffix('"')
        })
        .unwrap_or_else(|| panic!("{config}: kein `program = \"…\"`"));
    format!("{here}/{value}")
}

fn compile(path: &str) -> Option<takt_mir::Program> {
    let src = fs::read_to_string(path).ok()?;
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Hw, profile: None };
    let checked = takt_sema::compile(&src, &options);
    if checked.program.is_none() {
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("cargo:warning={path}: {d}");
        }
    }
    checked.program
}

fn run_takt_build(program: &str, emit: &[&str], out: &Path) {
    let Some(takt) = find_takt() else {
        let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
        panic!("Das Werkzeug takt fehlt; erst `cargo build -p takt-cli --{profile}`");
    };
    println!("cargo:rerun-if-changed={}", takt.display());
    let status = Command::new(&takt)
        .args(["build", program, "--target", "riscv32imac", "--build", "hw"])
        .args(emit)
        .arg("--out")
        .arg(out)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => panic!("takt build {} schlug fehl fuer {program}", emit.join(" ")),
        Err(e) => panic!("takt nicht aufrufbar ({e}); ist `cargo build -p takt-cli` gelaufen?"),
    }
}

fn compile_c(src: &Path, obj: &Path) {
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn entsteht kein Rahmen") };
    let ok = Command::new(&clang)
        .args([
            "-O2",
            "-c",
            "-ffreestanding",
            "-nostdlib",
            "--target=riscv32-unknown-none-elf",
            "-march=rv32imac",
            "-mabi=ilp32",
        ])
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}

fn archive(out: &Path, objs: &[&Path]) {
    let lib = out.join("libtaktprogramm.a");
    let _ = fs::remove_file(&lib);
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn auch kein llvm-ar") };
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar).arg("crs").arg(&lib).args(objs).status().is_ok_and(|s| s.success());
    assert!(ok, "llvm-ar schlug fehl; das Takt-Programm waere nicht gebunden");
}

fn clang() -> Option<PathBuf> {
    match takt_llvm::toolchain::find() {
        takt_llvm::toolchain::Clang::At(p) => Some(p),
        takt_llvm::toolchain::Clang::Missing => None,
    }
}

/// Das `takt`-Werkzeug aus demselben Zielverzeichnis, in dem dieses
/// Skript laeuft; ein paar Ebenen ueber `OUT_DIR`.
fn find_takt() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    let out = PathBuf::from(env::var("OUT_DIR").ok()?);
    let mut dir = out.as_path();
    for _ in 0..6 {
        let Some(parent) = dir.parent() else { break };
        dir = parent;
        let p = dir.join(&profile).join(exe);
        if p.exists() {
            return Some(p);
        }
    }
    None
}
