//! Die Pfade des Solvers je Korpusprogramm (M11 Schritt 28c,
//! `takt_conformance::paths`): Jede Beweisdatei passt zu ihrer Quelle, und
//! jeder gespeicherte Pfad erreicht sein Ziel im Interpreter noch — mit der
//! Beweisdatei uebersetzt, so dass eine bewiesene Pruefung, die doch
//! feuert, den Interpreter abbricht. Den Vergleich der drei Ausfuehrer
//! ueber die Pfade rechnet `the_three_executors_agree`.
//!
//! `UPDATE_PATHS=1` fragt den Solver neu (wie `takt prove`, Tiefe und Frist
//! aus `takt_conformance::paths`) und schreibt Pfade und Beweisdateien;
//! `UPDATE_PATHS=<datei>,<datei>` nur fuer diese Programme. Im Release-Bau
//! (`cargo nextest run --release`): Die Modelle der grossen Programme baut
//! ein Debug-Bau um ein Vielfaches langsamer.

use takt_conformance::paths::{self, DEPTH, Found, Path, TIMEOUT_S, Target};
use takt_mir::Program;
use takt_mir::analysis::proof::{Proof, Site};
use takt_prove::{CheckVerdict, Solver, Verdict};

fn source(name: &str) -> String {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn compile(name: &str, src: &str, proof: Option<&Proof>) -> Program {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile_with(src, &options, proof);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Die Programme mit Pfaden: die Suite `beweiser`.
fn programs() -> Vec<&'static str> {
    takt_conformance::suites::programs("beweiser")
}

/// Fragt den Solver wie `takt prove`: jede Eigenschaft, jede Pruefstelle je
/// Maschine (13.3). Ein `requires` bekommt keinen Pfad: Vertraege prueft der
/// Interpreter nicht (5.7), sein Pfad stammt allein aus dem Modell.
fn generate(src: &str, p: &Program, solver: &Solver) -> Result<Found, String> {
    let ticks = u64::from(DEPTH) + 2;
    let whole = takt_prove::encode(p);
    let mut found = Vec::new();
    if let Ok(model) = &whole {
        for r in takt_prove::prove(model, p, DEPTH, solver, TIMEOUT_S)? {
            if let Verdict::Violated { at, stimulus } = r.verdict {
                let label = format!("property-{}", r.name);
                found.push(Path { label, target: Target::Property { name: r.name, at }, stimulus, ticks });
            }
        }
    }
    let (checks, _) = takt_prove::classify_compositional(p, whole.as_ref().ok(), DEPTH, solver, TIMEOUT_S)?;
    let mut sites = Vec::new();
    for c in checks {
        match c.verdict {
            CheckVerdict::Reachable { at, stimulus } if c.kind != "requires" => {
                let label = format!("{}-{}", c.kind.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_"), c.start);
                let (start, end) = (c.span.start, c.span.end);
                let target = Target::Check { kind: c.kind.clone(), machine: c.machine.clone(), start, end, at };
                found.push(Path { label, target, stimulus, ticks });
            }
            CheckVerdict::Unreachable { k } if takt_mir::analysis::walk::tag_of_name(&c.kind).is_some() => {
                sites.push(Site { start: c.start, end: c.span.end, kind: c.kind, k });
            }
            _ => {}
        }
    }
    let identity = solver.identity().unwrap_or_default();
    Ok(Found { proof: paths::proof_text(src, &identity, &sites), paths: found })
}

/// Erreicht der Pfad im Interpreter noch sein Ziel? Eine Pruefstelle nach
/// derselben Regel, mit der `takt prove` den Pfad bestaetigte; `Err` nennt,
/// was stattdessen geschah.
fn reaches(p: &Program, path: &Path) -> Result<(), String> {
    let stimulus = takt_interp::Trace::parse(&path.stimulus).map_err(|e| format!("Stimulus: {e:?}"))?;
    let options = takt_interp::RunOptions { ticks: path.ticks, ..Default::default() };
    let r = takt_interp::run(p, &stimulus, &options).map_err(|e| format!("Lauf: {e:?}"))?;
    let hit = match &path.target {
        Target::Check { kind, machine, start, end, .. } => {
            takt_prove::fault_at(&r, machine, kind, takt_diag::Span::new(*start, *end)).is_some()
        }
        Target::Property { name, .. } => r
            .trace
            .lines
            .iter()
            .any(|l| matches!(&l.kind, takt_interp::trace::LineKind::Property { name: n, .. } if n == name)),
    };
    if hit { Ok(()) } else { Err(format!("`{}` erreicht sein Ziel nicht mehr", path.label)) }
}

/// Erzeugt Pfade und Beweisdateien neu: `1` fuer alle Programme, sonst die
/// genannten.
fn update(which: &str) {
    let found = Some(takt_prove::find()).filter(|s| !matches!(s, Solver::Missing));
    let Some(solver) = takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren") else {
        return;
    };
    let identity = solver.identity().unwrap_or_default();
    // Ein grosses Programm braucht den Solver eine Viertelstunde; die
    // Programme verteilen sich darum auf Faeden, je einer neben seinem
    // Solver-Prozess, ein Kern bleibt frei.
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let chosen: Vec<&str> = which.split(',').map(str::trim).collect();
    let all = programs();
    if let Some(name) = chosen.iter().find(|n| **n != "1" && !all.contains(n)) {
        panic!("`{name}` steht nicht in der Suite `beweiser`");
    }
    let picked: Vec<&str> = all.into_iter().filter(|n| which == "1" || chosen.contains(n)).collect();
    let queue = std::sync::Mutex::new(picked);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..((cores - 1) / 2).max(1) {
            let worker = || {
                loop {
                    // Die Sperre gilt nur fuer das Holen, nicht fuer die Arbeit.
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    let started = std::time::Instant::now();
                    let src = source(name);
                    let p = compile(name, &src, None);
                    // Scheitert der Solver (Speicher, Frist), bleibt der alte
                    // Stand des Programms stehen; der Test prueft ihn wie jeden.
                    match generate(&src, &p, &solver) {
                        Ok(found) => {
                            paths::store(name, &found, &identity).unwrap_or_else(|e| panic!("{name}: {e}"));
                            eprintln!("{name}: {} Pfade in {:.0?}", found.paths.len(), started.elapsed());
                        }
                        Err(e) => failed.lock().expect("Fehlschlaege").push(format!("{name}: {e}")),
                    }
                }
            };
            // Kodierung und Sema gehen tief: 64 MiB Stapel je Faden.
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
        }
    });
    for f in failed.into_inner().expect("Fehlschlaege") {
        eprintln!("ohne neue Pfade, der Solver scheiterte: {f}");
    }
}

