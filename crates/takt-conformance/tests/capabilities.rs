//! Die Faehigkeitsmatrix (13.8, Schritt 25, FB-377).
//!
//! Codegen und Beweiser nennen in einer Tabelle ohne Platzhalter, was sie
//! tragen (`takt_llvm::support`, `takt_prove::support`). Dieser Test haelt
//! jede Tabelle gegen ihre Komponente, an jedem Programm, das der Korpus
//! hergibt:
//!
//! - Traegt die Tabelle eine Konstruktion des Programms nicht (`No`), muss
//!   die Komponente es ablehnen; sonst ist die Tabelle zu vorsichtig.
//! - Lehnt die Komponente ab, muss das Programm eine Konstruktion haben,
//!   die die Tabelle nicht oder nur teilweise traegt; sonst verschweigt die
//!   Tabelle eine Grenze.
//!
//! Die Matrix steht in `plan/capabilities.csv`, mit der Spalte, ob der
//! Korpus die Konstruktion benutzt. Eine geaenderte Zelle ist eine
//! sichtbare Aenderung; `UPDATE_GOLDEN=1` schreibt die Datei neu.
//!
//! Der Rahmen hat keine Zeile: Er lehnt nichts ab, faellt aber mit dem, was
//! er nicht kann, im strengen Vergleich auf (`run::compare`). Der
//! Interpreter ist die Spezifikation und traegt alles.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use takt_mir::Program;
use takt_mir::census::{Construct, Support, all, census};
use takt_sema::{Build, Options};

/// Eine Komponente mit Tabelle.
struct Component {
    name: &'static str,
    support: fn(Construct) -> Support,
    /// Der Grund, aus dem die Komponente das Programm ablehnt.
    rejects: fn(&Program) -> Option<String>,
}

const COMPONENTS: [Component; 2] = [
    Component { name: "Codegen", support: takt_llvm::support::support, rejects: codegen_rejects },
    Component { name: "Beweiser", support: takt_prove::support::support, rejects: prover_rejects },
];

/// Die Matrix, relativ zum Crate.
const MATRIX: &str = "../../plan/capabilities.csv";

/// Weniger Programme heisst: Die Suche ist gebrochen, nicht der Korpus
/// kleiner geworden.
const MIN_PROGRAMS: usize = 230;

/// Konstruktionen, die kein Programm des Korpus benutzt, mit Grund. Die
/// Liste schrumpft nur: Benutzt der Korpus eine, scheitert der Test, bis
/// sie hier verschwindet; eine neue Konstruktion ohne Programm faellt nie
/// hinein, denn ob eine Tabelle sie traegt, prueft sonst niemand.
const UNUSED: &[(&str, &[&str])] = &[
    (
        "Sequenzen loest die Sema in Zustaende auf (6.2); keine Komponente sieht sie",
        &["Seq(Stmt)", "Seq(Wait)", "Seq(Until)", "Seq(Expect)", "Seq(Repeat)", "Seq(Step)", "Feature(Sequence)"],
    ),
    (
        "die Sema erzeugt den Knoten nie (FB-406)",
        &[
            "Accessor(Seq)",
            "Accessor(Text)",
            "Accessor(Data)",
            "Accessor(TimeWarped)",
            "Accessor(Done)",
            "Accessor(Result)",
            "Accessor(Armed)",
            "Accessor(Remaining)",
            "Type(HandleTrigger)",
        ],
    ),
    (
        "kein Korpusprogramm (FB-406). TODO(M11 Schritt 25): je eines",
        &[
            "Expr(Armed)",
            "Unary(BitNot)",
            "Builtin(TimeInState)",
            "Intrinsic(Ceil)",
            "Intrinsic(Rotl)",
            "Intrinsic(Rotr)",
            "Intrinsic(WrappingSub)",
            "Intrinsic(WrappingMul)",
            "Intrinsic(SaturatingAdd)",
            "Intrinsic(SaturatingSub)",
            "Accessor(Age)",
            "Accessor(T)",
            "Accessor(Malformed)",
            "Accessor(Overflowed)",
            "Accessor(Free)",
            "Accessor(Jitter)",
            "Accessor(Last)",
            "Accessor(StartsWith)",
            "Accessor(Contains)",
            "Accessor(Rate)",
            "Accessor(Truncated)",
            "Stmt(Skip)",
            "Feature(MachineHandler)",
            "Feature(Node)",
        ],
    ),
];

fn codegen_rejects(p: &Program) -> Option<String> {
    let l = takt_llvm::lower::program(p, "x86_64-pc-windows-msvc", &takt_llvm::symbols::Prefix::default());
    let reasons: Vec<String> = l
        .skipped
        .iter()
        .map(|s| format!("{}: {}", s.machine, s.reason))
        .chain(l.without_persist.iter().map(|m| format!("{m}: persist ohne Lesepfad")))
        .collect();
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

/// Ein Programm, das der Beweiser nur zum Teil kodiert, gilt als
/// abgelehnt: Eine Eigenschaft bliebe ungeprueft.
fn prover_rejects(p: &Program) -> Option<String> {
    match takt_prove::encode(p) {
        Err(e) => Some(e.what),
        Ok(model) => model.notes.into_iter().find(|n| n.contains("nicht kodiert")),
    }
}

/// Jede `.takt`-Datei unter `corpus-try` und `crates`, die fehlerfrei
/// uebersetzt (fuer die Simulation, sonst fuer die Hardware); einmal je Lauf.
fn corpus() -> &'static [(String, Program)] {
    static CORPUS: OnceLock<Vec<(String, Program)>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for dir in ["corpus-try", "crates"] {
            collect(&root.join(dir), &mut files);
        }
        files.sort();
        files
            .iter()
            .filter_map(|f| {
                let name = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
                compile(f).map(|p| (name, p))
            })
            .collect()
    })
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("Verzeichniseintrag").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|x| x == "takt") {
            out.push(path);
        }
    }
}

