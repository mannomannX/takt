//! Die Pfade des Solvers je Korpusprogramm (M11 Schritt 28c): Was
//! `takt prove` an einem Programm findet — Gegenbeispiele zu Eigenschaften,
//! Pfade zu erreichbaren Pruefstellen, bewiesen unerreichbare Stellen —,
//! steht unter `corpus-try/paths/`. Die Pfade sind Stimuli, die der
//! Vergleich wie jeden anderen Lauf durch alle drei Ausfuehrer rechnet; die
//! Beweise stehen in einer Beweisdatei (11.3), mit der er das Programm
//! uebersetzt. Der Codegen laesst die bewiesenen Pruefungen aus, der
//! Interpreter prueft sie weiter: Ein falscher Beweis faellt dort auf, und
//! der Vergleich sieht den Unterschied.
//!
//! Erzeugt werden sie mit Solver (`UPDATE_PATHS=1`, `tests/solver_paths.rs`;
//! die Bring-ups binden diese Crate ohne den Beweiser). Ohne ihn prueft der
//! Test, dass jeder Pfad sein Ziel im Interpreter noch erreicht — mit
//! derselben Regel, mit der `takt prove` ihn bestaetigte
//! (`takt_prove::fault_at`); eine Beweisdatei bindet der Hash ihrer Quelle.

use std::path::PathBuf;

use takt_mir::analysis::proof::{Site, render};

/// So tief sucht der Solver; die Pfade enden spaetestens dort.
pub const DEPTH: u32 = 5;

/// So viele Sekunden hat jede Anfrage an den Solver.
pub const TIMEOUT_S: u64 = 10;

/// Wohin ein Pfad fuehrt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Ein Fault an der Pruefstelle `start..end` in `machine`, im Tick `at`.
    Check {
        /// Die Art der Pruefung.
        kind: String,
        /// Die Maschine.
        machine: String,
        /// Anfang der Stelle in der Quelle.
        start: u32,
        /// Ihr Ende.
        end: u32,
        /// Tick des Faults.
        at: u64,
    },
    /// Die Verletzung der Eigenschaft `name` an der Position `at`.
    Property {
        /// Name.
        name: String,
        /// Position der Verletzung.
        at: u64,
    },
}

/// Ein Pfad des Solvers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    /// Sein Name, zugleich der Name seiner Datei.
    pub label: String,
    /// Sein Ziel.
    pub target: Target,
    /// Der Stimulus.
    pub stimulus: String,
    /// So viele Ticks laeuft er: zwei ueber die Tiefe des Solvers.
    pub ticks: u64,
}

/// Was der Solver an einem Programm fand: die Beweisdatei und die Pfade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Der Text der Beweisdatei; ohne bewiesene Stelle keine.
    pub proof: Option<String>,
    /// Die Pfade.
    pub paths: Vec<Path>,
}

/// Das Verzeichnis der Pfade. Ohne `crate::board`, das nur mit dem Feature
/// `board` gebaut wird.
pub fn dir() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/paths")
}

/// Der Stamm eines Korpusprogramms in `corpus-try/paths`.
fn stem(name: &str) -> String {
    name.trim_end_matches(".takt").replace(['/', '\\'], "_")
}

/// Die Beweisdatei des Programms `name`.
pub fn proof_file(name: &str) -> PathBuf {
    dir().join(format!("{}.takt-proof", stem(name)))
}

/// Das Verzeichnis der Pfade des Programms `name`.
pub fn paths_dir(name: &str) -> PathBuf {
    dir().join(stem(name))
}

/// Die Beweisdatei des Programms, wenn es eine gibt.
pub fn proof(name: &str) -> Option<takt_mir::analysis::proof::Proof> {
    let path = proof_file(name);
    let text = std::fs::read_to_string(&path).ok()?;
    let parsed = takt_mir::analysis::proof::parse(&text);
    Some(parsed.unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// Die gespeicherten Pfade des Programms `name`, nach ihrem Namen.
pub fn load(name: &str) -> Vec<Path> {
    let Ok(entries) = std::fs::read_dir(paths_dir(name)) else { return Vec::new() };
    let mut out: Vec<Path> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "trace"))
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let label = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            parse(&label, &text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

/// Liest eine Pfaddatei: zwei Kopfzeilen `# ziel …` und `# ticks …`, dann
/// der Stimulus.
fn parse(label: &str, text: &str) -> Result<Path, String> {
    let line = |key: &str| {
        text.lines().find_map(|l| l.strip_prefix(&format!("# {key} "))).ok_or_else(|| format!("`# {key}` fehlt"))
    };
    let words: Vec<&str> = line("ziel")?.split_whitespace().collect();
    let number = |w: &str| w.parse::<u64>().map_err(|_| format!("Zahl erwartet, `{w}` gefunden"));
    let target = match words.as_slice() {
        ["check", kind, machine, start, end, at] => Target::Check {
            kind: kind.to_string(),
            machine: machine.to_string(),
            start: u32::try_from(number(start)?).map_err(|_| "Anfang zu gross")?,
            end: u32::try_from(number(end)?).map_err(|_| "Ende zu gross")?,
            at: number(at)?,
        },
        ["property", name, at] => Target::Property { name: name.to_string(), at: number(at)? },
        _ => return Err(format!("`# ziel {}` nicht lesbar", words.join(" "))),
    };
    let ticks = number(line("ticks")?.trim())?;
    let stimulus = text.lines().filter(|l| !l.starts_with('#')).map(|l| format!("{l}\n")).collect();
    Ok(Path { label: label.to_string(), target, stimulus, ticks })
}

/// Die Datei eines Pfads; `solver` nennt Werkzeug und Version.
pub fn render_path(path: &Path, solver: &str) -> String {
    let goal = match &path.target {
        Target::Check { kind, machine, start, end, at } => format!("check {kind} {machine} {start} {end} {at}"),
        Target::Property { name, at } => format!("property {name} {at}"),
    };
    format!(
        "# Pfad des Solvers ({solver}, Tiefe {DEPTH}), erzeugt mit `UPDATE_PATHS` (M11 Schritt 28c)\n# ziel {goal}\n# ticks {}\n{}",
        path.ticks, path.stimulus
    )
}

/// Die Beweisdatei zu den bewiesen unerreichbaren Stellen `sites`, an die
/// Quelle gebunden; ohne Stelle keine.
pub fn proof_text(source: &str, solver: &str, sites: &[Site]) -> Option<String> {
    (!sites.is_empty()).then(|| render(&takt_mir::review::hash_of(source.as_bytes()), solver, sites))
}

/// Schreibt, was der Solver fand, an die Stelle des Programms `name`; was
/// dort vorher lag, geht.
pub fn store(name: &str, found: &Found, solver: &str) -> std::io::Result<()> {
    let dir = paths_dir(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    let file = proof_file(name);
    match &found.proof {
        Some(text) => {
            std::fs::create_dir_all(self::dir())?;
            std::fs::write(&file, text)?;
        }
        None if file.exists() => std::fs::remove_file(&file)?,
        None => {}
    }
    if !found.paths.is_empty() {
        std::fs::create_dir_all(&dir)?;
    }
    for p in &found.paths {
        std::fs::write(dir.join(format!("{}.trace", p.label)), render_path(p, solver))?;
    }
    Ok(())
}