/// **Die Pfade und Beweise des Solvers passen zum Korpus.** Jede
/// Beweisdatei ist an ihre Quelle gebunden, jeder Pfad erreicht sein Ziel,
/// und nichts unter `corpus-try/paths` gehoert zu einem Programm ausserhalb
/// der Suite `beweiser`.
#[test]
fn the_stored_solver_paths_still_hold() {
    if let Ok(which) = std::env::var("UPDATE_PATHS") {
        update(&which);
    }
    let hint = "`UPDATE_PATHS=<datei> cargo nextest run --release -p takt-conformance --test solver_paths` erzeugt neu";
    let mut failed = Vec::new();
    let mut known = Vec::new();
    for name in programs() {
        let src = source(name);
        let proof = paths::proof(name);
        if let Some(proof) = &proof
            && proof.program != takt_mir::review::hash_of(src.as_bytes())
        {
            failed.push(format!("{name}: die Beweisdatei gehoert zu einer anderen Quelle; {hint}"));
            continue;
        }
        let p = compile(name, &src, proof.as_ref());
        for path in paths::load(name) {
            if let Err(e) = reaches(&p, &path) {
                failed.push(format!("{name}: {e}; {hint}"));
            }
        }
        for f in [paths::proof_file(name), paths::paths_dir(name)] {
            known.push(f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
    for entry in std::fs::read_dir(paths::dir()).into_iter().flatten().flatten() {
        let n = entry.file_name().to_string_lossy().into_owned();
        if !known.contains(&n) {
            failed.push(format!("`corpus-try/paths/{n}` gehoert zu keinem Programm der Suite `beweiser`"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
