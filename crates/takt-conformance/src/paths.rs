//! Die Pfade des Solvers je Korpusprogramm (M11 Schritte 28c, 29a): Was
//! `takt prove` an einem Programm findet — Gegenbeispiele zu Eigenschaften,
//! Pfade zu erreichbaren Pruefstellen und Uebergaengen, bewiesen
//! unerreichbare Stellen und Uebergaenge —, steht unter
//! `corpus-try/paths/`. Die Pfade sind Stimuli, die der Vergleich wie jeden
//! anderen Lauf durch alle drei Ausfuehrer rechnet; die Beweise stehen in
//! einer Beweisdatei (11.3), mit der er das Programm uebersetzt. Der Codegen
//! laesst die bewiesenen Pruefungen aus, der Interpreter prueft sie weiter:
//! Ein falscher Beweis faellt dort auf, und der Vergleich sieht den
//! Unterschied. Bewiesen nie genommene Uebergaenge stehen daneben in
//! `<programm>.unfired`; der Codegen liest sie nicht, die Ratsche der
//! Abdeckung schon.
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
    /// Der Uebergang `key` (Schluessel der Coverage) in `machine`, genommen
    /// im Tick `at`.
    Transition {
        /// Die Maschine.
        machine: String,
        /// `VON->NACH @<anfang>`.
        key: String,
        /// Tick, in dem er genommen wird.
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

/// Was der Solver an einem Programm fand: die Beweisdatei, die bewiesen
/// unerreichbaren Uebergaenge und die Pfade.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// Der Text der Beweisdatei; ohne bewiesene Stelle keine.
    pub proof: Option<String>,
    /// Der Text von `<programm>.unfired`; ohne bewiesenen Uebergang keiner.
    pub unfired: Option<String>,
    /// Die Pfade.
    pub paths: Vec<Path>,
}

/// Die bewiesen nie genommenen Uebergaenge eines Programms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unfired {
    /// SHA-256 der Quelle, hexadezimal.
    pub program: String,
    /// Der Solver mit Version.
    pub solver: String,
    /// Maschine und Schluessel der Coverage je Uebergang.
    pub transitions: Vec<(String, String)>,
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

/// Die Datei der bewiesen nie genommenen Uebergaenge des Programms `name`.
pub fn unfired_file(name: &str) -> PathBuf {
    dir().join(format!("{}.unfired", stem(name)))
}

/// Die bewiesen nie genommenen Uebergaenge des Programms, wenn es sie gibt.
pub fn unfired(name: &str) -> Option<Unfired> {
    let path = unfired_file(name);
    let text = std::fs::read_to_string(&path).ok()?;
    Some(parse_unfired(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
}

/// Liest `<programm>.unfired`: `takt-unfired 1`, `program <hash>`, `solver
/// <Name> <Version>`, je Uebergang `uebergang <maschine> k=<tiefe>
/// <schluessel>`.
fn parse_unfired(text: &str) -> Result<Unfired, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    if lines.next() != Some("takt-unfired 1") {
        return Err("erste Zeile muss `takt-unfired 1` sein".into());
    }
    let mut out = Unfired::default();
    for line in lines {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "program" => out.program = rest.to_string(),
            "solver" => out.solver = rest.to_string(),
            "uebergang" => {
                let mut parts = rest.splitn(3, ' ');
                let (Some(machine), Some(_k), Some(key)) = (parts.next(), parts.next(), parts.next()) else {
                    return Err(format!("`{line}` unverstanden"));
                };
                out.transitions.push((machine.to_string(), key.to_string()));
            }
            _ => return Err(format!("`{line}` unverstanden")),
        }
    }
    if out.program.is_empty() || out.solver.is_empty() {
        return Err("`program` und `solver` fehlen".into());
    }
    Ok(out)
}

/// `<programm>.unfired` zu den bewiesen nie genommenen Uebergaengen,
/// `(maschine, schluessel, k)`, an die Quelle gebunden; ohne Uebergang keine.
pub fn unfired_text(source: &str, solver: &str, transitions: &[(String, String, u32)]) -> Option<String> {
    if transitions.is_empty() {
        return None;
    }
    let mut out =
        format!("takt-unfired 1\nprogram {}\nsolver {solver}\n", takt_mir::review::hash_of(source.as_bytes()));
    for (machine, key, k) in transitions {
        out.push_str(&format!("uebergang {machine} k={k} {key}\n"));
    }
    Some(out)
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
    let goal = line("ziel")?;
    let words: Vec<&str> = goal.split_whitespace().collect();
    let number = |w: &str| w.parse::<u64>().map_err(|_| format!("Zahl erwartet, `{w}` gefunden"));
    let target = match words.as_slice() {
        // Der Schluessel enthaelt Leerzeichen (`A->[Fault Range] @12`): der Rest der Zeile.
        ["uebergang", machine, at, ..] => {
            let key = goal.splitn(4, ' ').nth(3).unwrap_or_default();
            Target::Transition { machine: machine.to_string(), key: key.to_string(), at: number(at)? }
        }
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
        Target::Transition { machine, key, at } => format!("uebergang {machine} {at} {key}"),
    };
    format!(
        "# Pfad des Solvers ({solver}, Tiefe {DEPTH}), erzeugt mit `UPDATE_PATHS` (M11 Schritte 28c, 29a)\n# ziel {goal}\n# ticks {}\n{}",
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
    for (file, text) in [(proof_file(name), &found.proof), (unfired_file(name), &found.unfired)] {
        match text {
            Some(text) => {
                std::fs::create_dir_all(self::dir())?;
                std::fs::write(&file, text)?;
            }
            None if file.exists() => std::fs::remove_file(&file)?,
            None => {}
        }
    }
    if !found.paths.is_empty() {
        std::fs::create_dir_all(&dir)?;
    }
    for p in &found.paths {
        std::fs::write(dir.join(format!("{}.trace", p.label)), render_path(p, solver))?;
    }
    Ok(())
}
