//! Mehrere Programme in einem Abbild (12.11, M11 Schritt 4): jedes mit
//! seinem Praefix und seiner Arena, die Bibliotheken, die sie teilen, ohne
//! veraenderlichen Zustand.

mod common;

use takt_conformance::harness;
use takt_conformance::run::compare;
use takt_frame::mcu::Frame;
use takt_llvm::inspect::Binutils;
use takt_llvm::symbols::Prefix;
use takt_llvm::toolchain::Clang;

const TICKS: u64 = 60;

/// Zwei Programme mit Faults, Folgen und einem Ausgabestrom.
const PROGRAMS: [(&str, &str); 2] = [("pa", "03_sequences_and_faults.takt"), ("pb", "13_framing.takt")];

/// Die Leitung des Boards fuer beide Rahmen: `current` waehlt die Datei des
/// Programms, das gerade rechnet; die Zahlen wie in `takt-bringup-host`.
const BOARD: &str = r#"#include <stdio.h>
#include <string.h>

static FILE *out[2];
static int current;

void takt_board_trace(const char *line) { fputs(line, out[current]); }
void takt_board_trace_i64(long long v) { fprintf(out[current], "%lld ", v); }
void takt_board_trace_u64(unsigned long long v) { fprintf(out[current], "%llu ", v); }
void takt_board_trace_f64(double v) { fprintf(out[current], "%.17g ", v); }
void takt_board_trace_hex8(unsigned char v) { fprintf(out[current], "0x%02x", v); }
"#;

/// **Zwei Programme in einem Wirtsprozess rechnen wie allein.** Jedes hat
/// sein Praefix, seine Arena und seinen MCU-Rahmen; ein `main` startet beide
/// und tickt sie abwechselnd, wie die Schleife der Boards es fuer eines tut.
/// Fehlte einem externen Symbol das Praefix, linkte der Prozess nicht;
/// teilten die Programme Zustand, saehe eines die Spuren des anderen. Beide
/// Koepfe stehen in derselben Uebersetzungseinheit, jede Arena hat ihren Typ
/// (12.11). Die Kanaele an Hardware bekommen ausdruecklich Stummel (12.6).
#[test]
fn two_programs_run_side_by_side_like_the_interpreter() {
    let Some(clang) = common::clang_path() else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-two-programs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");

    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let mut sources = Vec::new();
    let mut heads = String::new();
    let mut main = String::from(BOARD);
    let mut calls = String::new();
    let mut programs = Vec::new();
    for (i, (name, file)) in PROGRAMS.iter().enumerate() {
        let prefix = Prefix::new(name).expect("Praefix");
        let p = common::board::corpus(file);
        let ir = takt_llvm::lower::program(&p, host.triple, &prefix).ir;
        let frame =
            takt_frame::mcu::build_with(&p, Frame { prefix: prefix.clone(), stubs: true, ..Default::default() });
        let (ll, c) = (dir.join(format!("{name}.ll")), dir.join(format!("{name}.c")));
        std::fs::write(&ll, ir).expect("IR");
        std::fs::write(&c, frame.source).expect("Rahmen");
        std::fs::write(dir.join(format!("{name}.h")), frame.header).expect("Kopf");
        sources.extend([ll, c]);
        heads.push_str(&format!("#include \"{name}.h\"\n"));
        main.push_str(&format!("static struct {name}_arena {name}_arena;\n"));
        calls.push_str(&format!(
            "        current = {i}; {name}_tick(&{name}_arena, k); {name}_commit(&{name}_arena); \
             {name}_dump(&{name}_arena, 0);\n"
        ));
        programs.push((name, p, dir.join(format!("{name}.trace"))));
    }
    main.push_str("\nint main(void) {\n");
    for (i, (name, _, trace)) in programs.iter().enumerate() {
        let path = trace.display().to_string().replace('\\', "/");
        main.push_str(&format!("    out[{i}] = fopen(\"{path}\", \"w\");\n    if (!out[{i}]) return 1;\n"));
        main.push_str(&format!("    current = {i}; {name}_init(&{name}_arena, 0); {name}_dump(&{name}_arena, 1);\n"));
    }
    main.push_str(&format!("    for (long long k = 1; k <= {TICKS}; k++) {{\n{calls}    }}\n"));
    main.push_str("    fclose(out[0]);\n    fclose(out[1]);\n    return 0;\n}\n");
    let main_c = dir.join("main.c");
    std::fs::write(&main_c, heads + &main).expect("main");

    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    let natives = harness::native_library().expect("Natives");
    let mut cmd = std::process::Command::new(&clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .args(&sources)
        .arg(&main_c)
        .arg(&natives)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(run.status.success(), "der Lauf brach ab: {}", run.status);

    for (name, p, trace) in &programs {
        let native = std::fs::read_to_string(trace).expect("Trace");
        let interpreted = common::board::run_interpreted(p);
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty() && native.lines().any(|l| l.contains(" out ")),
            "{name}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
            diffs.len(),
            diffs.iter().take(8).map(ToString::to_string).collect::<Vec<_>>().join("\n"),
            interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
            native.lines().take(12).collect::<Vec<_>>().join("\n")
        );
    }
}

