//! Was die Abnahmetests brauchen: bauen und ausfuehren.
//!
//! Jeder Test bindet dieses Modul einzeln ein, und keiner benutzt alles —
//! `dead_code` ist hier die Regel, nicht die Ausnahme.
#![allow(dead_code)]

pub mod board;

use takt_conformance::harness;
use takt_conformance::stimulus::Stimulus;
use takt_llvm::toolchain::Clang;
use takt_mir::program::Program;

/// clang fuer einen Test: fehlt er, scheitert der Test, es sei denn,
/// `TAKT_ALLOW_MISSING` erlaubt das Fehlen (FB-392).
pub fn clang() -> Option<Clang> {
    clang_path().map(Clang::At)
}

/// Die Binutils des Wirts (`size`, `nm`, `objdump`) fuer einen Test; wie
/// [`clang`] ein Pflichtwerkzeug.
pub fn binutils() -> Option<takt_llvm::inspect::Binutils> {
    let tools = takt_llvm::inspect::Binutils::host();
    takt_testkit::require("binutils", tools.available().then_some(tools), "`size`, `nm` und `objdump` auf den PATH")
}

/// Wie [`clang`], als Pfad.
pub fn clang_path() -> Option<std::path::PathBuf> {
    let found = match takt_llvm::toolchain::find() {
        Clang::At(path) => Some(path),
        Clang::Missing => None,
    };
    takt_testkit::require("clang", found, "`TAKT_CLANG` setzen oder LLVM installieren")
}

/// Die Programme, die der Codegen ganz senken muss, mit ihrem Namen
/// relativ zu `corpus-try`: die Suite `vergleich` des Manifests (FB-378;
/// ihre Ausnahmen sind die Luecken des Codegens) und jedes Beispiel unter
/// `corpus-try/sim/`, das die Sema ohne Fehler annimmt.
pub fn corpus_programs() -> Vec<(String, Program)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try");
    let mut names: Vec<String> =
        takt_conformance::suites::programs("vergleich").into_iter().map(str::to_string).collect();
    names.extend(
        std::fs::read_dir(root.join("sim"))
            .expect("corpus-try/sim lesbar")
            .flatten()
            .filter(|e| e.path().join("program.takt").is_file())
            .map(|e| format!("sim/{}/program.takt", e.file_name().to_string_lossy())),
    );
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let src = std::fs::read_to_string(root.join(&name)).ok()?;
            let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
            let out = takt_sema::compile(&src, &options);
            if out.diagnostics.iter().any(|d| d.is_error()) {
                return None;
            }
            out.program.map(|p| (name, p))
        })
        .collect()
}

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
    complete(takt_llvm::lower::program(p, triple, &takt_llvm::symbols::Prefix::default()))
}

/// Wie [`ir_of`], fuer einen Lauf unter AddressSanitizer (FB-461).
pub fn ir_sanitized(p: &Program) -> String {
    complete(takt_llvm::lower::program_sanitized(p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default()))
}

/// Die IR einer vollstaendigen Senkung.
fn complete(out: takt_llvm::lower::Lowered) -> String {
    // Eine Senkung, die etwas auslaesst, ist ein Fehlschlag und keine Notiz
    // auf stderr: Ein Konstrukt, das der Codegen nach einer Regression nicht
    // mehr senkt, nahme sonst seine Programme still aus dem Vergleich
    // (KON1-018).
    let skipped: Vec<String> =
        out.skipped.iter().map(|s| format!("{} fehlt: {}", takt_llvm::lower::Skipped::what(s), s.reason)).collect();
    assert!(
        skipped.is_empty(),
        "der Codegen senkt nicht alles:
{}",
        skipped.join(
            "
"
        )
    );
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
    run_native_inner(clang, p, name, None, ticks, &[], &[])
}

pub fn run_native_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    machine: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_native_inner(clang, p, name, Some(machine), ticks, inputs, &[])
}

/// Alle Maschinen mit Eingaben (12.5).
#[allow(dead_code)]
pub fn run_native_all_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_native_inner(clang, p, name, None, ticks, inputs, &[])
}

/// Alle Maschinen mit virtuellem Schlaf (9.9): der Pfad `_advance`.
#[allow(dead_code)]
pub fn run_native_sleeping(clang: &Clang, p: &Program, name: &str, ticks: u64) -> Result<String, String> {
    run_native_build(clang, p, name, ticks, harness::build_sleeping(p, ticks))
}

/// Wie [`run_native_sleeping`], mit Eingaben: geschlafen wird nie ueber
/// einen Tick mit einer Lieferung.
pub fn run_native_sleeping_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_native_build(clang, p, name, ticks, harness::build_sleeping_with(p, ticks, inputs))
}

/// Alle Maschinen, mit einer Journal-Nutzlast beim Start (5.9).
pub fn run_native_persist(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    payload: &[u8],
) -> Result<String, String> {
    run_native_inner(clang, p, name, None, ticks, &[], payload)
}

/// Ein Szenario nativ (13.6): die laufenden Maschinen samt dem
/// gewaehlten Szenario, wie `takt test` sie fuehrt.
#[allow(dead_code)]
pub fn run_native_scenario(
    clang: &Clang,
    p: &Program,
    name: &str,
    scenario: &str,
    ticks: u64,
) -> Result<String, String> {
    run_native_build(clang, p, name, ticks, harness::build_scenario(p, scenario, ticks, &[]))
}

