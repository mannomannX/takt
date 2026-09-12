//! Zeigt den Rahmen mit Eingaben und laesst ihn laufen.
fn main() {
    let path = std::env::args().nth(1).expect("Datei");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &o);
    let Some(p) = out.program else { return };
    let machine = p.machines.first().map(|m| m.name.clone()).unwrap_or_default();
    let inputs = vec![(3u64, "go".to_string()), (40, "go".to_string())];
    let h = takt_conformance::harness::build_with(&p, &machine, 60, &inputs);
    println!("{}", h.source);
}
