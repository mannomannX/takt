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
        "abi = 4",
        "protect = none",
        &format!("triple = {}", host_triple()),
        "form = logical",
        "profile = none",
        "tick_ns = 10000000",
        "drivers = valve_out_tank_valve, valve_alive_tank",
    ] {
        assert!(manifest.lines().any(|l| l == line), "`{line}` fehlt im Manifest:\n{manifest}");
    }

    // Die Arena aus einer Quelle (GEN-034): Groesse und Ausrichtung, die das
    // Manifest nennt, hat der Kopf fuer den C-Compiler und der Rust-Typ.
    let number = |key: &str| -> u64 {
        let line = manifest.lines().find_map(|l| l.strip_prefix(key)).unwrap_or_else(|| panic!("{key} fehlt"));
        line.trim().parse().unwrap_or_else(|_| panic!("{key} keine Zahl: {line}"))
    };
    let (bytes, align) = (number("arena_bytes = "), number("arena_align = "));
    let module = std::fs::read_to_string(dir.join("valve.rs")).expect("Modul");
    for part in [format!("#[repr(C, align({align}))]"), format!("MaybeUninit<[u8; {bytes}]>")] {
        assert!(module.contains(&part), "`{part}` fehlt in valve.rs:\n{module}");
    }

    let header = dir.join("valve.h");
    let c = dir.join("uses_header.c");
    std::fs::write(
        &c,
        format!(
            "#include \"valve.h\"\nstatic struct valve_arena arena;\nvoid *use(void) {{ return &arena; }}\n\
             _Static_assert(sizeof(struct valve_arena) == {bytes}, \"Groesse wie im Manifest\");\n\
             _Static_assert(_Alignof(struct valve_arena) == {align}, \"Ausrichtung wie im Manifest\");\n"
        ),
    )
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

    assert!(module.contains("ffi::valve_abi_4"), "die Huelle liest das ABI-Symbol nicht");
    let nm = clang.with_file_name(if cfg!(windows) { "llvm-nm.exe" } else { "llvm-nm" });
    let symbols = Command::new(&nm).arg("--defined-only").arg(&lib).output().expect("llvm-nm");
    let symbols = String::from_utf8_lossy(&symbols.stdout);
    let abi: Vec<&str> =
        symbols.lines().filter_map(|l| l.split_whitespace().last()).filter(|s| s.contains("_abi_")).collect();
    assert_eq!(abi, ["valve_abi_4"], "{symbols}");
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

/// Baut die Lieferform von `valve` und liefert das Verzeichnis.
fn deliver(name: &str, triple: &str, form: &str) -> PathBuf {
    let dir = scratch(name);
    let out = dir.to_str().expect("Pfad");
    let run = takt(&["build", VALVE, "--emit", "embed", "--target", triple, "--form", form, "--out", out]);
    assert!(run.status.success(), "{triple} {form}: {}", String::from_utf8_lossy(&run.stderr));
    dir
}

