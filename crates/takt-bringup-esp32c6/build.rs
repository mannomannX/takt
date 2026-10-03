//! Baut das Takt-Programm fuer den ESP32-C6 und bindet es ein.
//!
//! Dieselbe Konstruktion wie beim F401-Bring-up: `takt build` erzeugt das
//! Objekt fuer `riscv32imac`, `takt_frame::mcu` den C-Rahmen, `clang`
//! uebersetzt ihn fuer RV32IMAC, `llvm-ar` packt beides in ein Archiv.
//! Die Linker-Argumente stehen hier und nicht in `.cargo/config.toml`: Die
//! Konfigurationsdatei gilt nur, wenn `cargo` aus diesem Verzeichnis laeuft,
//! und ein Bau von aussen erzeugte sonst still ein Binary ohne Speicherkarte.
//!
//! **Die Treiber** (12.6) erzeugt `bringup::drivers` als Pruefstand: fuer
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
    let rahmen = out.join("takt_rahmen.c");
    let frame = takt_frame::mcu::build_with(
        &p,
        takt_frame::mcu::Frame {
            diagnostics: diagnostics(),
            hardware: bringup::hardware().as_ref(),
            protect: None,
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
    bringup::arena(&frame, "riscv32-unknown-none-elf", &["-march=rv32imac", "-mabi=ilp32"], &out.join("takt_arena.rs"));
    // Wo der Tick in der Arena steht, als absolutes Symbol: Die Probe liest
    // ihn ueber JTAG, wenn die Konsole schweigt (`Esp32c6::tick_over_jtag`).
    println!("cargo:rustc-link-arg=--defsym=__takt_tick_at={}", frame.tick_at);
    let ir = out.join("takt_programm.ll");
    run_takt_build(&program, &["--emit", "ir"], &ir);
    run_takt_build(&program, &["--emit", "consts-rs"], &out.join("takt_consts.rs"));
    let (obj, obj_rahmen) = (out.join("takt_programm.o"), out.join("takt_rahmen.o"));
    translate(&ir, &obj, &[]);
    // 4.5: Interrupts laufen auf dem Stack des unterbrochenen Fadens, also
    // auch auf dem des Jobs (`jobs.rs` im Board-Crate). Trap-Rahmen,
    // Verteiler und die Handler einer Prioritaetsstufe brauchen unter 1 KiB;
    // die Reserve verdoppelt das.
    translate(&rahmen, &obj_rahmen, &["-DTAKT_JOB_STACK_RESERVE=2048"]);
    let millicode = Path::new(env!("CARGO_MANIFEST_DIR")).join("millicode.S");
    println!("cargo:rerun-if-changed={}", millicode.display());
    let obj_mc = out.join("millicode.o");
    assemble(&millicode, &obj_mc);
    let reference = bench_reference(out);
    bringup::archive(out, &[&obj, &obj_rahmen, &obj_mc, &reference]);
    println!("cargo:rustc-link-arg=--icf=all");
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

/// `takt build` fuer `riscv32imac` mit `emit`, dazu Instrumentierung und
/// Diagnosestufe aus der Umgebung (11.2) und die Konfiguration des Boards.
fn run_takt_build(program: &str, emit: &[&str], out: &Path) {
    let mut extra: Vec<String> = emit.iter().map(|s| (*s).to_string()).collect();
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        extra.extend(["--instrument".into(), mode]);
    }
    if let Ok(level) = env::var("TAKT_DIAGNOSTICS") {
        extra.extend(["--diagnostics".into(), level]);
    }
    // 8.10: Anschluesse und NVM-Zeiten des Boards, wenn die Konfiguration da ist.
    let hardware = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/hw/esp32c6.hw");
    println!("cargo:rerun-if-changed={}", hardware.display());
    if hardware.exists() {
        extra.extend(["--hardware".into(), hardware.display().to_string()]);
    }
    let extra: Vec<&str> = extra.iter().map(String::as_str).collect();
    bringup::takt_build(program, "riscv32imac", &extra, out);
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

fn translate(src: &Path, obj: &Path, extra: &[&str]) {
    let ok = Command::new(bringup::clang())
        .args([
            "-c",
            "-Wno-override-module",
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

/// Die Diagnosestufe aus `TAKT_DIAGNOSTICS`; ohne Angabe `ids`.
fn diagnostics() -> takt_llvm::Diagnostics {
    env::var("TAKT_DIAGNOSTICS")
        .ok()
        .and_then(|l| takt_llvm::Diagnostics::parse(&l))
        .unwrap_or(takt_llvm::Diagnostics::Ids)
}
