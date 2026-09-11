//! Die Abnahme von M4: Interpreter ≡ nativ (13.8, Satz 9.4.4).
//!
//! Jedes Korpusprogramm wird zweimal ausgefuehrt — einmal vom
//! Referenzinterpreter, einmal als uebersetztes Binaerprogramm — und die
//! Outputs muessen Zeichen fuer Zeichen gleich sein.
//!
//! **Die Tests ueberspringen sich ohne clang**, wie die uebrigen
//! LLVM-Tests: Der Compiler baut ueberall, die Abnahme laeuft dort, wo
//! die Werkzeugkette steht.

use takt_conformance::{compare, harness};
use takt_llvm::emit::Module;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

/// Die Korpusprogramme, die der Codegen vollstaendig senkt.
const KORPUS: [&str; 11] = [
    "01_minimal.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
];

/// Wie viele Ticks verglichen werden.
///
/// Genug, dass jede `after`-Frist des Korpus feuert (die laengste ist
/// 500 ms bei 10 ms Tick), und wenig genug, dass ein Fehlschlag noch zu
/// lesen ist.
const TICKS: u64 = 60;

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Erzeugt die IR eines Programms, so wie der Compiler sie erzeugt.
fn ir_of(p: &Program) -> String {
    let mut m = Module::new("abnahme", "x86_64-pc-windows-msvc");
    takt_llvm::abi::Abi::declare(&mut m);
    takt_llvm::stream::Streams::declare(&mut m);
    let methoden: Vec<_> = p.blocks.iter().flat_map(|b| b.step.iter().chain(&b.methods).copied()).collect();
    for b in &p.blocks {
        let Some(inst) = takt_llvm::block::instance_of(b, p) else { continue };
        takt_llvm::block::declare(b, &inst, &mut m);
        for fid in b.step.iter().chain(&b.methods) {
            let Some(f) = p.fns.get(fid.index()) else { continue };
            let _ = takt_llvm::fns::block_method(b, f, p, &mut m);
        }
    }
    for (i, f) in p.fns.iter().enumerate() {
        if !methoden.contains(&takt_mir::FnId(i as u32)) {
            let _ = takt_llvm::fns::function(f, p, &mut m);
        }
    }
    for machine in &p.machines {
        let Some(st) = takt_llvm::machine::state_struct(machine, p) else { continue };
        takt_llvm::machine::declare_state(machine, &st, &mut m);
        let _ = takt_llvm::step::init_function(machine, &st, p, &mut m);
        let _ = takt_llvm::step::step_function(machine, &st, p, &mut m);
    }
    m.finish()
}

/// Fuehrt ein Programm nativ aus und liefert seine Ausgabe.
fn run_native(clang: &Clang, p: &Program, name: &str, machine: &str) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("takt-abnahme-{}", name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, ir_of(p)).map_err(|e| e.to_string())?;
    let h = harness::build(p, machine, TICKS);
    std::fs::write(&c, &h.source).map_err(|e| e.to_string())?;
    let path = clang.path().ok_or("clang")?;
    let build = std::process::Command::new(path)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| e.to_string())?;
    if !build.status.success() {
        return Err(format!("{}\n--- Rahmen ---\n{}", String::from_utf8_lossy(&build.stderr), h.source));
    }
    let out = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}

/// Fuehrt dasselbe Programm im Interpreter aus.
fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// **Die Abnahme.** Interpreter und erzeugter Code liefern dieselben
/// Outputs (Satz 9.4.4).
#[test]
fn the_interpreter_and_the_generated_code_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut gescheitert = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        let Some(machine) = p.machines.first().map(|m| m.name.clone()) else { continue };
        let native = match run_native(&clang, &p, name, &machine) {
            Ok(t) => t,
            Err(e) => {
                gescheitert.push(format!("{name}: laesst sich nicht bauen:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() {
            let liste: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            gescheitert.push(format!(
                "{name}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                liste.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(gescheitert.is_empty(), "{}", gescheitert.join("\n\n"));
}