/// **Eine geschuetzte Arena ist ihre Region** (8.10 `protect`, 12.3, M11
/// Schritt 10): Mit `protect = armv7m_mpu` fuellt der Rahmen den
/// Programmbereich auf die Region auf, die ihn deckt, der Tick steht dahinter,
/// und die Arena ist an der Region ausgerichtet; das Manifest nennt beides.
/// `job_stack_reserve` gibt dem Job-Stack seine Reserve; ihn stellt der Wirt
/// nach `JOB_STACK_BYTES` (4.5, 12.11).
#[test]
fn a_protected_arena_is_padded_and_aligned_to_its_region() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let dir = scratch("takt-embed-protect");
    let hw = dir.join("board.hw");
    std::fs::write(&hw, "# takt-hw 14\n[target.thumbv7em]\nprotect = armv7m_mpu\njob_stack_reserve = 3000\n")
        .expect("hw");
    let source = "corpus-try/40_jobs.takt";
    let out = dir.join("out");
    let run = takt(&[
        "build",
        source,
        "--emit",
        "embed",
        "--target",
        "thumbv7em-none-eabihf",
        "--form",
        "own",
        "--prefix",
        "jobs",
        "--hardware",
        hw.to_str().expect("Pfad"),
        "--out",
        out.to_str().expect("Pfad"),
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let text = std::fs::read_to_string(root().join(source)).expect("Quelle");
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let p = takt_sema::compile(&text, &options).program.expect("Programm");
    let window =
        takt_mir::hardware::Protect::Armv7mMpu.window(takt_llvm::arena::of(&p).bytes).expect("eine Region deckt ihn");
    let manifest = std::fs::read_to_string(out.join("jobs.manifest")).expect("Manifest");
    let value = |key: &str| {
        let prefix = format!("{key} = ");
        manifest.lines().find_map(|l| l.strip_prefix(&prefix)).unwrap_or_else(|| panic!("{key} fehlt:\n{manifest}"))
    };
    assert_eq!(value("protect"), "armv7m_mpu");
    assert_eq!(value("protect_bytes"), window.protected.to_string());
    assert_eq!(value("tick_at"), window.protected.to_string(), "der Tick steht hinter dem geschuetzten Bereich");
    assert_eq!(value("arena_align"), window.size.to_string(), "die Arena liegt, wo die Region liegt");
    // 4.5, 12.11: Den Job-Stack stellt der Wirt, mit der Reserve aus der Konfiguration.
    let job_stack = takt_frame::mcu::job_stack_bytes(&p, 3000);
    assert!(job_stack > 3000 + 32, "Vertrag, Reserve und Waechter: {job_stack}");
    assert_eq!(value("job_stack_bytes"), job_stack.to_string());
    let module = std::fs::read_to_string(out.join("jobs.rs")).expect("Modul");
    assert!(module.contains(&format!("align({})", window.size)), "das Modul legt die Arena ebenso aus");
    assert!(module.contains(&format!("pub const JOB_STACK_BYTES: usize = {job_stack};")), "{module}");
    let header = std::fs::read_to_string(out.join("jobs.h")).expect("Kopf");
    assert!(header.contains(&format!("#define JOBS_JOB_STACK_BYTES {job_stack}u")), "{header}");
}

/// **Die Kalibrierung eines Tripels steht unter seinem Ziel** (8.10,
/// FB-445): Cargo nennt `riscv32imac-unknown-none-elf`, die
/// Hardware-Konfiguration `[target.riscv32imac]`. Die Lieferform findet die
/// NVM-Zeiten des C6 dort; ohne sie gaelte sein Flash als asynchron, und das
/// Journal hielte den Kern mitten in einer Periode an.
#[test]
fn the_calibration_of_a_triple_is_found_under_its_target_name() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let dir = scratch("takt-embed-calibration");
    let run = takt(&[
        "build",
        "corpus-try/35_persist.takt",
        "--emit",
        "embed",
        "--target",
        "riscv32imac-unknown-none-elf",
        "--form",
        "own",
        "--prefix",
        "journal",
        "--hardware",
        "corpus-try/hw/esp32c6.hw",
        "--out",
        dir.to_str().expect("Pfad"),
    ]);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "{stderr}");
    assert!(!stderr.contains("kein Ziel"), "das Ziel des Tripels fehlt in der Konfiguration:\n{stderr}");
    let module = std::fs::read_to_string(dir.join("journal.rs")).expect("Modul");
    let blocking: i64 = module
        .lines()
        .find_map(|l| l.strip_prefix("pub const NVM_BLOCKING_NS: i64 = ")?.strip_suffix(';')?.parse().ok())
        .unwrap_or_else(|| panic!("NVM_BLOCKING_NS fehlt:\n{module}"));
    assert!(blocking > 0, "die NVM-Zeiten des C6 fehlen: {blocking}");
}

fn manifest_value(dir: &Path, key: &str) -> String {
    let text = std::fs::read_to_string(dir.join("valve.manifest")).expect("Manifest");
    let prefix = format!("{key} = ");
    text.lines().find_map(|l| l.strip_prefix(&prefix)).unwrap_or_else(|| panic!("{key} fehlt:\n{text}")).to_string()
}

