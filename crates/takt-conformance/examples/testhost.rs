//! Zeigt die Quellen des Testwirts (M11 Schritt 9) und die Speicherform
//! eines Programms; ein Stimulus ist optional.
//!
//! `cargo run -p takt-conformance --example testhost -- corpus-try/16_timing.takt [STIMULUS.trace]`

use takt_conformance::stimulus::Stimulus;
use takt_conformance::testhost::{Run, host_triple, sources};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("Datei angeben");
    let src = std::fs::read_to_string(&path).expect("lesbar");
    let o = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(&src, &o);
    let Some(p) = out.program else { return };
    let text = args.next().map(|s| std::fs::read_to_string(s).expect("Stimulus lesbar")).unwrap_or_default();
    let stimulus = takt_interp::Trace::parse(&text).expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    let run = Run { ticks: 60, stimulus: &inputs, ..Run::default() };
    match sources(&p, &run, host_triple(), false) {
        Ok(s) => {
            for (name, text) in s.files.iter().filter(|(n, _)| n.ends_with(".c") || n == "app.h") {
                println!("/* --- {name} --- */\n{text}");
            }
        }
        Err(e) => eprintln!("kein Testwirt: {e}"),
    }
    let layout = takt_frame::layout::of(&p);
    eprintln!("--- Speicherform ---");
    eprintln!("Abbild {} B, Latch {} B, Parameter {} B", layout.image, layout.latch, layout.params);
    for s in &layout.outputs {
        eprintln!("  out {} @ {} ({} B, {:?})", s.name, s.offset, s.size, s.ty);
    }
}
