//! Jede Formatversion der MIR bleibt lesbar (11.3, grammar/mir-format.md W5).
//!
//! `mir-golden/vN.mir` hat der Compiler geschrieben, mit dem Version N
//! eingefuehrt wurde — gebaut aus seinem Commit, nicht nachgestellt; die
//! Quelle ist `mir-golden/program.takt`. Version 1 hatte noch keinen
//! Uebersetzer, der eine Datei haette schreiben koennen. Ein neuer
//! Versionssprung legt seine Datei dazu.
//!
//! Lesbar heisst zweierlei: Der heutige Leser nimmt die Datei an, und der
//! Interpreter fuehrt das gelesene Programm genauso aus wie das frisch
//! uebersetzte — ein Feld, das die alte Datei nicht kannte, liest er mit
//! seinem Default, und der muss dasselbe bedeuten.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::format::{FORMAT_VERSION, read_program};
use takt_sema::{Build, Options};

fn dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mir-golden")
}

fn trace(p: &takt_mir::Program) -> String {
    let stimulus = Trace::parse("t=0 in p 1 bar\nt=5 in p 8 bar\nt=30 in p 2 bar\n").expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks: 40, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn every_format_version_reads_and_runs_like_today() {
    let src = std::fs::read_to_string(dir().join("program.takt")).expect("Quelle");
    let options = Options { policy: Policy::default(), build: Build::Sim, ..Default::default() };
    let today = takt_sema::compile(&src, &options).program.expect("Programm");
    let want = trace(&today);
    assert!(want.contains("state m OPEN"), "der Stimulus soll beide Zustaende zeigen:\n{want}");
    for version in 2..=FORMAT_VERSION {
        let path = dir().join(format!("v{version}.mir"));
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e} (Datei der Version fehlt)", path.display()));
        let (header, old) = read_program(&bytes).unwrap_or_else(|e| panic!("v{version}: {e:?}"));
        assert_eq!(header.format_version, version, "v{version}: Kopf");
        let names = |p: &takt_mir::Program| {
            let m = &p.machines.iter().find(|m| m.name == "m").expect("Maschine m");
            (m.states.iter().map(|s| s.name.clone()).collect::<Vec<_>>(), p.channels.len())
        };
        assert_eq!(names(&old), names(&today), "v{version}: Gestalt");
        assert_eq!(trace(&old), want, "v{version}: der Interpreter rechnet anders");
    }
}
