//! Der Bauhelfer mit dem echten Werkzeug (12.11): Ein Programm, das nicht
//! uebersetzt, laesst den Bau mit den Meldungen von `takt build` scheitern.

use std::path::{Path, PathBuf};

#[test]
fn a_broken_program_fails_the_build_with_its_diagnostics() {
    let tool = std::env::var_os("TAKT").map(PathBuf::from).expect("`TAKT` nennt das Werkzeug (FB-392)");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("broken-build");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let source = dir.join("broken.takt");
    std::fs::write(
        &source,
        "system:\n    language = 1\n    tick     = 10 ms\n\noutput o : int in 0..9 @ sim(\"o/o\")\n\n\
         machine m:\n    initial A\n\n    state A:\n        loop:\n            o = gibt_es_nicht\n",
    )
    .expect("Quelle");
    let triple = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let call = takt_embed::build::Program::new(&source).invocation(&dir, triple);
    let lines = call.run(Path::new(&tool)).expect_err("ein kaputtes Programm baut nicht");
    assert!(lines.iter().any(|l| l.contains("gibt_es_nicht")), "die Meldung nennt den Namen: {lines:?}");
    assert!(!call.module.exists(), "kein Modul aus einem gescheiterten Bau");
}
