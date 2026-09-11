//! Nennt die Knoten, die `scope` als offen zaehlt, mit ihrer Stelle.
//!
//! `cargo run -p takt-llvm --example find -- corpus-try/02_units_and_data.takt`

fn main() {
    let path = std::env::args().nth(1).expect("Datei angeben");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let Some(p) = out.program else { return };
    for m in &p.machines {
        let mut c = takt_llvm::scope::Coverage::default();
        takt_llvm::scope::machine(m, &mut c);
        if c.open.is_empty() {
            continue;
        }
        println!("{}: {:?}", m.name, c.open);
    }
}