/// 12.8 und 12.11: Die Lieferform entsteht fuer jede Zielklasse, nicht nur
/// fuer den Wirt. Die Bibliothek definiert die Einstiege, und Groesse und
/// Ausrichtung der Arena im Manifest gelten fuer das Ziel: Der Kopf haelt sie
/// unter dem C-Compiler dieses Ziels.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn every_target_class_gets_a_library_with_its_entries_and_arena() {
    let Some(clang) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let nm = clang.with_file_name(if cfg!(windows) { "llvm-nm.exe" } else { "llvm-nm" });
    for triple in [host_triple(), "thumbv7em-none-eabihf", "riscv32imac-unknown-none-elf"] {
        let dir = deliver(&format!("takt-embed-target-{triple}"), triple, "logical");
        let lib = dir.join(if triple.ends_with("-msvc") { "valve.lib" } else { "libvalve.a" });
        let symbols = Command::new(&nm).arg("--defined-only").arg(&lib).output().expect("llvm-nm");
        let symbols = String::from_utf8_lossy(&symbols.stdout);
        let defined: Vec<&str> = symbols.lines().filter_map(|l| l.split_whitespace().last()).collect();
        for entry in [
            "valve_init",
            "valve_tick",
            "valve_commit",
            "valve_deadline",
            "valve_output_timing",
            "valve_abi_4",
            "valve_arena_bytes",
        ] {
            assert!(defined.contains(&entry), "{triple}: `{entry}` fehlt:\n{symbols}");
        }
        let (bytes, align) = (manifest_value(&dir, "arena_bytes"), manifest_value(&dir, "arena_align"));
        let c = dir.join("arena.c");
        std::fs::write(
            &c,
            format!(
                "#include \"valve.h\"\n_Static_assert(sizeof(struct valve_arena) == {bytes}, \"Groesse\");\n\
                 _Static_assert(_Alignof(struct valve_arena) == {align}, \"Ausrichtung\");\n"
            ),
        )
        .expect("Quelle");
        // Das LLVM-Triple und `-march` des Ziels, nicht das Tripel des Wirts.
        let target = takt_llvm::Target::by_host_triple(triple).expect("bekanntes Tripel");
        let march: Vec<String> =
            Some(target.march).filter(|m| !m.is_empty()).map(|m| format!("-march={m}")).into_iter().collect();
        let mut cmd = Command::new(&clang);
        let check = Clang::deterministic(&mut cmd)
            .args(["-fsyntax-only", "-std=c11", "-ffreestanding"])
            .arg(format!("--target={}", target.triple))
            .args(&march)
            .arg("-I")
            .arg(&dir)
            .arg(&c)
            .output()
            .expect("clang");
        assert!(check.status.success(), "{triple}: {}", String::from_utf8_lossy(&check.stderr));
    }
}

/// 12.11: Jede Form baut und traegt ihr Profil ins Manifest; zwei Laeufe in
/// verschiedene Verzeichnisse liefern bitgleiche Bibliotheken (11.3), der
/// Pfad der Ausgabe steht nicht im Objekt.
#[test]
fn every_form_builds_with_its_profile_and_two_builds_are_identical() {
    let Some(_) = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let triple = "thumbv7em-none-eabihf";
    for (form, profile) in
        [("own", "baremetal"), ("interrupt", "shared"), ("poll", "shared"), ("rtos", "shared"), ("linux", "linux_rt")]
    {
        let dir = deliver(&format!("takt-embed-form-{form}"), triple, form);
        assert_eq!((manifest_value(&dir, "form"), manifest_value(&dir, "profile")), (form.into(), profile.into()));
    }
    let first = deliver("takt-embed-repro-a", triple, "logical");
    let second = deliver("takt-embed-repro-b-anderer-pfad", triple, "logical");
    for file in ["libvalve.a", "valve.h", "valve.rs", "valve.manifest"] {
        let (a, b) = (std::fs::read(first.join(file)).expect("a"), std::fs::read(second.join(file)).expect("b"));
        assert!(a == b, "{file} weicht zwischen zwei Laeufen ab");
    }
}

/// 12.8 und 12.11: Ein Profil im Programm, das der Form widerspricht, ist
/// ein Baufehler; unter `logical` hat die Form kein Profil und nichts
/// widerspricht.
#[test]
fn every_profile_against_a_foreign_form_is_refused() {
    let text = std::fs::read_to_string(root().join(VALVE)).expect("Programm");
    let cases = [
        ("baremetal", "interrupt", false),
        ("baremetal", "poll", false),
        ("baremetal", "rtos", false),
        ("shared", "own", false),
        ("shared", "linux", false),
        ("linux_rt", "rtos", false),
        ("baremetal", "own", true),
        ("shared", "poll", true),
        ("linux_rt", "logical", true),
    ];
    for (profile, form, accepted) in cases {
        let dir = scratch(&format!("takt-embed-pf-{profile}-{form}"));
        let src = dir.join("valve.takt");
        std::fs::write(
            &src,
            text.replace("    tick     = 10 ms\n", &format!("    tick     = 10 ms\n    target   = {profile}\n")),
        )
        .expect("Programm");
        let out = dir.join("out");
        let run = takt(&[
            "build",
            src.to_str().expect("Pfad"),
            "--emit",
            "embed",
            "--target",
            "thumbv7em-none-eabihf",
            "--form",
            form,
            "--out",
            out.to_str().expect("Pfad"),
        ]);
        let err = String::from_utf8_lossy(&run.stderr);
        if accepted {
            assert!(run.status.success(), "{profile} unter {form}: {err}");
        } else {
            assert!(!run.status.success(), "{profile} unter {form} angenommen");
            assert!(err.contains(&format!("`{form}`")) && err.contains(profile), "{profile} unter {form}: {err}");
        }
    }
}

