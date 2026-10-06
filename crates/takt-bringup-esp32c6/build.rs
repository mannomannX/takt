//! Baut das Takt-Programm fuer den ESP32-C6 und bindet es ein.
//!
//! **Wie jeder Wirt** (12.11, M11 Schritt 10): Der Bauhelfer aus
//! `takt-embed` ruft `takt build --emit embed` und bindet die Bibliothek
//! `libapp.a` mit Rahmen und erzeugtem Code; das Modul `app.rs` traegt
//! Konstanten, Arena, Treiber-Traits und die Huelle. Die Linker-Argumente
//! stehen hier und nicht in `.cargo/config.toml`: Die Konfigurationsdatei
//! gilt nur, wenn `cargo` aus diesem Verzeichnis laeuft, und ein Bau von
//! aussen erzeugte sonst still ein Binary ohne Speicherkarte.
//!
//! **Die Treiber** (12.6) erzeugt `bringup::rig` als Pruefstand: fuer
//! `takt` nach der Verdrahtung des Boards und des Pruefgeraets, fuer
//! `bench` ganz aus Stummeln — der Messkern misst den Tick, nicht die
//! Peripherie.
//!
//! **Die C-Referenz fuer `takt bench`** (13.8) nennt `TAKT_BENCH_C`, wie
//! beim F401: dieselben Flags wie der erzeugte Code, dazu
//! `-ffp-contract=off` und `-fno-math-errno`, und im Archiv, also mit ihm
//! im RAM (12.3).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_conformance::bringup;

fn main() {
    // `linkall.x` liefert `esp-hal`: Speicherkarte, Vektortabelle, Cache-Mapping.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    image_key();
    // Konformitaetslauf (plan/esp32c6.md 5): so viele Ticks, dann `takt end`.
    println!("cargo:rerun-if-env-changed=TAKT_TICKS");
    if let Ok(ticks) = env::var("TAKT_TICKS") {
        println!("cargo:rustc-env=TAKT_TICKS={ticks}");
    }
    // `TAKT_FRESH_JOURNAL`: das Journal vor dem Lauf loeschen — wie der
    // Interpreter ohne Speicher beginnen (Konformitaetslauf).
    println!("cargo:rerun-if-env-changed=TAKT_FRESH_JOURNAL");
    if env::var("TAKT_FRESH_JOURNAL").is_ok() {
        println!("cargo:rustc-env=TAKT_FRESH_JOURNAL=1");
    }
    // 11.2: `statements` fuellt `pc` je Maschine (`takt build --instrument`);
    // Default auf `baremetal` ist `states`, also aus.
    println!("cargo:rerun-if-env-changed=TAKT_INSTRUMENT");
    println!("cargo:rerun-if-env-changed=TAKT_DIAGNOSTICS");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    ram_resident(&out);
    build_takt_program(&out);
    native_vectors(&out);
    math_vectors(&out);
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

/// Legt Takt-Code und tick-gelesene Konstanten ins RAM (12.3).
///
/// `esp-hal` bindet `rwtext_hook.x` in `.rwtext` ein; die Datei muss im
/// Suchpfad des Linkers liegen, und `OUT_DIR` steht dort. Eingeschaltet
/// wird der Haken ueber `ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK` — als echte
/// Umgebungsvariable zur Bauzeit von `esp-hal`, darum in
/// `.cargo/config.toml` und nicht hier.
fn ram_resident(out: &Path) {
    let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("rwtext_hook.x");
    println!("cargo:rerun-if-changed={}", hook.display());
    if let Err(e) = fs::copy(&hook, out.join("rwtext_hook.x")) {
        panic!("rwtext_hook.x nicht kopierbar: {e}");
    }
    println!("cargo:rustc-link-search={}", out.display());
}

fn build_takt_program(out: &Path) {
    let program = program_path();
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    println!("cargo:rerun-if-changed={program}");
    if !Path::new(&program).exists() {
        panic!("Takt-Programm nicht gefunden: {program}");
    }
    let Some(p) = bringup::compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wiring =
        bringup::wiring(&[&here.join(bringup::WIRING), &here.join("../takt-driver-probe").join(bringup::WIRING)]);
    bringup::rig(&p, &wiring, &out.join("takt_rig.rs"));
    bringup::rig(&p, &[], &out.join("takt_rig_bench.rs"));
    let mut embed = takt_embed::build::Program::new(&program)
        .tool(bringup::takt())
        .prefix(takt_llvm::symbols::Prefix::default().as_str())
        .form("own")
        .build_for(bringup::build_name())
        .drivers("crate::drivers::Rig")
        .hardware(hardware());
    // 11.2: `statements` fuellt `pc` je Maschine; Default auf `baremetal`
    // ist `states`, also aus.
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        embed = embed.instrument(&mode);
    }
    if let Ok(level) = env::var("TAKT_DIAGNOSTICS") {
        embed = embed.diagnostics(&level);
    }
    let built = embed.build();
    // Wo der Tick in der Arena steht, als absolutes Symbol: Die Probe liest
    // ihn ueber JTAG, wenn die Konsole schweigt (`Esp32c6::tick_over_jtag`).
    println!("cargo:rustc-link-arg=--defsym=__takt_tick_at={}", built.value("tick_at"));
    // Was das Board dazulegt: die Millicode-Routinen fuer `-msave-restore`
    // und die C-Referenz von `takt bench`, wie das Programm im RAM.
    let millicode = here.join("millicode.S");
    println!("cargo:rerun-if-changed={}", millicode.display());
    let obj_mc = out.join("millicode.o");
    assemble(&millicode, &obj_mc);
    let reference = bench_reference(out);
    bringup::archive(out, "taktboard", &[&obj_mc, &reference]);
    println!("cargo:rustc-link-arg=--icf=all");
}

/// Die Hardware-Konfiguration des Boards (8.10): Anschluesse, Kalibrierung,
/// NVM-Zeiten und die Reserve des Job-Stacks. `TAKT_HARDWARE` sticht, etwa
/// fuer `takt driver-test` mit gemessenem `guard`.
fn hardware() -> String {
    bringup::hardware_path().unwrap_or_else(|| {
        let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/hw/esp32c6.hw");
        println!("cargo:rerun-if-changed={}", own.display());
        own.display().to_string()
    })
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

/// Uebersetzt eine C-Quelle mit den Flags, die auch der erzeugte Code bekommt.
fn translate(src: &Path, obj: &Path, extra: &[&str]) {
    let ok = Command::new(bringup::clang())
        .args([
            "-c",
            "-ffreestanding",
            "-nostdlib",
            "--target=riscv32-unknown-none-elf",
            "-march=rv32imac",
            "-mabi=ilp32",
        ])
        .args(takt_llvm::toolchain::object_flags("riscv32-unknown-none-elf"))
        .args(extra)
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}

/// Uebersetzt die Millicode-Routinen fuer `-msave-restore`.
fn assemble(src: &Path, obj: &Path) {
    let ok = Command::new(bringup::clang())
        .args(["-c", "--target=riscv32-unknown-none-elf", "-march=rv32imac", "-mabi=ilp32"])
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}
