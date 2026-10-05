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
//! `app_init` und `app_tick`, und die entstehen erst, wenn eine
//! `.takt`-Datei uebersetzt und der MCU-Rahmen erzeugt wurde. Beides
//! passiert hier, damit `cargo build` genuegt.
//!
//! **Welches Programm?** `takt.toml` neben `Cargo.toml` nennt den Pfad
//! (FB-141); `TAKT_PROGRAM` sticht nur fuer einen einmaligen Versuch.
//!
//! **Die Treiber** (12.6) erzeugt `bringup::drivers` als Pruefstand: fuer
//! `takt` nach der Verdrahtung des Boards und des Pruefgeraets, fuer
//! `bench` ganz aus Stummeln — der Messkern misst den Tick, nicht die
//! Peripherie.
//!
//! **Die C-Referenz fuer `takt bench`** (13.8) nennt `TAKT_BENCH_C`. Sie
//! wird mit denselben Flags uebersetzt wie der erzeugte Code — das
//! Verhaeltnis der Zeiten soll die Sprachen vergleichen, nicht die
//! Optimierungsstufen —, mit `-ffp-contract=off`, weil Takt nie
//! stillschweigend zu `fma` zusammenzieht (4.2), und mit `-fno-math-errno`:
//! Ohne sie wird `__builtin_fmaf` zum Bibliotheksaufruf statt zum Befehl,
//! und Takt kennt kein `errno` (FB-287).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_conformance::bringup;

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
    image_key();

    // 4.2, 12.11: die Fliesskomma-Umgebung vor dem Lauf verstellen, als
    // Pruefung, dass jeder Einstieg seine eigene herstellt.
    println!("cargo:rerun-if-env-changed=TAKT_HOSTILE_FPU");
    if env::var("TAKT_HOSTILE_FPU").is_ok() {
        println!("cargo:rustc-env=TAKT_HOSTILE_FPU=1");
    }

    build_takt_program(&out);
    native_vectors(&out);
    math_vectors(&out);
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
    let Some(p) = bringup::compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let rahmen = out.join("takt_rahmen.c");
    let protect = state_section(out, takt_llvm::arena::of(&p).bytes);
    let frame = takt_frame::mcu::build_with(
        &p,
        takt_frame::mcu::Frame {
            diagnostics: takt_llvm::Diagnostics::Ids,
            hardware: bringup::hardware().as_ref(),
            protect: Some(protect),
            prefix: takt_llvm::symbols::Prefix::default(),
            stubs: false,
        },
    );
    if let Err(e) = fs::write(&rahmen, &frame.source) {
        panic!("Rahmen nicht schreibbar: {e}");
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wiring =
        bringup::wiring(&[&here.join(bringup::WIRING), &here.join("../takt-driver-probe").join(bringup::WIRING)]);
    bringup::drivers(&p, &wiring, &out.join("takt_drivers.rs"));
    bringup::drivers(&p, &[], &out.join("takt_drivers_bench.rs"));
    bringup::arena(&frame, "thumbv7em-none-eabihf", &[], &out.join("takt_arena.rs"));

    let ir = out.join("takt_programm.ll");
    bringup::takt_build(&program, "thumbv7em", &["--emit", "ir"], &ir);
    bringup::takt_build(&program, "thumbv7em", &["--emit", "consts-rs"], &out.join("takt_consts.rs"));
    let (obj, obj_rahmen) = (out.join("takt_programm.o"), out.join("takt_rahmen.o"));
    translate(&ir, &obj, &[]);
    translate(&rahmen, &obj_rahmen, &[]);
    let reference = bench_reference(out);
    bringup::archive(out, &[&obj, &obj_rahmen, &reference]);
    println!("cargo:rustc-link-arg=--icf=all");
}

/// Der Schluessel des Baus (KON1-009) als Symbol ins ELF
/// (`takt_board_support::image_key`): Wer parallel in dasselbe
/// Zielverzeichnis baut, prueft an seiner Kopie, dass sie die eigene ist.
/// Ohne `TAKT_IMAGE_KEY` traegt das Abbild keinen.
fn image_key() {
    use takt_board_support::image_key::{SYMBOL, valid};
    println!("cargo:rerun-if-env-changed=TAKT_IMAGE_KEY");
    if let Ok(key) = env::var("TAKT_IMAGE_KEY") {
        assert!(valid(&key), "TAKT_IMAGE_KEY: 1 bis 32 Hexziffern erwartet, `{key}` gefunden");
        println!("cargo:rustc-link-arg=--defsym={SYMBOL}{key}=0");
    }
}