/// 5.9 und 12.11: Findet der Codegen fuer eine `persist var` keinen Lesepfad,
/// startet jeder Lauf still beim Default; der Bau scheitert darum wie bei
/// einem fehlenden Schritt, statt nur auf stderr zu warnen, das ein
/// Bauhelfer im Erfolgsfall verschluckt (`vec` hat heute keine Byte-Form).
#[test]
fn a_persist_var_without_a_read_path_fails_the_build() {
    let dir = scratch("takt-embed-persist-vec");
    let src = dir.join("keeper.takt");
    std::fs::write(
        &src,
        "system:\n    language = 1\n    tick     = 10 ms\n\noutput led : bool @ hw(\"o/led\") with safe = false\n\n\
         machine m:\n    persist var x : vec<int, 3> = [1, 2, 3]\n\n    initial RUN\n\n    state RUN:\n        loop:\n            led = true\n",
    )
    .expect("Programm");
    let src = src.to_str().expect("Pfad");
    let ir = dir.join("keeper.ll");
    let run = takt(&["build", src, "--emit", "ir", "--out", ir.to_str().expect("Pfad")]);
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success(), "ein Bau ohne Lesepfad bestand: {err}");
    assert!(err.contains("`persist var` ohne Lesepfad"), "{err}");
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_some() {
        let out = dir.join("out");
        let run = takt(&[
            "build",
            src,
            "--emit",
            "embed",
            "--target",
            host_triple(),
            "--form",
            "logical",
            "--out",
            out.to_str().expect("Pfad"),
        ]);
        assert!(!run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    }
}

/// Ein Programm mit dem Geraet `sys` (12.7): `previous_run` stellt der Wirt,
/// `next_run` fuehrt er aus; dazu ein gewoehnlicher Ausgang.
const SYS_PROGRAM: &str = r#"system:
    language = 1
    tick     = 10 ms

input  previous_run : PreviousRun @ hw("sys/previous_run")
output next_run     : NextRun     @ hw("sys/next_run") with safe = NONE
output led          : bool        @ hw("ui/led")       with safe = false

machine m:
    initial RUN

    state RUN:
        enter:
            led = previous_run.or(NONE) == ENDED

        after 100 ms: -> DONE

    state DONE:
        enter:
            next_run = AFTER(delay = 1 s)
"#;

/// 12.11 (GEN-037): Die Kanaele des Geraets `sys` sind keine Treiber. Das
/// Manifest nennt sie in einer eigenen Zeile `sys`, die Zeile `drivers` nur
/// die Treiber, und `next_run` steht unter den Diensten.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn the_manifest_keeps_sys_channels_apart_from_drivers() {
    let Some(_) = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let dir = scratch("takt-embed-sys");
    let src = dir.join("plant.takt");
    std::fs::write(&src, SYS_PROGRAM).expect("Programm");
    let out = dir.join("out");
    let run = takt(&[
        "build",
        src.to_str().expect("Pfad"),
        "--emit",
        "embed",
        "--target",
        host_triple(),
        "--form",
        "logical",
        "--out",
        out.to_str().expect("Pfad"),
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let manifest = std::fs::read_to_string(out.join("plant.manifest")).expect("Manifest");
    let line = |key: &str| {
        let prefix = format!("{key} = ");
        manifest.lines().find_map(|l| l.strip_prefix(&prefix)).unwrap_or_else(|| panic!("{key} fehlt:\n{manifest}"))
    };
    assert_eq!(line("drivers"), "plant_out_ui_led, plant_alive_ui", "{manifest}");
    assert_eq!(line("sys"), "plant_sys_previous_run", "{manifest}");
    assert_eq!(line("services"), "next_run", "{manifest}");
}

/// Die Baufehler der Lieferform: eine unbekannte Form nennt die sechs, die es
/// gibt; ohne `--out` gibt es kein Verzeichnis.
#[test]
fn an_unknown_form_and_a_missing_out_are_named() {
    let run = takt(&["build", VALVE, "--emit", "embed", "--target", host_triple(), "--form", "banana"]);
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("--form: `banana` unbekannt; bekannt: own, interrupt, poll, rtos, linux, logical"), "{err}");
    let run = takt(&["build", VALVE, "--emit", "embed", "--target", host_triple(), "--form", "logical"]);
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(err.contains("--emit embed schreibt in ein Verzeichnis; `--out VERZEICHNIS` fehlt"), "{err}");
}

