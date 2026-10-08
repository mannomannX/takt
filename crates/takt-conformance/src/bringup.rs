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
    let path = hardware_path()?;
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Some(takt_mir::hardware::parse(&text).unwrap_or_else(|e| panic!("{path}:{}: {}", e.line, e.message)))
}

/// Der Pfad aus `TAKT_HARDWARE`, fuer den Bauhelfer (`takt_embed::build`).
pub fn hardware_path() -> Option<String> {
    println!("cargo:rerun-if-env-changed=TAKT_HARDWARE");
    let path = env::var("TAKT_HARDWARE").ok()?;
    println!("cargo:rerun-if-changed={path}");
    Some(path)
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
        build: build(),
        profile: None,
        channel_imports,
        core: None,
    };
    let checked = takt_sema::compile(&src, &options);
    if checked.program.is_none() {
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("cargo:warning={path}: {d}");
        }
    }
    checked.program
}

/// Der Build aus `TAKT_BUILD` (8.3): Der Board-Vergleich verlangt `sim`,
/// damit Plant-Modelle mitlaufen; ohne Angabe baut ein Bring-up fuer die
/// Hardware. Rahmen ([`compile`]) und Codegen ([`takt_build`]) lesen ihn
/// beide hier, sonst ruft der Rahmen Maschinen, die der Codegen ausliess.
fn build() -> takt_sema::Build {
    println!("cargo:rerun-if-env-changed=TAKT_BUILD");
    match env::var("TAKT_BUILD").as_deref() {
        Ok("sim") => takt_sema::Build::Sim,
        _ => takt_sema::Build::Hw,
    }
}

/// Der Build aus `TAKT_BUILD` als Wort fuer den Bauhelfer
/// (`takt_embed::build::Program::build_for`), derselbe wie in [`compile`].
pub fn build_name() -> &'static str {
    if build() == takt_sema::Build::Sim { "sim" } else { "hw" }
}

/// Schreibt die Arena des Programms als Rust-Typ nach `file` (12.11): so gross
/// und so ausgerichtet, wie der Uebersetzer den Rahmen fuer das Ziel `triple`
/// mit `flags` legt ([`takt_frame::mcu::arena_layout`]). Das Bring-up legt sie
/// an und reicht sie jedem Einstieg.
pub fn arena(frame: &takt_frame::mcu::McuHarness, triple: &str, flags: &[&str], file: &Path) {
    let x = takt_llvm::symbols::Prefix::default();
    let (bytes, align) =
        takt_frame::mcu::arena_layout(frame, &x, triple, flags).unwrap_or_else(|e| panic!("Arena nicht bemessen: {e}"));
    fs::write(file, takt_frame::mcu::rust_arena(bytes, align)).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

/// Der Name der Verdrahtung eines Treiber-Crates oder Bring-ups (12.6): je
/// Adresse der Rust-Typ, der sie bedient ([`takt_frame::drivers::wiring`]).
pub const WIRING: &str = "takt-drivers.toml";

/// Liest die Verdrahtungen aus `files` fuer das Build-Skript
/// ([`read_wiring`]); ein Fehler bricht den Bau ab.
pub fn wiring(files: &[&Path]) -> Vec<(String, String)> {
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    read_wiring(files).unwrap_or_else(|e| panic!("{e}"))
}

/// Die Verdrahtungen aus `files`, in ihrer Reihenfolge zusammengefuehrt.
/// Genau ein Geraet bedient eine Adresse (12.6): Nennen zwei Dateien
/// dieselbe, etwa Bring-up und Treiber-Crate, ist das ein Fehler mit beiden
/// Dateien und kein Vorrang der ersten.
pub fn read_wiring(files: &[&Path]) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut from: Vec<&Path> = Vec::new();
    for file in files {
        let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        for (address, ty) in takt_frame::drivers::wiring(&text).map_err(|e| format!("{}: {e}", file.display()))? {
            if let Some(i) = out.iter().position(|(a, _)| *a == address) {
                return Err(format!(
                    "{}: `{address}` verdrahtet schon {}; genau ein Geraet bedient eine Adresse (12.6)",
                    file.display(),
                    from[i].display()
                ));
            }
            out.push((address, ty));
            from.push(file);
        }
    }
    Ok(out)
}

