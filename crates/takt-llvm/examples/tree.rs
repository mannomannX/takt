//! Zeigt den Zustandsbaum einer Maschine nach dem Entzuckern.
//!
//! `cargo run -p takt-llvm --example tree -- corpus-try/03_sequences_and_faults.takt`

fn main() {
    let path = std::env::args().nth(1).expect("Datei angeben");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let Some(p) = out.program else { return };
    for m in &p.machines {
        println!("machine {} — {} Zustaende, Tiefe {}", m.name, m.states.len(), takt_llvm::machine::depth(m));
        for (i, s) in m.states.iter().enumerate() {
            let tiefe = takt_llvm::machine::path_to(m, takt_mir::StateId(i as u32)).len();
            let blatt = if s.children.is_empty() { "Blatt" } else { "" };
            println!("  {:>2} {}{} {}", i, "  ".repeat(tiefe - 1), s.name, blatt);
        }
    }
}