/// Wie [`run_native_scenario`], mit Eingaben (12.5): der Stimulus des
/// Szenarios aus `<szenario>.stim.trace`.
pub fn run_native_scenario_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    scenario: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_native_build(clang, p, name, ticks, harness::build_scenario(p, scenario, ticks, inputs))
}

/// Der gemeinsame Rumpf: `Some(name)` fuehrt eine Maschine, `None` alle.
fn run_native_inner(
    clang: &Clang,
    p: &Program,
    name: &str,
    machine: Option<&str>,
    ticks: u64,
    inputs: &[Stimulus],
    payload: &[u8],
) -> Result<String, String> {
    run_native_build(clang, p, name, ticks, harness::build_restoring(p, machine, ticks, inputs, payload))
}

/// Uebersetzt Programm und Rahmen, laeuft und liefert den Trace.
///
/// Jeder Aufruf baut in seinem eigenen Verzeichnis: Tests desselben Binarys
/// laufen parallel ueber dieselben Programme mit verschiedenen Rahmen, und
/// ein geteiltes Verzeichnis raeumte dem einen die Dateien des anderen weg
/// (KON1-007).
fn run_native_build(clang: &Clang, p: &Program, name: &str, ticks: u64, h: harness::Harness) -> Result<String, String> {
    run_built(clang, p, name, ticks, h, None)
}

/// Wie die anderen Laeufe, unter AddressSanitizer (FB-461): erzeugter Code
/// und Rahmen mit Schutzzonen um jeden Platz; ein Zugriff daneben bricht
/// den Lauf mit dem Bericht des Sanitizers ab. `search` ist der Suchpfad
/// des Laufs ([`asan_path`]).
pub fn run_native_sanitized(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    h: harness::Harness,
    search: &std::ffi::OsStr,
) -> Result<String, String> {
    run_built(clang, p, name, ticks, h, Some(search))
}

/// Der Suchpfad fuer einen Lauf unter AddressSanitizer: Unter Windows laedt
/// das Programm die Laufzeit als DLL aus dem Verzeichnis von clang. `None`,
/// wenn sie dort fehlt.
pub fn asan_path(clang: &Clang) -> Option<std::ffi::OsString> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    if !cfg!(windows) {
        return Some(current);
    }
    let out = std::process::Command::new(clang.path()?).arg("-print-resource-dir").output().ok()?;
    let dir = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).join("lib/windows");
    dir.join("clang_rt.asan_dynamic-x86_64.dll").is_file().then_some(())?;
    std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&current))).ok()
}

/// Der gemeinsame Rumpf der Laeufe; `asan` ist der Suchpfad eines Laufs
/// unter AddressSanitizer.
fn run_built(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    h: harness::Harness,
    asan: Option<&std::ffi::OsStr>,
) -> Result<String, String> {
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "takt-abnahme-{}-{}-{run}",
        name.replace(['.', '/'], "_"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    let ir = if asan.is_some() { ir_sanitized(p) } else { ir_of(p) };
    std::fs::write(&ll, ir).map_err(|e| e.to_string())?;
    std::fs::write(&c, &h.source).map_err(|e| e.to_string())?;
    let path = clang.path().ok_or("clang")?;
    let natives = harness::native_library()?;
    let mut cmd = std::process::Command::new(path);
    Clang::deterministic(&mut cmd).args(["-Wno-override-module", "-O1"]);
    if asan.is_some() {
        cmd.arg("-fsanitize=address");
    }
    let build = cmd.arg(&ll).arg(&c).arg(&natives).arg("-o").arg(&exe).output().map_err(|e| e.to_string())?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).to_string());
    }
    let mut run = std::process::Command::new(&exe);
    if let Some(search) = asan {
        run.env("PATH", search);
    }
    let out = run.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    if asan.is_some() && !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).lines().take(24).collect::<Vec<_>>().join("\n"));
    }
    finished(&out.status, text).and_then(|text| reaches(text, ticks))
}

/// Der Lauf reichte bis zum letzten Tick (KON1-010): Der Rahmen nennt am
/// Ende, bis wohin er kam (`takt end <tick>`), nach der Schleife `ticks + 1`.
/// Endet das Programm frueher selbst (12.7), steht das Ende in diesem Tick.
pub fn reaches(text: String, ticks: u64) -> Result<String, String> {
    let Some(last) = text.lines().find_map(|l| l.strip_prefix("takt end ")?.trim().parse::<u64>().ok()) else {
        return Err(format!("der Lauf endete ohne `takt end`:\n{text}"));
    };
    let ended = text.lines().any(|l| l.starts_with(&format!("t={last} end ")));
    if last == ticks + 1 || ended {
        Ok(text)
    } else {
        Err(format!("der Lauf endete in Tick {last} von {ticks} ohne Ende des Programms:\n{text}"))
    }
}

/// Ein Lauf, der abbricht, liefert einen abgeschnittenen Trace, und der
/// Vergleich prueft nur, was beide Seiten melden (FB-305): Der Abbruch
/// ist der Befund, nicht die Zeilen davor.
pub fn finished(status: &std::process::ExitStatus, text: String) -> Result<String, String> {
    if status.success() {
        return Ok(text);
    }
    let tail: Vec<&str> = text.lines().rev().take(4).collect();
    Err(format!("der Lauf brach ab ({status}); zuletzt:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
}