/// Die Lieferform fuer den ESP32-C6 (`xip_flash`, 12.3) in `dir`.
fn xip_delivery(dir: &Path) -> Output {
    takt(&[
        "build",
        VALVE,
        "--emit",
        "embed",
        "--target",
        "riscv32imac-unknown-none-elf",
        "--form",
        "own",
        "--prefix",
        "valve",
        "--hardware",
        "corpus-try/hw/esp32c6.hw",
        "--out",
        dir.to_str().expect("Pfad"),
    ])
}

/// **Unter `xip_flash` legen Fragmente den Tick-Pfad in den RAM** (12.3,
/// M11 Schritt 11): `P_ram.x` fuer GNU ld und lld, `P.lf` fuer `ldgen`; das
/// Manifest sagt `xip_flash` und nennt die Einstiege des Tick-Pfads. Ohne
/// Instruktions-RAM gibt es keine Fragmente.
#[test]
fn xip_flash_brings_the_fragments_and_the_tick_path() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let dir = scratch("takt-embed-xip");
    let run = xip_delivery(&dir);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let ram = std::fs::read_to_string(dir.join("valve_ram.x")).expect("Fragment fuer ld");
    assert!(ram.contains("*libvalve.a:(.text .text.*"), "{ram}");
    let lf = std::fs::read_to_string(dir.join("valve.lf")).expect("Fragment fuer ldgen");
    assert!(lf.contains("archive: libvalve.a"), "{lf}");
    let manifest = std::fs::read_to_string(dir.join("valve.manifest")).expect("Manifest");
    assert!(manifest.contains("xip_flash = true"), "{manifest}");
    assert!(manifest.contains("tick_path = valve_tick, valve_commit,"), "{manifest}");
    assert!(manifest.contains("arena_symbol = valve_arena"), "{manifest}");

    let plain = scratch("takt-embed-no-xip");
    let run = takt(&[
        "build",
        VALVE,
        "--emit",
        "embed",
        "--target",
        host_triple(),
        "--form",
        "logical",
        "--out",
        plain.to_str().expect("Pfad"),
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert!(!plain.join("valve_ram.x").exists() && !plain.join("valve.lf").exists(), "ohne `iram` keine Fragmente");
    let manifest = std::fs::read_to_string(plain.join("valve.manifest")).expect("Manifest");
    assert!(manifest.contains("xip_flash = false"), "{manifest}");
}

/// **`P.lf` liest der Parser von ESP-IDF** (12.3, 12.11): eine Abbildung
/// `takt_valve` fuer `libvalve.a`, alles nach `noflash`. Braucht ESP-IDF
/// (`IDF_PATH`) und ein Python mit `pyparsing` (`TAKT_IDF_PYTHON`, sonst
/// `python`).
#[test]
#[ignore = "ESP-IDF: IDF_PATH und ein Python mit pyparsing (TAKT_IDF_PYTHON); mit --ignored"]
fn the_ldgen_fragment_parses_with_esp_idf() {
    let idf = std::env::var("IDF_PATH").expect("IDF_PATH nennt ESP-IDF");
    let python = std::env::var("TAKT_IDF_PYTHON").unwrap_or_else(|_| "python".to_string());
    let dir = scratch("takt-embed-ldgen");
    let run = xip_delivery(&dir);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let script = "import sys\n\
                  sys.path.insert(0, sys.argv[1])\n\
                  from ldgen.fragments import parse_fragment_file\n\
                  for f in parse_fragment_file(sys.argv[2], None).fragments:\n    \
                  print(type(f).__name__, f.name, f.archive, sorted(str(e) for e in f.entries))\n";
    let out = Command::new(python)
        .args(["-c", script])
        .arg(Path::new(&idf).join("tools").join("ldgen"))
        .arg(dir.join("valve.lf"))
        .output()
        .expect("Python startet");
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    assert!(text.starts_with("Mapping takt_valve libvalve.a"), "{text}");
    assert!(text.contains("noflash"), "{text}");
}
