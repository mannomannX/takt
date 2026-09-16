//! Legt `memory.x` bereit und uebersetzt das Takt-Programm mit.
//!
//! **Zwei Aufgaben, beide mit demselben Grund: Der Linker muss finden,
//! was er braucht.**
//!
//! `link.x` aus `cortex-m-rt` enthaelt ein `INCLUDE memory.x`, und der
//! Linker sucht die Datei in seinen `-L`-Pfaden. Baut man aus dem
//! Crate-Verzeichnis, findet er sie ueber das Arbeitsverzeichnis und
//! alles scheint zu gehen; von aussen gebaut bricht er ab oder, schlimmer,
//! zieht eine fremde `memory.x` — dann laege die Anwendung bei
//! 0x0800_0000 und ueberschriebe beim ersten Flashen den Bootloader.
//!
//! Dasselbe gilt fuer das Takt-Programm: Das Binary `takt` ruft
//! `takt_mcu_init` und `takt_mcu_tick`, und die entstehen erst, wenn eine
//! `.takt`-Datei uebersetzt und der MCU-Rahmen erzeugt wurde. Beides
//! passiert hier, damit `cargo build` genuegt.
//!
//! **Welches Programm?** `takt.toml` neben `Cargo.toml` nennt den Pfad
//! (FB-141); `TAKT_PROGRAM` sticht nur fuer einen einmaligen Versuch.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).expect("memory.x schreiben");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // **Die Linker-Argumente gehoeren hierher, nicht in
    // `.cargo/config.toml`.** Jene Datei gilt nur, wenn `cargo` aus
    // diesem Verzeichnis laeuft. Von aussen gebaut fehlte `-Tlink.x`, und
    // es entstuende eine Binaerdatei ohne Vektortabelle: Sie baut, sie
    // linkt, und sie ist unbrauchbar.
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=--nmagic");

    build_takt_program(&out);
}

/// Uebersetzt das Takt-Programm und bindet es als Objekt ein.
///
/// **Scheitert es, scheitert der Bau.** Eine erste Fassung begnuegte sich
/// mit einer Warnung, weil `blink` und `minimal` das Programm nicht
/// brauchen — und genau das verdeckte einen Ausfall: Das Build-Skript lief
/// nicht mehr, niemand sah die Warnung im Rauschen, und gebaut wurde
/// weiter gegen ein Objekt aus einem frueheren Lauf. Ein Binary, das
/// reproduzierbar sein soll (13.8), darf nicht davon abhaengen, was
/// zufaellig noch im Zielverzeichnis liegt.
///
/// `blink` und `minimal` bleiben baubar: Sie binden weder die Bibliothek
/// noch die erzeugten Konstanten ein.
fn build_takt_program(out: &Path) {
    let program = program_path();
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    println!("cargo:rerun-if-changed={program}");

    if !Path::new(&program).exists() {
        panic!("Takt-Programm nicht gefunden: {program}");
    }

    // Der Rahmen (12.1) entsteht hier, weil er an der Speicherform in
    // `takt-conformance` haengt; `takt build` uebersetzt das Programm.
    let Some(p) = compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let rahmen = out.join("takt_rahmen.c");
    if let Err(e) = fs::write(&rahmen, takt_conformance::mcu::build(&p).source) {
        panic!("Rahmen nicht schreibbar: {e}");
    }

    let obj = out.join("takt_programm.o");
    run_takt_build(&program, &["--emit", "obj"], &obj);

    // Die Tickperiode und die Ausgangsindizes kommen aus dem Programm,
    // nicht aus der Hand: Beide standen schon einmal doppelt, und die
    // logische Zeit lief darum zehnfach zu schnell.
    run_takt_build(&program, &["--emit", "consts-rs"], &out.join("takt_consts.rs"));

    let obj_rahmen = out.join("takt_rahmen.o");
    compile_c(&rahmen, &obj_rahmen);
    archive(out, &[&obj, &obj_rahmen]);

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=taktprogramm");
}

