//! Zeigt die Schrittfunktion einer Maschine aus dem Korpus.
//!
//! `cargo run -p takt-llvm --example machine -- corpus-try/01_minimal.takt`

use takt_llvm::emit::Module;
use takt_llvm::machine::{declare_state, state_struct};
use takt_llvm::step::{init_function, step_function};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "corpus-try/01_minimal.takt".into());
    let src = std::fs::read_to_string(&path).expect("Quelle lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    for d in out.diagnostics.iter().filter(|d| d.is_error()) {
        eprintln!("{d}");
    }
    let Some(p) = out.program else { return };
    let mut m = Module::new(&path, "x86_64-pc-windows-msvc");
    takt_llvm::abi::Abi::declare(&mut m);
    // Bloecke zuerst: Die Maschinen halten ihre Instanzen (5.7).
    for b in &p.blocks {
        let Some(inst) = takt_llvm::block::instance_of(b, &p) else { continue };
        takt_llvm::block::declare(b, &inst, &mut m);
        for fid in b.step.iter().chain(&b.methods) {
            let Some(f) = p.fns.get(fid.index()) else { continue };
            if let Err(e) = takt_llvm::fns::block_method(b, f, &p, &mut m) {
                eprintln!("; block {}.{}: {e:?}", b.name, f.name);
            }
        }
    }
    // Reine Funktionen: Die Maschinen rufen sie (4.4). Die Methoden der
    // Bloecke stehen auch in `p.fns`, sind aber schon geschrieben.
    let methoden: Vec<_> = p.blocks.iter().flat_map(|b| b.step.iter().chain(&b.methods).copied()).collect();
    for (i, f) in p.fns.iter().enumerate() {
        if methoden.contains(&takt_mir::FnId(i as u32)) {
            continue;
        }
        if let Err(e) = takt_llvm::fns::function(f, &p, &mut m) {
            eprintln!("; fn {}: {e:?}", f.name);
        }
    }
    for machine in &p.machines {
        let Some(st) = state_struct(machine, &p) else {
            eprintln!("; {}: Zustand nicht abbildbar", machine.name);
            continue;
        };
        declare_state(machine, &st, &mut m);
        if let Err(e) = init_function(machine, &st, &p, &mut m) {
            eprintln!("; {}: init: {e:?}", machine.name);
        }
        if let Err(e) = step_function(machine, &st, &p, &mut m) {
            eprintln!("; {}: {e:?}", machine.name);
        }
    }
    print!("{}", m.finish());
}
