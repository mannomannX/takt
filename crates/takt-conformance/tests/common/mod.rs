//! Was die Abnahmetests brauchen: bauen und ausfuehren.
//!
//! Jeder Test bindet dieses Modul einzeln ein, und keiner benutzt alles —
//! `dead_code` ist hier die Regel, nicht die Ausnahme.
#![allow(dead_code)]

use takt_conformance::harness;
use takt_conformance::stimulus::Stimulus;
use takt_llvm::toolchain::Clang;
use takt_mir::program::Program;

/// Erzeugt die IR eines Programms, so wie der Compiler sie erzeugt.
pub fn ir_of(p: &Program) -> String {
    ir_for(p, "x86_64-pc-windows-msvc")
}

/// Wie `ir_of`, fuer ein bestimmtes Ziel (12.8).
///
/// Der einzige Unterschied ist das Triple im Kopf — das ist die
/// Bedingung, unter der Satz 9.4.4 eine Aussage ueber eine Uebersetzung
/// ist und nicht ueber zwei Programme.
pub fn ir_for(p: &Program, triple: &str) -> String {
    // Die Uebersetzung selbst steht in `takt-llvm::lower` — sie wird
    // seit M5 auch ausserhalb der Abnahme gebraucht (das Bring-up-
    // Programm bindet den erzeugten Code mit). Zwei Fassungen derselben
    // Folge waeren eine Quelle dafuer, dass der Test etwas anderes
    // prueft, als die Werkzeuge erzeugen.
    let out = takt_llvm::lower::program(p, triple, "abnahme");
    for s in &out.skipped {
        eprintln!("{}_step fehlt: {}", s.machine, s.reason);
    }
    out.ir
}

/// Uebersetzt ein Programm mit seinem Testrahmen und fuehrt es aus.
pub fn run_native(clang: &Clang, p: &Program, name: &str, machine: &str, ticks: u64) -> Result<String, String> {
    run_native_with(clang, p, name, machine, ticks, &[])
}

/// Wie `run_native`, mit Eingaben (12.5): Commands (8.5) und
/// Stromelemente (8.6). Beide Seiten sehen damit denselben Stimulus.
/// Uebersetzt ein Programm mit *allen* Maschinen und fuehrt es aus.
///
/// Fuer Programme mit Plant-Modell (8.3): Das Modell ist eine
/// gewoehnliche Maschine, und ohne sie bleiben die Eingaenge `Bad`.
#[allow(dead_code)]
pub fn run_native_all(clang: &Clang, p: &Program, name: &str, ticks: u64) -> Result<String, String> {
    run_native_inner(clang, p, name, None, ticks, &[])
}

pub fn run_native_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    machine: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_native_inner(clang, p, name, Some(machine), ticks, inputs)
}

/// Der gemeinsame Rumpf: `Some(name)` fuehrt eine Maschine, `None` alle.
fn run_native_inner(
    clang: &Clang,
    p: &Program,
    name: &str,
    machine: Option<&str>,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("takt-abnahme-{}", name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, ir_of(p)).map_err(|e| e.to_string())?;
    let h = match machine {
        Some(name) => harness::build_with(p, name, ticks, inputs),
        None => harness::build_all(p, ticks, inputs),
    };
    std::fs::write(&c, &h.source).map_err(|e| e.to_string())?;
    let path = clang.path().ok_or("clang")?;
    let mut cmd = std::process::Command::new(path);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| e.to_string())?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).to_string());
    }
    let out = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}
