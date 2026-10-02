//! Der Bau eines Bring-ups aus seinem `build.rs` (12.1): das Programm
//! uebersetzen, den Rahmen dazu erzeugen, beides als Bibliothek binden.
//!
//! Jedes Bring-up tut dasselbe und unterscheidet sich nur im Ziel und in
//! dessen Flags; was gleich ist, steht hier einmal. Die Funktionen laufen
//! im Build-Skript und schreiben ihre Zeilen `cargo:` an Cargo. Wer etwas
//! nicht bauen kann, bricht den Bau mit einer Meldung ab: Ein Binary, das
//! reproduzierbar sein soll (13.8), darf nicht von einem Objekt abhaengen,
//! das zufaellig noch im Zielverzeichnis liegt.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Die Hardware-Konfiguration aus `TAKT_HARDWARE` (8.10): Sie gibt jedem
/// geplanten Output sein `guard` (7.5). Ohne sie ist es null wie in der
/// Simulation — ein Konformitaetslauf vergleicht mit dem Interpreter.
pub fn hardware() -> Option<takt_mir::hardware::Hardware> {
    println!("cargo:rerun-if-env-changed=TAKT_HARDWARE");
    let path = env::var("TAKT_HARDWARE").ok()?;
    println!("cargo:rerun-if-changed={path}");
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Some(takt_mir::hardware::parse(&text).unwrap_or_else(|e| panic!("{path}:{}: {}", e.line, e.message)))
}

/// Uebersetzt das Programm fuer die Hardware, um den Rahmen dazu bauen zu
/// koennen; die Konfigurationen aus `import channels` liegen neben ihm
/// (8.2). Fehler gehen als Warnungen an Cargo.
pub fn compile(path: &str) -> Option<takt_mir::Program> {
    println!("cargo:rerun-if-changed={path}");
    let src = fs::read_to_string(path).ok()?;
    let dir = Path::new(path).parent().unwrap_or(Path::new("."));
    let channel_imports = takt_sema::channel_imports(&src)
        .into_iter()
        .filter_map(|file| {
            let at = dir.join(&file);
            println!("cargo:rerun-if-changed={}", at.display());
            Some((file, fs::read_to_string(at).ok()?))
        })
        .collect();
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Hw,
        profile: None,
        channel_imports,
    };
    let checked = takt_sema::compile(&src, &options);
    if checked.program.is_none() {
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("cargo:warning={path}: {d}");
        }
    }
    checked.program
}

/// Ruft `takt build PROGRAMM --target ZIEL --build hw` mit `extra`, etwa
/// `--emit ir`, und schreibt nach `out`.
///
/// **Der Umweg ueber die Kommandozeile ist Absicht.** Ein Build-Skript,
/// das `takt_llvm::lower` direkt ruft, uebersetzt anders als das Werkzeug —
/// nicht heute, aber beim naechsten Schalter, den nur eines von beiden
/// bekommt (FB-138).
pub fn takt_build(program: &str, target: &str, extra: &[&str], out: &Path) {
    let takt = takt();
    let status = Command::new(&takt)
        .args(["build", program, "--target", target, "--build", "hw"])
        .args(extra)
        .arg("--out")
        .arg(out)
        .status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => panic!("takt build {} schlug fehl fuer {program}", extra.join(" ")),
        Err(e) => panic!("{} nicht aufrufbar: {e}", takt.display()),
    }
}

/// Wo clang steckt — dieselbe Suche wie `takt-llvm::toolchain`.
pub fn clang() -> PathBuf {
    match takt_llvm::toolchain::find() {
        takt_llvm::toolchain::Clang::At(p) => p,
        takt_llvm::toolchain::Clang::Missing => panic!("clang fehlt; ohne ihn entsteht kein Programm"),
    }
}

/// Bindet die Objekte zur Bibliothek `taktprogramm` in `out` und meldet sie
/// dem Linker. Der Name folgt dem Ziel: `taktprogramm.lib` fuer MSVC,
/// sonst `libtaktprogramm.a`.
pub fn archive(out: &Path, objs: &[&Path]) {
    let msvc = env::var("TARGET").is_ok_and(|t| t.ends_with("-msvc"));
    let lib = out.join(if msvc { "taktprogramm.lib" } else { "libtaktprogramm.a" });
    let _ = fs::remove_file(&lib);
    let ar = clang().with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar).arg("crs").arg(&lib).args(objs).status().is_ok_and(|s| s.success());
    assert!(ok, "llvm-ar schlug fehl; das Takt-Programm waere nicht gebunden");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=taktprogramm");
}

/// Das Werkzeug `takt` aus dem Zielverzeichnis dieses Baus, im selben
/// Profil, und nicht aelter als der Compiler.
///
/// **Nicht ueber `CARGO_BIN_EXE_takt`**: Die Variable gibt es nur fuer
/// Binaries desselben Crates. Und nicht ueber den PATH, weil dort ein
/// fremdes `takt` stehen koennte. **Dasselbe Profil, nicht das neueste**:
/// Welches Binary gerade juenger ist, entschiede sonst, wer zuletzt
/// `cargo test` gerufen hat.
pub fn takt() -> PathBuf {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    // `OUT_DIR` ist `<target>/[<triple>/]<profil>/build/<crate>-<hash>/out`;
    // die CLI liegt fuer den Wirt gebaut, also ohne Triple.
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let found = out.ancestors().skip(1).take(6).map(|dir| dir.join(&profile).join(exe)).find(|p| p.exists());
    let Some(takt) = found else { panic!("Das Werkzeug takt fehlt; erst `cargo build -p takt-cli --{profile}`") };
    // Auch das Werkzeug ist eine Quelle: Ohne diese Zeile baute Cargo nach
    // einer Aenderung am Compiler nicht neu.
    println!("cargo:rerun-if-changed={}", takt.display());
    assert_fresh(&takt);
    takt
}

/// Ein `takt`, das aelter ist als der Compiler, baut stillschweigend das
/// Objekt von gestern (FB-193). Der Vergleich ist grob — Aenderungszeit
/// gegen jede Quelle der Compiler-Crates —, aber er faellt genau dann,
/// wenn es darauf ankommt.
fn assert_fresh(takt: &Path) {
    let Ok(built) = fs::metadata(takt).and_then(|m| m.modified()) else { return };
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for krate in ["takt-syntax", "takt-diag", "takt-mir", "takt-sema", "takt-interp", "takt-llvm", "takt-cli"] {
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(krate).join("src"), &mut newest);
    }
    if let Some((t, file)) = newest
        && t > built
    {
        panic!(
            "{} ist aelter als {}; `cargo build -p takt-cli --release` vor dem Bring-up (FB-193)",
            takt.display(),
            file.display()
        );
    }
}

fn walk(dir: &Path, newest: &mut Option<(SystemTime, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            walk(&path, newest);
        } else if path.extension().is_some_and(|x| x == "rs")
            && let Ok(t) = fs::metadata(&path).and_then(|m| m.modified())
            && newest.as_ref().is_none_or(|(n, _)| t > *n)
        {
            *newest = Some((t, path));
        }
    }
}