/// Die Arena als eigener Abschnitt am Anfang des RAM (12.3, 12.11,
/// `takt_state.x`). `program` ist die Groesse ihres Programmbereichs; die
/// MPU-Region deckt ihn in ganzen Achteln. Liefert, wie weit der Rahmen
/// ihn auffuellt: bis ans Ende dieser Achtel, damit die Runtime dahinter
/// ungeschuetzt bleibt.
fn state_section(out: &Path, program: u64) -> u64 {
    const RAM_ORIGIN: u32 = 0x2000_0000;
    println!("cargo:rerun-if-changed=takt_state.x");
    let bytes = u32::try_from(program).unwrap_or_else(|_| panic!("Programmbereich {program} Byte"));
    let region = takt_board_support::mpu::Region::covering(RAM_ORIGIN, bytes)
        .expect("ORIGIN(RAM) ist fuer jede Region ausgerichtet");
    fs::write(out.join("takt_state.x"), include_bytes!("takt_state.x")).expect("takt_state.x schreiben");
    println!("cargo:rustc-link-arg=--defsym=__takt_state_size={}", region.protected());
    println!("cargo:rustc-link-arg=-Ttakt_state.x");
    u64::from(region.protected())
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
        .find_map(|l| {
            l.strip_prefix("program")?.trim_start().strip_prefix('=')?.trim().strip_prefix('"')?.strip_suffix('"')
        })
        .unwrap_or_else(|| panic!("{config}: kein `program = \"…\"`"));
    format!("{here}/{value}")
}

/// Die C-Referenz fuer `takt bench` als Objekt: die Datei aus
/// `TAKT_BENCH_C` oder schwache Definitionen, die niemand ruft, solange
/// `bench_reference.rs` sagt, dass keine da ist.
fn bench_reference(out: &Path) -> PathBuf {
    println!("cargo:rerun-if-env-changed=TAKT_BENCH_C");
    let given = env::var("TAKT_BENCH_C").ok();
    let src = match &given {
        Some(path) => {
            println!("cargo:rerun-if-changed={path}");
            PathBuf::from(path)
        }
        None => {
            let stub = out.join("bench_reference_none.c");
            let text = "__attribute__((weak)) void takt_bench_reference(void) {}\n\
                        __attribute__((weak)) unsigned long long takt_bench_reference_digest(void) { return 0; }\n";
            fs::write(&stub, text).expect("bench_reference_none.c schreiben");
            stub
        }
    };
    let present = format!("/// Ist eine C-Referenz gebunden?\npub const PRESENT: bool = {};\n", given.is_some());
    fs::write(out.join("bench_reference.rs"), present).expect("bench_reference.rs schreiben");
    let obj = out.join("bench_reference.o");
    translate(&src, &obj, &["-ffp-contract=off", "-fno-math-errno"]);
    obj
}

/// Die Vektoren der kuratierten Natives fuer das Messprogramm `natives`
/// (13.8), aus der Spezifikation `grammar/takt-native.md`.
fn native_vectors(out: &Path) {
    let spec = takt_conformance::natives::spec_path();
    println!("cargo:rerun-if-changed={}", spec.display());
    let text = fs::read_to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    let vectors = takt_conformance::natives::vectors(&text).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    fs::write(out.join("native_vectors.rs"), takt_conformance::natives::table_source(&vectors))
        .expect("native_vectors.rs schreiben");
}

/// Die Vektoren der korrekt gerundeten Mathematik fuer dasselbe
/// Messprogramm (4.2, 13.8).
fn math_vectors(out: &Path) {
    let spec = takt_conformance::math::spec_path();
    println!("cargo:rerun-if-changed={}", spec.display());
    let text = fs::read_to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    let vectors = takt_conformance::math::vectors(&text).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    fs::write(out.join("math_vectors.rs"), takt_conformance::math::table_source(&vectors))
        .expect("math_vectors.rs schreiben");
}

/// Uebersetzt eine Quelle (IR oder C) mit den Groessenflags des Ziels.
fn translate(src: &Path, obj: &Path, extra: &[&str]) {
    let ok = Command::new(bringup::clang())
        .args(["-c", "-Wno-override-module", "-ffreestanding", "-nostdlib", "--target=thumbv7em-none-eabihf"])
        .args(takt_llvm::toolchain::object_flags("thumbv7em-none-eabihf"))
        .args(extra)
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}
