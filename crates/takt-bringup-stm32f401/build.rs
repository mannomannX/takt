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
//! **Welches Programm?** `TAKT_PROGRAM` nennt den Pfad; ohne die Variable
//! nimmt der Bau `corpus-try/16_timing.takt`. Ein Default ist hier
//! richtig, weil das Binary sonst gar nicht baut und der Fehler dann wie
//! ein Defekt aussaehe statt wie eine fehlende Angabe.

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
/// Scheitert es, bleibt es bei einer Warnung: Die anderen Binaries
/// (`blink`, `minimal`) brauchen das Programm nicht, und ein harter
/// Abbruch machte sie mit unbaubar.
fn build_takt_program(out: &Path) {
    let program = env::var("TAKT_PROGRAM").unwrap_or_else(|_| {
        format!("{}/../../corpus-try/16_timing.takt", env!("CARGO_MANIFEST_DIR"))
    });
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    println!("cargo:rerun-if-changed={program}");

    let Ok(src) = fs::read_to_string(&program) else {
        println!("cargo:warning=Takt-Programm nicht lesbar: {program}");
        return;
    };

    // Uebersetzen und senken — dieselbe Folge, die die Abnahme nimmt.
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Hw, profile: None };
    let checked = takt_sema::compile(&src, &options);
    let Some(p) = checked.program else {
        for d in checked.diagnostics.iter().filter(|d| d.is_error()) {
            println!("cargo:warning={program}: {d}");
        }
        return;
    };

    let target = takt_llvm::Target::THUMBV7EM;
    let lowered = takt_llvm::lower::program(&p, target.triple, "takt");
    for s in &lowered.skipped {
        // Ein fehlender Schritt ist keine Warnung, sondern ein Loch: Der
        // Linker meldete sonst nur ein unbekanntes Symbol.
        println!("cargo:warning={}_step fehlt: {}", s.machine, s.reason);
    }

    let ll = out.join("takt.ll");
    let rahmen = out.join("takt_rahmen.c");
    fs::write(&ll, &lowered.ir).expect("IR schreiben");
    fs::write(&rahmen, takt_conformance::mcu::build(&p).source).expect("Rahmen schreiben");

    // Beides uebersetzen und zu einem Objekt binden. `clang` steht in
    // `takt-llvm::toolchain`, wo auch die Abnahme es sucht.
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        println!("cargo:warning=clang fehlt; das Takt-Programm wird nicht gebunden");
        return;
    };
    let obj = out.join("takt_programm.o");
    let obj_rahmen = out.join("takt_rahmen.o");
    for (src, dst) in [(&ll, &obj), (&rahmen, &obj_rahmen)] {
        let ok = Command::new(&clang)
            .args(["-Wno-override-module", "-O2", "-c", "-ffreestanding", "-nostdlib"])
            .arg(format!("--target={}", target.triple))
            .arg(src)
            .arg("-o")
            .arg(dst)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            println!("cargo:warning={}: uebersetzt nicht", src.display());
            return;
        }
    }

    // Als statische Bibliothek, damit `rustc` sie wie jede andere
    // einbindet. Ein loses Objekt ginge auch, aber `cargo` kennt den Weg
    // ueber `-l static=` und raeumt ihn mit auf.
    let lib = out.join("libtaktprogramm.a");
    let _ = fs::remove_file(&lib);
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar)
        .arg("crs")
        .arg(&lib)
        .arg(&obj)
        .arg(&obj_rahmen)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        println!("cargo:warning=llvm-ar fehlt; das Takt-Programm wird nicht gebunden");
        return;
    }

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=taktprogramm");
}
