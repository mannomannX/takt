//! Groessen-Baseline (plan/codegen-hebel.md 5): Das Objekt je
//! Korpusprogramm fuer `riscv32imac` und `thumbv7em` darf nicht still
//! wachsen, und ein Schrumpfen zieht die Baseline nach, damit sie nicht
//! spaeteres Wachstum verdeckt. Die Zahlen stehen in `size_baseline.txt`;
//! `UPDATE_SIZE_BASELINE=1` schreibt sie neu und nennt die Aenderungen.
//! Ohne clang scheitert der Test wie die LLVM-Tests (`takt_testkit::require`);
//! `llvm-size` muss neben clang liegen.

use std::path::{Path, PathBuf};
use std::process::Command;

use takt_llvm::toolchain::{find, object_flags};
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
    let o = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &o);
    out.program.unwrap_or_else(|| panic!("{name}: uebersetzt nicht"))
}

/// Die Ziele, je eine Spalte der Baseline.
const TARGETS: [Target; 2] = [Target::RISCV32IMAC, Target::THUMBV7EM];

/// Die `.text`-Groesse des Objekts, wie `llvm-size` sie nennt.
fn text_size(clang: &Path, ir: &str, dir: &Path, name: &str, target: Target) -> u64 {
    let ll = dir.join(format!("{name}_{}.ll", target.name));
    let obj = dir.join(format!("{name}_{}.o", target.name));
    std::fs::write(&ll, ir).expect("IR schreibbar");
    let mut cmd = Command::new(clang);
    cmd.args(["-c", "-Wno-override-module", "-ffreestanding", "-nostdlib"]).arg(format!("--target={}", target.triple));
    if !target.march.is_empty() {
        cmd.arg(format!("-march={}", target.march));
    }
    let ok = cmd.args(object_flags(target.triple)).arg(&ll).arg("-o").arg(&obj).status().is_ok_and(|s| s.success());
    assert!(ok, "{name}: clang schlug fehl fuer {}", target.name);
    let size = clang.with_file_name(if cfg!(windows) { "llvm-size.exe" } else { "llvm-size" });
    let out = Command::new(&size)
        .arg(&obj)
        .output()
        .unwrap_or_else(|e| panic!("`{}` neben clang fehlt: {e}", size.display()));
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().nth(1).unwrap_or_else(|| panic!("{name}: llvm-size ohne Zeile:\n{text}"));
    line.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("{name}: {line}"))
}

/// Je Zeile `<programm> <riscv32imac> <thumbv7em>`.
fn read_baseline() -> Vec<(String, Vec<u64>)> {
    std::fs::read_to_string(baseline_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let name = w.next()?.to_string();
            let bytes: Option<Vec<u64>> = w.map(|n| n.parse().ok()).collect();
            Some((name, bytes?))
        })
        .collect()
}

#[test]
fn the_objects_of_the_corpus_stay_within_the_baseline() {
    let Some(clang) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-size-baseline");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let measured: Vec<(String, Vec<u64>)> = PROGRAMS
        .iter()
        .map(|name| {
            let p = program(name);
            let sizes = TARGETS
                .iter()
                .map(|target| {
                    let instrument = Instrument::default_for(p.config.runtime_profile(), *target);
                    let out = takt_llvm::lower::program_with(
                        &p,
                        target.triple,
                        &takt_llvm::symbols::Prefix::default(),
                        instrument,
                    );
                    // Ein unvollstaendiges Objekt waere kleiner, ohne dass
                    // der Codegen etwas gewonnen haette.
                    assert!(out.complete(), "{name}: {:?}", out.skipped.iter().map(|s| &s.reason).collect::<Vec<_>>());
                    text_size(&clang, &out.ir, &dir, name.trim_end_matches(".takt"), *target)
                })
                .collect();
            (name.to_string(), sizes)
        })
        .collect();
    let baseline = read_baseline();
    let mut changed = Vec::new();
    for (name, bytes) in &measured {
        let Some((_, was)) = baseline.iter().find(|(n, _)| n == name) else {
            changed.push(format!("{name}: {bytes:?} Byte, ohne Baseline"));
            continue;
        };
        for (i, target) in TARGETS.iter().enumerate() {
            let (now, before) = (bytes[i], was.get(i).copied().unwrap_or(0));
            let upper = (before as f64 * (1.0 + SLACK)) as u64 + SLACK_BYTES;
            let lower = ((before as f64 * (1.0 - SLACK)) as u64).saturating_sub(SLACK_BYTES);
            if now > upper {
                changed.push(format!("{name} ({}): {before} -> {now} Byte, gewachsen", target.name));
            } else if now < lower {
                changed.push(format!("{name} ({}): {before} -> {now} Byte, geschrumpft", target.name));
            }
        }
    }
    if std::env::var_os("UPDATE_SIZE_BASELINE").is_some() {
        let text: String = measured
            .iter()
            .map(|(n, b)| format!("{n} {}\n", b.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")))
            .collect();
        std::fs::write(baseline_path(), text).expect("Baseline schreibbar");
        eprintln!("Baseline neu geschrieben:\n{}", changed.join("\n"));
        return;
    }
    assert!(
        changed.is_empty(),
        "Objekte jenseits der Baseline (UPDATE_SIZE_BASELINE=1 schreibt sie neu):\n{}",
        changed.join("\n")
    );
}
