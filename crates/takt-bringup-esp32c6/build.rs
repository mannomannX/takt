//! Baut das Takt-Programm fuer den ESP32-C6 und bindet es ein.
//!
//! Dieselbe Konstruktion wie beim F401-Bring-up: `takt build` erzeugt das
//! Objekt fuer `riscv32imac`, `takt_conformance::mcu` den C-Rahmen, `clang`
//! uebersetzt ihn fuer RV32IMAC, `llvm-ar` packt beides in ein Archiv.
//! Die Linker-Argumente stehen hier und nicht in `.cargo/config.toml`: Die
//! Konfigurationsdatei gilt nur, wenn `cargo` aus diesem Verzeichnis laeuft,
//! und ein Bau von aussen erzeugte sonst still ein Binary ohne Speicherkarte.
//!
//! **Die C-Referenz fuer `takt bench`** (13.8) nennt `TAKT_BENCH_C`, wie
//! beim F401: dieselben Flags wie der erzeugte Code, dazu
//! `-ffp-contract=off` und `-fno-math-errno`, und im Archiv, also mit ihm
//! im RAM (12.3).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    // 11.2: `statements` fuellt `pc` je Maschine; Default auf `baremetal`
    // ist `states`, also aus.
    println!("cargo:rerun-if-env-changed=TAKT_INSTRUMENT");
    println!("cargo:rerun-if-env-changed=TAKT_DIAGNOSTICS");
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        println!("cargo:rustc-env=TAKT_INSTRUMENT={mode}");
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    ram_resident(&out);
    build_takt_program(&out);
    native_vectors(&out);
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
    let Some(p) = compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let rahmen = out.join("takt_rahmen.c");
    if let Err(e) = fs::write(&rahmen, takt_conformance::mcu::build_with(&p, diagnostics()).source) {
        panic!("Rahmen nicht schreibbar: {e}");
    }
    let ir = out.join("takt_programm.ll");
    run_takt_build(&program, &["--emit", "ir"], &ir);
    run_takt_build(&program, &["--emit", "consts-rs"], &out.join("takt_consts.rs"));
    let (obj, obj_rahmen) = (out.join("takt_programm.o"), out.join("takt_rahmen.o"));
    translate(&ir, &obj, &[]);
    translate(&rahmen, &obj_rahmen, &[]);
    let millicode = Path::new(env!("CARGO_MANIFEST_DIR")).join("millicode.S");
    println!("cargo:rerun-if-changed={}", millicode.display());
    let obj_mc = out.join("millicode.o");
    assemble(&millicode, &obj_mc);
    let reference = bench_reference(out);
    archive(out, &[&obj, &obj_rahmen, &obj_mc, &reference]);
    println!("cargo:rustc-link-arg=--icf=all");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=taktprogramm");
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

fn run_takt_build(program: &str, emit: &[&str], out: &Path) {
    let Some(takt) = find_takt() else {
        let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
        panic!("Das Werkzeug takt fehlt; erst `cargo build -p takt-cli --{profile}`");
    };
    println!("cargo:rerun-if-changed={}", takt.display());
    let mut cmd = Command::new(&takt);
    cmd.args(["build", program, "--target", "riscv32imac", "--build", "hw"]).args(emit).arg("--out").arg(out);
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        cmd.args(["--instrument", &mode]);
    }
    if let Ok(level) = env::var("TAKT_DIAGNOSTICS") {
        cmd.args(["--diagnostics", &level]);
    }
    // 8.10: Anschluesse und NVM-Zeiten des Boards, wenn die Konfiguration da ist.
    let hardware = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/hw/esp32c6.hw");
    println!("cargo:rerun-if-changed={}", hardware.display());
    if hardware.exists() {
        cmd.arg("--hardware").arg(&hardware);
    }
    let status = cmd.status();
    match status {
        Ok(s) if s.success() => {}
        Ok(_) => panic!("takt build {} schlug fehl fuer {program}", emit.join(" ")),
        Err(e) => panic!("takt nicht aufrufbar ({e}); ist `cargo build -p takt-cli` gelaufen?"),
    }
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
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn entsteht kein Programm") };
    let ok = Command::new(&clang)
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
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn entsteht kein Millicode") };
    let ok = Command::new(&clang)
        .args(["-c", "--target=riscv32-unknown-none-elf", "-march=rv32imac", "-mabi=ilp32"])
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}

fn archive(out: &Path, objs: &[&Path]) {
    let lib = out.join("libtaktprogramm.a");
    let _ = fs::remove_file(&lib);
    let Some(clang) = clang() else { panic!("clang fehlt; ohne ihn auch kein llvm-ar") };
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let ok = Command::new(&ar).arg("crs").arg(&lib).args(objs).status().is_ok_and(|s| s.success());
    assert!(ok, "llvm-ar schlug fehl; das Takt-Programm waere nicht gebunden");
}

fn clang() -> Option<PathBuf> {
    match takt_llvm::toolchain::find() {
        takt_llvm::toolchain::Clang::At(p) => Some(p),
        takt_llvm::toolchain::Clang::Missing => None,
    }
}

/// Das `takt`-Werkzeug aus demselben Zielverzeichnis, in dem dieses
/// Skript laeuft; ein paar Ebenen ueber `OUT_DIR`.
fn find_takt() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "takt.exe" } else { "takt" };
    let profile = env::var("PROFILE").unwrap_or_else(|_| "release".into());
    let out = PathBuf::from(env::var("OUT_DIR").ok()?);
    let mut dir = out.as_path();
    for _ in 0..6 {
        let Some(parent) = dir.parent() else { break };
        dir = parent;
        let p = dir.join(&profile).join(exe);
        if p.exists() {
            assert_fresh(&p);
            return Some(p);
        }
    }
    None
}

/// Ein `takt`, das aelter ist als der Compiler, baut stillschweigend das
/// Objekt von gestern (FB-193). Der Vergleich ist grob — Aenderungszeit
/// gegen jede Quelle der Compiler-Crates —, aber er faellt genau dann,
/// wenn es darauf ankommt.
fn assert_fresh(takt: &Path) {
    let Ok(built) = fs::metadata(takt).and_then(|m| m.modified()) else { return };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for krate in ["takt-syntax", "takt-diag", "takt-mir", "takt-sema", "takt-interp", "takt-llvm", "takt-cli"] {
        walk(&root.join("crates").join(krate).join("src"), &mut newest);
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

fn walk(dir: &Path, newest: &mut Option<(std::time::SystemTime, PathBuf)>) {
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

/// Die Diagnosestufe aus `TAKT_DIAGNOSTICS`; ohne Angabe `ids`.
fn diagnostics() -> takt_llvm::Diagnostics {
    env::var("TAKT_DIAGNOSTICS")
        .ok()
        .and_then(|l| takt_llvm::Diagnostics::parse(&l))
        .unwrap_or(takt_llvm::Diagnostics::Ids)
}
