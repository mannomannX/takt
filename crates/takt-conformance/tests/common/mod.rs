//! Was beide Abnahmetests brauchen: bauen und ausfuehren.

use takt_conformance::harness;
use takt_llvm::emit::Module;
use takt_llvm::toolchain::Clang;
use takt_mir::program::Program;

/// Erzeugt die IR eines Programms, so wie der Compiler sie erzeugt.
pub fn ir_of(p: &Program) -> String {
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

/// Uebersetzt ein Programm mit seinem Testrahmen und fuehrt es aus.
pub fn run_native(clang: &Clang, p: &Program, name: &str, machine: &str, ticks: u64) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("takt-abnahme-{}", name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, ir_of(p)).map_err(|e| e.to_string())?;
    let h = harness::build(p, machine, ticks);
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
        return Err(String::from_utf8_lossy(&build.stderr).to_string());
    }
    let out = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}
