//! `takt build --emit embed` (12.11, M11 Schritt 8): die Lieferform eines
//! Programms fuer einen Wirt, ihre Baufehler und das Beispiel
//! `examples/rust-host`, das sie mit einem Aufruf bindet.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use takt_llvm::toolchain::{Clang, find};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// Das Tripel des Wirts, wie Cargo es einem Bauskript nennt.
fn host_triple() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

const VALVE: &str = "examples/rust-host/takt/valve.takt";

/// **Ein Aufruf liefert alles** (12.11): Bibliothek, Kopf, Rust-Modul und
/// Manifest. Der Kopf uebersetzt allein in strengem C11; die Bibliothek
/// definiert genau das ABI-Symbol, das die Huelle liest, sodass eine Huelle
/// anderer Version nicht bindet.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn one_call_delivers_library_header_module_and_manifest() {
    let Some(clang) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let dir = scratch("takt-embed-delivery");
    let out = dir.to_str().expect("Pfad");
    let run = takt(&[
        "build",
        VALVE,
        "--emit",
        "embed",
        "--target",
        host_triple(),
        "--form",
        "logical",
        "--drivers",
        "crate::Tank",
        "--out",
        out,
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let lib = dir.join(if cfg!(windows) { "valve.lib" } else { "libvalve.a" });
    for f in [&lib, &dir.join("valve.h"), &dir.join("valve.rs"), &dir.join("valve.manifest")] {
        assert!(f.is_file(), "{} fehlt", f.display());
    }

    let manifest = std::fs::read_to_string(dir.join("valve.manifest")).expect("Manifest");
    for line in [
        "# takt-manifest 1",
        "prefix = valve",
        "abi = 1",
        &format!("triple = {}", host_triple()),
        "form = logical",
        "profile = none",
        "tick_ns = 10000000",
        "drivers = valve_out_tank_valve, valve_alive_tank",
    ] {
        assert!(manifest.lines().any(|l| l == line), "`{line}` fehlt im Manifest:\n{manifest}");
    }

    let header = dir.join("valve.h");
    let c = dir.join("uses_header.c");
    std::fs::write(&c, "#include \"valve.h\"\nstatic struct valve_arena arena;\nvoid *use(void) { return &arena; }\n")
        .expect("Quelle");
    let mut cmd = Command::new(&clang);
    let strict = Clang::deterministic(&mut cmd)
        .args(["-fsyntax-only", "-std=c11", "-Wall", "-Wextra", "-pedantic", "-Werror", "-I"])
        .arg(&dir)
        .arg(&c)
        .output()
        .expect("clang");
    assert!(strict.status.success(), "{}: {}", header.display(), String::from_utf8_lossy(&strict.stderr));
    let text = std::fs::read_to_string(&header).expect("Kopf");
    assert!(text.contains("#define VALVE_TICK_NS 10000000LL"), "{text}");

    let module = std::fs::read_to_string(dir.join("valve.rs")).expect("Modul");
    assert!(module.contains("ffi::valve_abi_1"), "die Huelle liest das ABI-Symbol nicht");
    let nm = clang.with_file_name(if cfg!(windows) { "llvm-nm.exe" } else { "llvm-nm" });
    let symbols = Command::new(&nm).arg("--defined-only").arg(&lib).output().expect("llvm-nm");
    let symbols = String::from_utf8_lossy(&symbols.stdout);
    let abi: Vec<&str> =
        symbols.lines().filter_map(|l| l.split_whitespace().last()).filter(|s| s.contains("_abi_")).collect();
    assert_eq!(abi, ["valve_abi_1"], "{symbols}");
}

/// **Die Float-ABI des Tripels gehoert zur Zielklasse** (12.11, 2.9): Ein
/// Tripel ohne FPU zu einer Klasse mit FPU ist ein Baufehler mit Tripel und
/// Klasse, kein stiller Wechsel auf Soft-Float.
#[test]
fn a_soft_float_triple_is_refused_with_triple_and_class() {
    let run = takt(&["build", VALVE, "--emit", "embed", "--target", "thumbv7em-none-eabi", "--form", "own"]);
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("`thumbv7em-none-eabi`") && err.contains("thumbv7em-none-eabihf"), "{err}");
}

/// **Das Profil kommt aus der Einbindung** (12.8, 12.11): Nennt das
/// Programm eines, das der Form widerspricht, nennt der Fehler beide und
/// schlaegt vor, `system: target` wegzulassen. Ohne Form gibt es keine
/// Lieferform.
#[test]
fn a_profile_against_the_form_is_refused_with_both_names() {
    let dir = scratch("takt-embed-profile");
    let src = dir.join("linux.takt");
    let text = std::fs::read_to_string(root().join(VALVE)).expect("Programm");
    std::fs::write(&src, text.replace("    tick     = 10 ms\n", "    tick     = 10 ms\n    target   = linux_rt\n"))
        .expect("Programm");
    let out = dir.join("out");
    let (src, out) = (src.to_str().expect("Pfad"), out.to_str().expect("Pfad"));
    let run = takt(&["build", src, "--emit", "embed", "--target", host_triple(), "--form", "own", "--out", out]);
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("linux_rt") && err.contains("`own`") && err.contains("baremetal"), "{err}");
    assert!(err.contains("system: target"), "kein Vorschlag: {err}");

    let run = takt(&["build", src, "--emit", "embed", "--target", host_triple(), "--out", out]);
    assert!(!run.status.success());
    assert!(String::from_utf8_lossy(&run.stderr).contains("`--form` fehlt"));
}

/// **`examples/rust-host` baut mit einem Aufruf und laeuft wie der
/// Interpreter** (12.11, 13.1): `cargo test` im Beispiel ruft ueber
/// `takt_embed::build` dieses Werkzeug, bindet die Lieferform und
/// vergleicht den Trace des Programms in logischer Zeit mit dem
/// Interpreter.
#[test]
fn the_rust_example_builds_with_one_call_and_agrees_with_the_interpreter() {
    let takt = PathBuf::from(env!("CARGO_BIN_EXE_takt"));
    // Dasselbe Zielverzeichnis wie diese Suite: `<ziel>/<profil>/takt`.
    let target = takt.parent().and_then(Path::parent).expect("Zielverzeichnis");
    let run = Command::new("cargo")
        .args(["test", "--offline", "--manifest-path"])
        .arg(root().join("examples/rust-host/Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .env("TAKT", &takt)
        .output()
        .expect("cargo");
    assert!(run.status.success(), "{}\n{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(String::from_utf8_lossy(&run.stdout).contains("the_valve_runs_like_the_interpreter ... ok"));
}
