//! Was die Abnahmetests brauchen: bauen und ausfuehren.
//!
//! Jeder Test bindet dieses Modul einzeln ein, und keiner benutzt alles —
//! `dead_code` ist hier die Regel, nicht die Ausnahme.
#![allow(dead_code)]

pub mod board;

use takt_conformance::harness;
use takt_conformance::stimulus::Stimulus;
use takt_conformance::testhost::Run;
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
    let root = takt_conformance::repo_root().join("corpus-try");
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

/// Ein Lauf im Produktrahmen auf dem Wirt (M11 Schritt 9,
/// [`takt_conformance::testhost`]): alle Maschinen ausser den Szenarien,
/// ohne Eingaben.
pub fn run_native_all(clang: &Clang, p: &Program, name: &str, ticks: u64) -> Result<String, String> {
    run_product(clang, p, name, &Run { ticks, ..Run::default() })
}

/// Wie [`run_native_all`], mit Eingaben (12.5): Der Stimulus kommt ueber
/// die Treiber, Commands, Tunables und Faults ueber das SDK.
pub fn run_native_all_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_product(clang, p, name, &Run { ticks, stimulus: inputs, ..Run::default() })
}

/// Wie [`run_native_all`], mit einer Journal-Nutzlast beim Start (5.9).
pub fn run_native_persist(
    clang: &Clang,
    p: &Program,
    name: &str,
    ticks: u64,
    payload: &[u8],
) -> Result<String, String> {
    run_product(clang, p, name, &Run { ticks, payload, ..Run::default() })
}

/// Ein Szenario nativ (13.6): die laufenden Maschinen samt dem gewaehlten
/// Szenario, wie `takt test` sie fuehrt, mit dem Stimulus des Szenarios.
pub fn run_native_scenario_with(
    clang: &Clang,
    p: &Program,
    name: &str,
    scenario: &str,
    ticks: u64,
    inputs: &[Stimulus],
) -> Result<String, String> {
    run_product(clang, p, name, &Run { ticks, stimulus: inputs, scenario: Some(scenario), ..Run::default() })
}

/// Wie [`run_native_scenario_with`], ohne Eingaben.
pub fn run_native_scenario(
    clang: &Clang,
    p: &Program,
    name: &str,
    scenario: &str,
    ticks: u64,
) -> Result<String, String> {
    run_native_scenario_with(clang, p, name, scenario, ticks, &[])
}

/// Wie [`run_product`], unter AddressSanitizer (FB-461): erzeugter Code,
/// Rahmen und Wirt mit Schutzzonen um jeden Platz; ein Zugriff daneben
/// bricht den Lauf mit dem Bericht des Sanitizers ab. `search` ist der
/// Suchpfad des Laufs ([`asan_path`]).
pub fn run_native_sanitized(
    clang: &Clang,
    p: &Program,
    name: &str,
    run: &Run<'_>,
    search: &std::ffi::OsStr,
) -> Result<String, String> {
    run_in_own_dir(clang, p, name, run, Some(search))
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

/// Das SDK-Archiv des Testwirts, einmal je Prozess gebaut (12.11): der Kern
/// ohne Warten mit den Natives und den Jobs aus `takt-crypto`.
pub fn sdk() -> Result<std::path::PathBuf, String> {
    static SDK: std::sync::OnceLock<Result<std::path::PathBuf, String>> = std::sync::OnceLock::new();
    SDK.get_or_init(|| harness::sdk_library_with(&takt_conformance::testhost::SDK_FEATURES, None)).clone()
}

/// Ein Lauf im Produktrahmen auf dem Wirt (M11 Schritt 9,
/// [`takt_conformance::testhost`]).
pub fn run_product(clang: &Clang, p: &Program, name: &str, run: &Run<'_>) -> Result<String, String> {
    run_in_own_dir(clang, p, name, run, None)
}

/// Jeder Lauf baut in seinem eigenen Verzeichnis: Tests desselben Binarys
/// laufen parallel ueber dieselben Programme, und ein geteiltes Verzeichnis
/// raeumte dem einen die Dateien des anderen weg (KON1-007).
fn run_in_own_dir(
    clang: &Clang,
    p: &Program,
    name: &str,
    run: &Run<'_>,
    asan: Option<&std::ffi::OsStr>,
) -> Result<String, String> {
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "takt-wirt-{}-{}-{n}",
        name.replace(['.', '/'], "_"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let path = clang.path().ok_or("clang")?;
    let text = takt_conformance::testhost::run(path, &sdk()?, p, run, &dir, asan);
    let _ = std::fs::remove_dir_all(&dir);
    text
}