/// Schreibt die Treiber des Programms fuer einen Pruefstand nach `file`
/// (12.6): den Trait `Drivers`, den Pruefstand `Rig` nach `wiring` und den
/// Kleber vom Rahmen zu ihm, mit dem Praefix der Bring-ups. Eine Adresse
/// ohne Geraet bekommt einen Stummel, den die Dokumentation von `Rig` nennt;
/// wer ihn nicht verlangt, prueft die Verdrahtung vorher
/// ([`takt_frame::drivers::wiring_errors`], so `board::host` mit
/// Treiber-Crate).
pub fn drivers(p: &takt_mir::Program, wiring: &[(String, String)], file: &Path) {
    use takt_frame::drivers::{of, rust_glue, rust_rig, rust_trait};
    let list = of(p, &takt_frame::layout::of(p));
    let x = takt_llvm::symbols::Prefix::default();
    let text = format!(
        "// Erzeugt von `takt_conformance::bringup::drivers` (12.6).\n\n{}\n{}\n{}",
        rust_trait(&list),
        rust_rig(&list, "Rig", wiring),
        rust_glue(&list, &x, "Rig")
    );
    fs::write(file, text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

/// Schreibt den Pruefstand `Rig` des Programms nach `file` (12.6): je Adresse
/// das Geraet aus `wiring`, sonst ein Stummel, den die Dokumentation von
/// `Rig` nennt. Die Traits `Drivers` und `Sys` und den Kleber dazu bringt das
/// Modul der Lieferform (`P.rs`); hier steht er im Modul `crate::app`.
pub fn rig(p: &takt_mir::Program, wiring: &[(String, String)], file: &Path) {
    use takt_frame::drivers::{Kind, of, rust_rig};
    let list = of(p, &takt_frame::layout::of(p));
    let traits = if list.iter().any(|d| d.kind == Kind::Sys) { "{Drivers, Sys}" } else { "Drivers" };
    let text = format!(
        "// Erzeugt von `takt_conformance::bringup::rig` (12.6).\n\nuse crate::app::{traits};\n\n{}",
        rust_rig(&list, "Rig", wiring)
    );
    fs::write(file, text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
}

/// Ruft `takt build PROGRAMM --target ZIEL --build BUILD --prefix app` mit
/// `extra`, etwa `--emit ir`, und schreibt nach `out`; `BUILD` aus [`build`].
///
/// **Der Umweg ueber die Kommandozeile ist Absicht.** Ein Build-Skript,
/// das `takt_llvm::lower` direkt ruft, uebersetzt anders als das Werkzeug —
/// nicht heute, aber beim naechsten Schalter, den nur eines von beiden
/// bekommt (FB-138).
pub fn takt_build(program: &str, target: &str, extra: &[&str], out: &Path) {
    let takt = takt();
    let status = Command::new(&takt)
        .args([
            "build",
            program,
            "--target",
            target,
            "--build",
            if build() == takt_sema::Build::Sim { "sim" } else { "hw" },
        ])
        // Das Praefix des Rahmens der Bring-ups (`takt_frame::mcu::Frame`, 12.11).
        .args(["--prefix", takt_llvm::symbols::Prefix::default().as_str()])
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

/// Bindet die Objekte zur Bibliothek `name` in `out` und meldet sie dem
/// Linker. Der Dateiname folgt dem Ziel: `name.lib` fuer MSVC, sonst
/// `libname.a`.
pub fn archive(out: &Path, name: &str, objs: &[&Path]) {
    let msvc = env::var("TARGET").is_ok_and(|t| t.ends_with("-msvc"));
    let lib = out.join(if msvc { format!("{name}.lib") } else { format!("lib{name}.a") });
    let _ = fs::remove_file(&lib);
    let ar = clang().with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar).arg("crs").arg(&lib).args(objs).status().is_ok_and(|s| s.success());
    assert!(ok, "llvm-ar schlug fehl; `{name}` waere nicht gebunden");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static={name}");
}

/// Das Werkzeug `takt` aus dem Zielverzeichnis dieses Baus, im selben
/// Profil, und nicht aelter als der Compiler.
///
/// **Nicht ueber `CARGO_BIN_EXE_takt`**: Die Variable gibt es nur fuer
/// Binaries desselben Crates. Und nicht ueber den PATH, weil dort ein
/// fremdes `takt` stehen koennte. **Dasselbe Profil, nicht das neueste**:
/// Welches Binary gerade juenger ist, entschiede sonst, wer zuletzt
/// `cargo test` gerufen hat. Baut der Harness in einem zweiten
/// Zielverzeichnis ([`crate::board::BUILDS`]), nennt er das Werkzeug des
/// ersten ausdruecklich in [`TOOL`].
pub fn takt() -> PathBuf {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    println!("cargo:rerun-if-env-changed={TOOL}");
    let found = match env::var_os(TOOL) {
        Some(tool) => Some(PathBuf::from(tool)).filter(|p| p.exists()),
        // `OUT_DIR` ist `<target>/[<triple>/]<profil>/build/<crate>-<hash>/out`;
        // die CLI liegt fuer den Wirt gebaut, also ohne Triple.
        None => {
            let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
            out.ancestors().skip(1).take(6).map(|dir| dir.join(&profile).join(exe)).find(|p| p.exists())
        }
    };
    let Some(takt) = found else { panic!("Das Werkzeug takt fehlt; erst `{}`", build_command(&profile)) };
    // Auch das Werkzeug ist eine Quelle: Ohne diese Zeile baute Cargo nach
    // einer Aenderung am Compiler nicht neu.
    println!("cargo:rerun-if-changed={}", takt.display());
    assert_fresh(&takt, &profile);
    takt
}

/// Die Umgebungsvariable, die das Werkzeug ausdruecklich nennt ([`takt`]).
pub const TOOL: &str = "TAKT_TOOL";

/// Wie das Werkzeug fuer `profile` entsteht; `dev` und `debug` sind Cargos Vorgabe.
fn build_command(profile: &str) -> String {
    if profile == "release" { "cargo build -p takt-cli --release".into() } else { "cargo build -p takt-cli".into() }
}

/// Ein `takt`, das aelter ist als der Compiler, baut stillschweigend das
/// Objekt von gestern (FB-193). Der Vergleich ist grob — Aenderungszeit
/// gegen jede Quelle der Crates, aus denen das Werkzeug entsteht
/// ([`tool_crates`]) —, aber er faellt genau dann, wenn es darauf ankommt.
fn assert_fresh(takt: &Path, profile: &str) {
    let Ok(built) = fs::metadata(takt).and_then(|m| m.modified()) else { return };
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for krate in tool_crates() {
        walk(&krate.join("src"), &mut newest);
    }
    if let Some((t, file)) = newest
        && t > built
    {
        panic!(
            "{} ist aelter als {}; `{}` vor dem Bring-up (FB-193)",
            takt.display(),
            file.display(),
            build_command(profile)
        );
    }
}

/// Die Crates, aus denen das Werkzeug entsteht: `takt-cli` und jede
/// Pfadabhaengigkeit fuer den Bau, transitiv aus den `Cargo.toml` gelesen.
/// Eine Liste von Hand vergass `takt-frame`, und ein altes `takt` schrieb
/// die Huelle von gestern (FB-444).
pub fn tool_crates() -> Vec<PathBuf> {
    let canonical = |p: PathBuf| fs::canonicalize(&p).unwrap_or(p);
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut todo = vec![canonical(Path::new(env!("CARGO_MANIFEST_DIR")).join("../takt-cli"))];
    while let Some(dir) = todo.pop() {
        if seen.contains(&dir) {
            continue;
        }
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        todo.extend(path_dependencies(&manifest).into_iter().map(|p| canonical(dir.join(p))));
        seen.push(dir);
    }
    seen
}

/// Die Pfade der Abhaengigkeiten eines `Cargo.toml`, ohne die zum Testen
/// (`[dev-dependencies]`): Was ein Test braucht, steckt nicht im Werkzeug.
fn path_dependencies(manifest: &str) -> Vec<String> {
    let mut section = "";
    let mut out = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        if !section.contains("dependencies") || section.contains("dev-dependencies") {
            continue;
        }
        if let Some(path) = line.split("path = \"").nth(1).and_then(|rest| rest.split('"').next()) {
            out.push(path.to_string());
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::{path_dependencies, read_wiring, tool_crates};

    /// Die Pfade der Abhaengigkeiten fuer den Bau, auch als eigene Tabelle;
    /// die zum Testen nicht (FB-444).
    #[test]
    fn the_path_dependencies_of_a_manifest_leave_out_the_dev_ones() {
        let manifest = "[package]\nname = \"x\"\n\n[dependencies]\na = { path = \"../a\" }\nserde = \"1\"\n\
                        \n[build-dependencies]\nb = { path = \"../b\", default-features = false }\n\
                        \n[dependencies.c]\npath = \"../c\"\n\n[dev-dependencies]\nd = { path = \"../d\" }\n";
        assert_eq!(path_dependencies(manifest), ["../a", "../b", "../c"]);
    }

    /// Das Werkzeug entsteht aus `takt-frame` und den Compiler-Crates, nicht
    /// aus dem, was nur seine Tests brauchen (FB-444).
    #[test]
    fn the_tool_is_built_from_the_frame_and_not_from_the_test_kit() {
        let names: Vec<String> =
            tool_crates().iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
        for wanted in ["takt-cli", "takt-frame", "takt-llvm", "takt-sema", "takt-mir"] {
            assert!(names.iter().any(|n| n == wanted), "{wanted} fehlt: {names:?}");
        }
        assert!(!names.iter().any(|n| n == "takt-testkit"), "{names:?}");
    }

    /// **Zwei Dateien verdrahten eine Adresse: ein Fehler** (12.6, GEN-021),
    /// der beide Dateien und die Adresse nennt, gleich in welcher
    /// Reihenfolge; jede Datei fuer sich ist gueltig.
    #[test]
    fn a_second_file_cannot_wire_an_address_again() {
        let dir = crate::target_dir().join(format!("takt-wiring-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        let (bringup, driver) = (dir.join("bringup.toml"), dir.join("driver.toml"));
        std::fs::write(&bringup, "\"sys/previous_run\" = \"Host\"\n").expect("schreibbar");
        std::fs::write(&driver, "\"edge_o\" = \"O\"\n\"sys/previous_run\" = \"Crate\"\n").expect("schreibbar");
        for files in [[&bringup, &driver], [&driver, &bringup]] {
            let e = read_wiring(&[files[0].as_path(), files[1].as_path()]).expect_err("doppelt");
            assert!(e.contains("`sys/previous_run`") && e.contains("bringup.toml") && e.contains("driver.toml"), "{e}");
        }
        assert_eq!(read_wiring(&[driver.as_path()]).map(|w| w.len()), Ok(2));
        assert!(read_wiring(&[dir.join("fehlt.toml").as_path()]).is_err_and(|e| e.contains("fehlt.toml")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
