//! Zeigt den erzeugten Testrahmen und seine Speicherform.
//!
//! `cargo run -p takt-conformance --example rahmen -- corpus-try/12_bitfields.takt`

fn main() {
    let path = std::env::args().nth(1).expect("Datei angeben");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let o = takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &o);
    let Some(p) = out.program else { return };
    let machine = p.machines.first().map(|m| m.name.clone()).unwrap_or_default();
    let h = takt_conformance::harness::build(&p, &machine, 3);
    println!("{}", h.source);
    eprintln!("--- Speicherform ---");
    eprintln!("Abbild {} B, Latch {} B, Parameter {} B", h.layout.image, h.layout.latch, h.layout.params);
    for s in &h.layout.outputs {
        eprintln!("  out {} @ {} ({} B, {:?})", s.name, s.offset, s.size, s.ty);
    }
}