fn compile(path: &Path) -> Option<Program> {
    let src = std::fs::read_to_string(path).ok()?;
    let dir = path.parent()?;
    let channel_imports: std::collections::BTreeMap<String, String> = takt_sema::channel_imports(&src)
        .into_iter()
        .filter_map(|f| std::fs::read_to_string(dir.join(&f)).ok().map(|t| (f, t)))
        .collect();
    [Build::Sim, Build::Hw].into_iter().find_map(|build| {
        let options = Options { build, channel_imports: channel_imports.clone(), ..Default::default() };
        let out = takt_sema::compile(&src, &options);
        if out.diagnostics.iter().any(|d| d.is_error()) { None } else { out.program }
    })
}

/// **Die Wahrheit der Tabellen.** Keine Tabelle verspricht weniger, als
/// ihre Komponente am Korpus kann, und keine verschweigt eine Ablehnung.
#[test]
fn every_table_tells_the_truth_about_its_component() {
    let programs = corpus();
    assert!(programs.len() >= MIN_PROGRAMS, "nur {} Programme im Korpus", programs.len());
    let known: BTreeSet<Construct> = all().into_iter().collect();
    let mut wrong = Vec::new();
    for (name, p) in programs {
        let used = census(p);
        if let Some(c) = used.difference(&known).next() {
            wrong.push(format!("{name}: {c:?} fehlt in `census::all`"));
        }
        for comp in &COMPONENTS {
            let rejected = (comp.rejects)(p);
            let unsupported: Vec<&Construct> =
                used.iter().filter(|&&c| matches!((comp.support)(c), Support::No(_))).collect();
            let limited = used.iter().any(|&c| !matches!((comp.support)(c), Support::Yes));
            match &rejected {
                None if !unsupported.is_empty() => {
                    wrong.push(format!("{} nimmt {name} an, die Tabelle sagt nein zu {unsupported:?}", comp.name));
                }
                Some(why) if !limited => {
                    wrong.push(format!("{} lehnt {name} ab ({why}), die Tabelle traegt alles", comp.name));
                }
                _ => {}
            }
        }
    }
    assert!(wrong.is_empty(), "{} Abweichungen:\n{}", wrong.len(), wrong.join("\n"));
}

/// **Jede Konstruktion hat ein Programm.** Was der Korpus nicht benutzt,
/// steht mit Grund in [`UNUSED`]; die Liste schrumpft nur.
#[test]
fn every_construct_has_a_corpus_program_or_a_reason() {
    let used: BTreeSet<String> = corpus().iter().flat_map(|(_, p)| census(p)).map(|c| format!("{c:?}")).collect();
    let listed: BTreeSet<&str> = UNUSED.iter().flat_map(|(_, names)| names.iter().copied()).collect();
    let mut wrong = Vec::new();
    for c in all() {
        let name = format!("{c:?}");
        match (used.contains(&name), listed.contains(name.as_str())) {
            (false, false) => wrong.push(format!("{name}: kein Korpusprogramm und kein Grund in `UNUSED`")),
            (true, true) => wrong.push(format!("{name}: benutzt der Korpus jetzt; aus `UNUSED` streichen")),
            _ => {}
        }
    }
    let known: BTreeSet<String> = all().iter().map(|c| format!("{c:?}")).collect();
    wrong.extend(listed.iter().filter(|n| !known.contains(**n)).map(|n| format!("{n}: keine Konstruktion")));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// **Die Matrix ist sichtbar.** Jede Zelle steht in `plan/capabilities.csv`;
/// was sich an einer Tabelle oder an der Nutzung im Korpus aendert, aendert
/// die Datei.
#[test]
fn the_matrix_matches_its_golden_file() {
    let programs = corpus();
    let used: BTreeSet<Construct> = programs.iter().flat_map(|(_, p)| census(p)).collect();
    let mut csv = String::from("Konstruktion,");
    for comp in &COMPONENTS {
        csv.push_str(comp.name);
        csv.push(',');
    }
    csv.push_str("Korpus\n");
    for c in all() {
        csv.push_str(&field(&format!("{c:?}")));
        for comp in &COMPONENTS {
            csv.push(',');
            csv.push_str(&field(&(comp.support)(c).cell()));
        }
        csv.push_str(if used.contains(&c) { ",ja\n" } else { ",nein\n" });
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(MATRIX);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &csv).expect("Matrix schreiben");
        return;
    }
    let golden = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(golden == csv, "{} weicht ab; mit UPDATE_GOLDEN=1 neu schreiben und die Aenderung pruefen", path.display());
}

/// Ein CSV-Feld, in Anfuehrungszeichen, wenn es Komma oder Anfuehrungszeichen enthaelt.
fn field(s: &str) -> String {
    if s.contains([',', '"']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}
