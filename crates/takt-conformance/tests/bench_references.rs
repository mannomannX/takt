//! Die Referenzkerne aus 13.8 rechnen, was ihre Takt-Programme rechnen:
//! je Tick derselbe Digest im Interpreter und in der C-Referenz, auf dem
//! Wirt uebersetzt. `takt bench` meldet `same_digest` sonst erst nach einem
//! Lauf auf dem Board — und ein Unterschied dort liesse offen, ob die
//! Referenz falsch rechnet oder der Codegen.

mod common;

use takt_conformance::bench::{KERNELS, kernel_path};
use takt_llvm::toolchain::Clang;

/// Ticks je Kern: mehrere Faults der Sequenz, das Einschwingen der
/// Regelung und mehr als einmal um den Zeilenring.
const TICKS: usize = 400;

/// Ein Messkern, fuer die Simulation uebersetzt.
fn kernel(name: &str) -> takt_mir::Program {
    let path = kernel_path(name, "takt");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Der Trace des Interpreters ueber [`TICKS`] Ticks.
fn interpreted_trace(name: &str, p: &takt_mir::Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS as u64, ..Default::default() };
    takt_interp::run(p, &takt_interp::Trace::default(), &options)
        .unwrap_or_else(|e| panic!("{name}: Interpreter: {e:?}"))
        .trace
        .render()
}

/// Der Digest je Tick im Interpreter.
fn interpreted(name: &str) -> Vec<u64> {
    digests(&interpreted_trace(name, &kernel(name))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Der Digest je Tick aus den Zeilen `t=… out digest …`. Ein Output steht
/// nur im Trace, wenn er sich aendert, und Tick 0 schreibt jeden; eine
/// Zeile, die fehlt oder sich nicht lesen laesst, ist ein Fehler und kein
/// stiller Wert (KON1-035).
fn digests(trace: &str) -> Result<Vec<u64>, String> {
    let mut changes: Vec<Option<u64>> = vec![None; TICKS];
    for line in trace.lines() {
        let mut w = line.split_whitespace();
        let (Some(t), Some("out"), Some("digest"), Some(v)) = (w.next(), w.next(), w.next(), w.next()) else {
            continue;
        };
        let t = t.strip_prefix("t=").and_then(|t| t.parse::<usize>().ok()).ok_or(format!("`{line}`: kein Tick"))?;
        let v = v.parse::<u64>().map_err(|e| format!("`{line}`: {e}"))?;
        if t < TICKS {
            changes[t] = Some(v);
        }
    }
    let mut last = changes[0].ok_or("in Tick 0 steht kein Digest")?;
    Ok(changes
        .into_iter()
        .map(|c| {
            last = c.unwrap_or(last);
            last
        })
        .collect())
}

/// Der Digest je Tick der C-Referenz, auf dem Wirt uebersetzt: dieselben
/// Gleitkommaregeln wie auf dem Board (`-ffp-contract=off`,
/// `-fno-math-errno`), ein Aufruf je Tick.
fn native(clang: &Clang, name: &str) -> Result<Vec<u64>, String> {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-bench-reference-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let main = dir.join("main.c");
    let exe = dir.join(if cfg!(windows) { "reference.exe" } else { "reference" });
    std::fs::write(
        &main,
        format!(
            "#include <stdio.h>\nvoid takt_bench_reference(void);\nunsigned long long \
             takt_bench_reference_digest(void);\nint main(void) {{\n    for (int t = 0; t < {TICKS}; t++) {{\n        \
             takt_bench_reference();\n        printf(\"%llu\\n\", takt_bench_reference_digest());\n    }}\n    return \
             0;\n}}\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(clang.path().ok_or("clang")?);
    let build = Clang::deterministic(&mut cmd)
        .args(["-O2", "-ffp-contract=off", "-fno-math-errno", "-Wall", "-Wextra", "-Werror"])
        .arg(kernel_path(name, "c"))
        .arg(&main)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| e.to_string())?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).to_string());
    }
    let out = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&dir);
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().parse::<u64>().map_err(|e| format!("`{l}`: {e}")))
        .collect()
}

/// **Kern und Referenz rechnen dasselbe** (13.8: „gleiche Semantik
/// inklusive Pruefungen"), Tick fuer Tick.
#[test]
fn every_reference_computes_what_its_kernel_computes() {
    let Some(clang) = common::clang() else { return };
    let mut failed = Vec::new();
    for name in KERNELS {
        let takt = interpreted(name);
        match native(&clang, name) {
            Ok(c) if c.len() != TICKS => failed.push(format!("{name}: {} statt {TICKS} Ticks", c.len())),
            Ok(c) => {
                if let Some(t) = (0..TICKS).find(|t| takt[*t] != c[*t]) {
                    failed.push(format!("{name}: ab Tick {t} Takt {} gegen C {}", takt[t], c[t]));
                }
            }
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Fehlende und unlesbare Digests sind Fehler (KON1-035).
#[test]
fn a_missing_or_unreadable_digest_is_an_error() {
    assert!(digests("t=0 out digest 7\nt=3 out digest 9\n").is_ok_and(|d| d[2] == 7 && d[3] == 9));
    assert!(digests("t=1 out digest 7\n").is_err(), "ohne Digest in Tick 0");
    assert!(digests("t=0 out digest 7\nt=2 out digest x\n").is_err(), "unlesbar");
}

/// **Die Messkerne laufen nativ wie im Interpreter** (13.8, Satz 9.4.4;
/// KON1-035): Was `takt bench` auf dem Board als Takt-Code misst, rechnet
/// der erzeugte Code schon auf dem Wirt Tick fuer Tick wie der Interpreter
/// — sonst stuende `same_digest` erst nach einem Boardlauf in Frage.
#[test]
fn every_kernel_runs_natively_like_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    let mut failed = Vec::new();
    for name in KERNELS {
        let p = kernel(name);
        let interpreted = interpreted_trace(name, &p);
        let native = match common::run_native_all(&clang, &p, &format!("kern_{name}"), TICKS as u64) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf: {e}"));
                continue;
            }
        };
        let diffs = takt_conformance::compare(&interpreted, &native);
        if !diffs.is_empty() {
            failed.push(format!("{name}: {} Abweichungen, etwa {:?}", diffs.len(), &diffs[..diffs.len().min(4)]));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