/// **Zwei Instanzen desselben Programms teilen nichts als den Code**
/// (12.11; KON2-032). `101_correct_math` ruft `libtaktm`, `20_native` die
/// Natives; das erste laeuft in zwei Arenen, die vor `init` mit 0xA5
/// gefuellt sind, die zweite Instanz nur jeden zweiten Durchlauf — eigene
/// Ticks. Jede Instanz rechnet wie der Interpreter ueber ihre Tickzahl;
/// teilten sie Zustand, saehe eine die Spuren der anderen.
#[test]
fn instances_of_one_program_share_nothing() {
    let Some(clang) = common::clang_path() else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-two-instances");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let mut sources = Vec::new();
    let mut main = BOARD.replace("static FILE *out[2];", "static FILE *out[3];");
    let mut heads = String::new();
    let programs = [("ma", "101_correct_math.takt"), ("nb", "20_native.takt")];
    let mut compiled = Vec::new();
    for (name, file) in programs {
        let prefix = Prefix::new(name).expect("Praefix");
        let p = common::board::corpus(file);
        let frame =
            takt_frame::mcu::build_with(&p, Frame { prefix: prefix.clone(), stubs: true, ..Default::default() });
        let (ll, c) = (dir.join(format!("{name}.ll")), dir.join(format!("{name}.c")));
        std::fs::write(&ll, takt_llvm::lower::program(&p, host.triple, &prefix).ir).expect("IR");
        std::fs::write(&c, frame.source).expect("Rahmen");
        std::fs::write(dir.join(format!("{name}.h")), frame.header).expect("Kopf");
        sources.extend([ll, c]);
        heads.push_str(&format!("#include \"{name}.h\"\n"));
        compiled.push(p);
    }
    // Drei Instanzen: zwei Arenen von `ma`, eine von `nb`; je eine Datei.
    let instances = [("ma", 0usize, 1u64), ("ma", 1, 2), ("nb", 0, 1)];
    for (i, (name, n, _)) in instances.iter().enumerate() {
        main.push_str(&format!("static struct {name}_arena arena{i}; /* {name} #{n} */\n"));
    }
    main.push_str("\nint main(void) {\n");
    for (i, (name, _, _)) in instances.iter().enumerate() {
        let path = dir.join(format!("i{i}.trace")).display().to_string().replace('\\', "/");
        main.push_str(&format!("    out[{i}] = fopen(\"{path}\", \"w\");\n    if (!out[{i}]) return 1;\n"));
        main.push_str(&format!("    memset(&arena{i}, 0xA5, sizeof arena{i});\n"));
        main.push_str(&format!("    current = {i}; {name}_init(&arena{i}, 0); {name}_dump(&arena{i}, 1);\n"));
    }
    main.push_str(&format!("    for (long long k = 1; k <= {TICKS}; k++) {{\n"));
    for (i, (name, _, every)) in instances.iter().enumerate() {
        main.push_str(&format!(
            "        if (k % {every} == 0) {{ current = {i}; {name}_tick(&arena{i}, k / {every}); \
             {name}_commit(&arena{i}); {name}_dump(&arena{i}, 0); }}\n"
        ));
    }
    main.push_str("    }\n    for (int i = 0; i < 3; i++) fclose(out[i]);\n    return 0;\n}\n");
    let main_c = dir.join("main.c");
    std::fs::write(&main_c, heads + &main).expect("main");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    let natives = harness::native_library().expect("Natives");
    let mut cmd = std::process::Command::new(&clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .args(&sources)
        .arg(&main_c)
        .arg(&natives)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(run.status.success(), "der Lauf brach ab: {}", run.status);
    for (i, (name, n, every)) in instances.iter().enumerate() {
        let p = &compiled[usize::from(*name == "nb")];
        let native = std::fs::read_to_string(dir.join(format!("i{i}.trace"))).expect("Trace");
        let options = takt_interp::RunOptions { ticks: TICKS / every, ..Default::default() };
        let interpreted = takt_interp::run(p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
        let interpreted = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(p));
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty() && native.lines().any(|l| l.contains(" out ")),
            "{name} #{n}: {} Abweichungen\n{}",
            diffs.len(),
            diffs.iter().take(8).map(ToString::to_string).collect::<Vec<_>>().join("\n")
        );
    }
}

