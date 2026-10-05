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

/// Was eine alte Datei ueber das Programm sagt, ohne den Ablauf: Kanaele
/// mit Richtung, Bindung und Typ samt Range, Maschinenvariablen mit Typ.
fn shape(p: &takt_mir::Program) -> Vec<String> {
    let ty = |t: takt_mir::TypeId| format!("{:?}", p.types.list[t.index()]);
    let mut out: Vec<String> =
        p.channels.iter().map(|c| format!("{} {:?} {:?} {}", c.name, c.dir, c.binding, ty(c.ty))).collect();
    for m in &p.machines {
        out.extend(m.vars.iter().map(|v| format!("{}.{} {}", m.name, v.name, ty(v.ty))));
    }
    out
}

/// 11.3: Neben Gestalt und Lauf stimmen Typen, Ranges und Bindungen jeder
/// alten Datei mit dem heutigen Programm ueberein.
///
/// Der Logik-Hash gehoert nicht dazu: Er ist ein Hash der MIR, und die
/// enthaelt, was der Compiler beitraegt (Prelude, Analysen). Schon die
/// Datei der heutigen Version 16 hat einen anderen als das frisch
/// uebersetzte Programm, weil der Compiler seit ihrem Commit gewachsen ist.
/// Ob er ueber Formatversionen gleich bleiben muss, legt definition.md
/// nicht fest (11.3 nennt ihn nur als Kennung im Lauf-Header); fest steht
/// hier nur, dass Schreiben und Lesen ihn nicht aendern.
#[test]
fn every_format_version_keeps_types_ranges_and_bindings() {
    let src = std::fs::read_to_string(dir().join("program.takt")).expect("Quelle");
    let options = Options { policy: Policy::default(), build: Build::Sim, ..Default::default() };
    let today = takt_sema::compile(&src, &options).program.expect("Programm");
    for version in 2..=FORMAT_VERSION {
        let bytes = std::fs::read(dir().join(format!("v{version}.mir"))).expect("Datei der Version");
        let (_, old) = read_program(&bytes).unwrap_or_else(|e| panic!("v{version}: {e:?}"));
        assert_eq!(shape(&old), shape(&today), "v{version}");
    }
    let written = takt_mir::format::write_program(&today, "test");
    let (_, back) = read_program(&written).expect("lesbar");
    assert_eq!(takt_mir::hash::logic_hash(&back), takt_mir::hash::logic_hash(&today));
}

/// Die Herkunft jeder Datei (`mir-golden/provenance.csv`): der Commit,
/// dessen Compiler sie schrieb — der, der die Version einfuehrte —, und der,
/// der sie eintrug. Fuer jede Version genau eine Zeile.
#[test]
fn every_format_version_names_its_origin() {
    let text = std::fs::read_to_string(dir().join("provenance.csv")).expect("provenance.csv");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("Version;Geschrieben;Eingetragen"));
    let hash = |h: &str| (7..=40).contains(&h.len()) && h.bytes().all(|b| b.is_ascii_hexdigit());
    let mut seen = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split(';').collect();
        let [version, written, added] = fields[..] else { panic!("drei Felder erwartet: `{line}`") };
        assert!(hash(written) && hash(added), "kein Commit-Hash: `{line}`");
        seen.push(version.parse::<u16>().unwrap_or_else(|_| panic!("Version: `{line}`")));
    }
    assert_eq!(seen, (2..=FORMAT_VERSION).collect::<Vec<_>>(), "je Version genau eine Zeile, aufsteigend");
}