/// Welches Programm gebaut wird.
///
/// **Aus `takt.toml`, nicht aus der Umgebung.** Der Pfad bestimmt den
/// Inhalt des Binaries, und eine Umgebungsvariable taete das unsichtbar:
/// Einem fertigen Binary sieht man nicht an, mit welchem Wert es entstand.
/// Ein Bau ohne `TAKT_PROGRAM` fiel darum still auf einen Default zurueck
/// und erzeugte ein Programm, das auf dem Board korrekt lief und dabei
/// dunkel blieb — es hing an einem Kommando, das dort niemand sendet
/// (FB-141).
///
/// `TAKT_PROGRAM` sticht weiterhin: fuer einen einmaligen Versuch, nicht
/// fuer den Normalfall.
fn program_path() -> String {
    if let Ok(p) = env::var("TAKT_PROGRAM") {
        return p;
    }
    let here = env!("CARGO_MANIFEST_DIR");
    let config = format!("{here}/takt.toml");
    println!("cargo:rerun-if-changed={config}");
    let text = fs::read_to_string(&config).unwrap_or_else(|e| panic!("{config}: {e}"));
    // Eine Zeile `program = "…"`; ein TOML-Parser waere fuer einen
    // Schluessel eine Abhaengigkeit zu viel.
    let value = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| l.strip_prefix("program")?.trim_start().strip_prefix('=')?.trim().strip_prefix('"')?.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{config}: kein `program = \"…\"`"));
    format!("{here}/{value}")
}

/// Uebersetzt das Programm, um den Rahmen dazu bauen zu koennen.
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

/// Ruft `takt build` — dasselbe Kommando, das ein Nutzer aufruft.
///
/// **Der Umweg ueber die Kommandozeile ist Absicht.** Eine `build.rs`,
/// die `takt_llvm::lower` direkt ruft, uebersetzt anders als das
/// Werkzeug — nicht heute, aber beim naechsten Schalter, den nur eines
/// von beiden bekommt. FB-138 hielt fest, dass die Pipeline in ein
/// Kommando gehoert; sie hier erneut zu schreiben hiesse, den Befund
/// abzuhaken und die Ursache zu behalten.
fn run_takt_build(program: &str, emit: &[&str], out: &Path) {
    let Some(takt) = find_takt() else {
        let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
        panic!("Das Werkzeug takt fehlt; erst `cargo build -p takt-cli --{profile}`");
    };
    // **Auch das Werkzeug ist eine Quelle.** Ohne diese Zeile kennt Cargo
    // nur die `.takt`-Datei und baut nicht neu, wenn sich der Compiler
    // geaendert hat — das erzeugte Objekt bliebe aus dem vorigen Stand.
    println!("cargo:rerun-if-changed={}", takt.display());
    let status = Command::new(&takt)
        .args(["build", program, "--target", "thumbv7em", "--build", "hw"])
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

/// Uebersetzt den C-Rahmen.
fn compile_c(src: &Path, obj: &Path) {
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn entsteht kein Rahmen") };
    let ok = Command::new(&clang)
        .args(["-O2", "-c", "-ffreestanding", "-nostdlib", "--target=thumbv7em-none-eabihf"])
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}

/// Bindet die Objekte zu einer statischen Bibliothek.
fn archive(out: &Path, objs: &[&Path]) {
    let lib = out.join("libtaktprogramm.a");
    let _ = fs::remove_file(&lib);
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn auch kein llvm-ar") };
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar).arg("crs").arg(&lib).args(objs).status().is_ok_and(|s| s.success());
    assert!(ok, "llvm-ar schlug fehl; das Takt-Programm waere nicht gebunden");
}

/// Wo clang steckt — dieselbe Suche wie `takt-llvm::toolchain`.
fn clang() -> Option<PathBuf> {
    match takt_llvm::toolchain::find() {
        takt_llvm::toolchain::Clang::At(p) => Some(p),
        takt_llvm::toolchain::Clang::Missing => None,
    }
}

/// Wo das Werkzeug `takt` liegt.
///
/// **Nicht ueber `CARGO_BIN_EXE_takt`**: Die Variable gibt es nur fuer
/// Binaries desselben Crates, und `takt-cli` ist ein anderes. Und nicht
/// ueber den PATH, weil dort ein fremdes `takt` stehen koennte — gesucht
/// wird im Zielverzeichnis dieses Baus.
///
/// **Dasselbe Profil, nicht das erstbeste und nicht das neueste.** Zwei
/// Fassungen lagen vorher daneben: Die erste probierte `debug` vor
/// `release` und nahm ein veraltetes Werkzeug, das einen neuen Schalter
/// nicht kannte. Die zweite nahm das juengste — und band den Bau damit an
/// einen Zufall, denn welches Binary gerade juenger ist, entscheidet, wer
/// zuletzt `cargo test` gerufen hat.
///
/// `PROFILE` beantwortet es ohne Raten: Ein Release-Bau uebersetzt mit dem
/// Release-Werkzeug. Findet sich keines, bricht der Bau ab und sagt, was
/// zu tun ist — besser als ein Objekt aus einem fremden Stand.
fn find_takt() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    // Dasselbe Profil wie dieser Bau: `PROFILE` ist `debug` oder `release`.
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    // `OUT_DIR` ist `<target>/<triple>/<profil>/build/<crate>-<hash>/out`;
    // die CLI liegt fuer den *Wirt* gebaut, also ohne Triple daneben.
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