/// Die Symbole geteilter Bibliotheken in `.data` oder `.bss` einer
/// Bibliothek; `__imp_*` sind die Adressfelder, die PE/COFF fuer jedes Symbol
/// eines Archivs anlegt, der Zustand stuende im Symbol selbst.
fn mutable_shared(symbols: &[takt_llvm::inspect::Symbol]) -> (bool, Vec<String>) {
    let shared = |name: &str| {
        ["libtaktm", "takt_native", "takt_hal", "takt_crypto", "takt_edge_", "takt_m_"].iter().any(|c| name.contains(c))
    };
    let any = symbols.iter().any(|s| shared(&s.name));
    let mutable = symbols
        .iter()
        .filter(|s| {
            matches!(s.kind.to_ascii_lowercase(), 'd' | 'b') && shared(&s.name) && !s.name.starts_with("__imp_")
        })
        .map(|s| format!("{} ({})", s.name, s.kind))
        .collect();
    (any, mutable)
}

/// **Die geteilten Bibliotheken haben keinen veraenderlichen Zustand.**
/// `libtaktm`, die kuratierten Natives und der Randkern liegen einmal im
/// Abbild, auch bei mehreren Programmen und Instanzen (12.11); darum steht
/// keines ihrer Symbole in `.data` oder `.bss`. Geprueft wird die
/// Bibliothek, die der Wirtsrahmen bindet.
#[test]
fn the_shared_libraries_hold_no_mutable_state() {
    let Some(_) = common::clang_path() else { return };
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let lib = harness::native_library().expect("Natives");
    let symbols = Binutils::best_for(host).symbols(&lib).expect("`nm` liest die Bibliothek des Wirts");
    let (any, mutable) = mutable_shared(&symbols);
    assert!(any, "die Bibliothek nennt keine geteilten Symbole");
    assert!(mutable.is_empty(), "veraenderlicher Zustand in geteilten Bibliotheken:\n{}", mutable.join("\n"));
}

/// **Auch die Bibliotheken der Boards haben keinen** (12.11; KON2-033):
/// dieselbe Pruefung an den Archiven fuer Cortex-M4F und RV32IMAC, gelesen
/// mit dem `nm` aus `llvm-tools`, das jedes Ziel liest.
#[test]
fn the_board_libraries_hold_no_mutable_state() {
    let Some(_) = common::clang_path() else { return };
    let tools = takt_testkit::require("llvm-tools", Binutils::llvm(), "`rustup component add llvm-tools`");
    let Some(tools) = tools else { return };
    // Die Rust-Ziele der Boards; das von RV32IMAC heisst anders als sein LLVM-Triple.
    for rust in ["thumbv7em-none-eabihf", "riscv32imac-unknown-none-elf"] {
        let lib = harness::native_library_for(Some(rust)).unwrap_or_else(|e| panic!("{rust}: {e}"));
        let symbols = tools.symbols(&lib).unwrap_or_else(|| panic!("{rust}: `nm` liest {}", lib.display()));
        let (any, mutable) = mutable_shared(&symbols);
        assert!(any, "{rust}: die Bibliothek nennt keine geteilten Symbole");
        assert!(mutable.is_empty(), "{rust}: veraenderlicher Zustand:\n{}", mutable.join("\n"));
    }
}
