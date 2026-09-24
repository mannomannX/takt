//! Groessen-Baseline (plan/codegen-hebel.md 5): Das Objekt je
//! Korpusprogramm fuer `riscv32imac` darf nicht still wachsen. Die Zahlen
//! stehen in `size_baseline.txt`; `UPDATE_SIZE_BASELINE=1` schreibt sie
//! neu. Ohne clang und llvm-size wird uebersprungen, wie die LLVM-Tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use takt_llvm::toolchain::{Clang, find, object_flags};
use takt_llvm::{Instrument, Target};

const PROGRAMS: &[&str] = &[
    "01_minimal.takt",
    "03_sequences_and_faults.takt",
    "04_blocks_and_multirate.takt",
    "13_framing.takt",
    "30_idle.takt",
    "13_protocol_analysis.takt",
    "17_nested.takt",
    "46_matrices.takt",
    "71_places.takt",
    "78_length_guards.takt",
];

/// Wachstum, das noch keine Meldung ist: zwei Prozent oder 64 Byte.
const SLACK: f64 = 0.02;
const SLACK_BYTES: u64 = 64;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/size_baseline.txt")
}

fn program(name: &str) -> takt_mir::Program {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &o);
    out.program.unwrap_or_else(|| panic!("{name}: uebersetzt nicht"))
}

/// Die `.text`-Groesse des Objekts, wie `llvm-size` sie nennt.
fn text_size(clang: &Path, ir: &str, dir: &Path, name: &str) -> u64 {
    let ll = dir.join(format!("{name}.ll"));
    let obj = dir.join(format!("{name}.o"));
    std::fs::write(&ll, ir).expect("IR schreibbar");
    let target = Target::RISCV32IMAC;
    let ok = Command::new(clang)
        .args(["-c", "-Wno-override-module", "-ffreestanding", "-nostdlib"])
        .arg(format!("--target={}", target.triple))
        .arg(format!("-march={}", target.march))
        .args(object_flags(target.triple))
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{name}: clang schlug fehl");
    let size = clang.with_file_name(if cfg!(windows) { "llvm-size.exe" } else { "llvm-size" });
    let out = Command::new(&size).arg(&obj).output().expect("llvm-size");
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1).unwrap_or_else(|| panic!("{name}: llvm-size ohne Zeile:\n{text}"));
    line.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{name}: {line}"))
}

fn read_baseline() -> Vec<(String, u64)> {
    std::fs::read_to_string(baseline_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (name, bytes) = l.split_once(' ')?;
            Some((name.to_string(), bytes.trim().parse().ok()?))
        })
        .collect()
}

#[test]
fn the_objects_of_the_corpus_stay_within_the_baseline() {
    let Clang::At(clang) = find() else {
        eprintln!("clang fehlt; die Groessen-Baseline bleibt ungeprueft");
        return;
    };
    let dir = std::env::temp_dir().join("takt-size-baseline");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let measured: Vec<(String, u64)> = PROGRAMS
        .iter()
        .map(|name| {
            let p = program(name);
            let instrument = Instrument::default_for(p.config.runtime_profile(), Target::RISCV32IMAC);
            let ir = takt_llvm::lower::program_with(&p, Target::RISCV32IMAC.triple, "baseline", instrument).ir;
            (name.to_string(), text_size(&clang, &ir, &dir, name.trim_end_matches(".takt")))
        })
        .collect();
    if std::env::var_os("UPDATE_SIZE_BASELINE").is_some() {
        let text: String = measured.iter().map(|(n, b)| format!("{n} {b}\n")).collect();
        std::fs::write(baseline_path(), text).expect("Baseline schreibbar");
        return;
    }
    let baseline = read_baseline();
    let mut grown = Vec::new();
    for (name, bytes) in &measured {
        let Some((_, was)) = baseline.iter().find(|(n, _)| n == name) else {
            grown.push(format!("{name}: {bytes} Byte, ohne Baseline"));
            continue;
        };
        let limit = (*was as f64 * (1.0 + SLACK)) as u64 + SLACK_BYTES;
        if *bytes > limit {
            grown.push(format!("{name}: {was} -> {bytes} Byte"));
        }
    }
    assert!(
        grown.is_empty(),
        "Objekte gewachsen (UPDATE_SIZE_BASELINE=1 schreibt die Baseline neu):\n{}",
        grown.join("\n")
    );
}
