//! Die Pfade des Solvers je Korpusprogramm (M11 Schritte 28c, 29a,
//! `takt_conformance::paths`): Jede Beweisdatei und jede Liste bewiesen nie
//! genommener Uebergaenge passt zu ihrer Quelle, und jeder gespeicherte Pfad
//! erreicht sein Ziel im Interpreter noch — mit der Beweisdatei uebersetzt,
//! so dass eine bewiesene Pruefung, die doch feuert, den Interpreter
//! abbricht. Den Vergleich der drei Ausfuehrer ueber die Pfade rechnet
//! `the_three_executors_agree`.
//!
//! Gesucht wird, was die Laeufe ohne Pfad nicht erreichen (`still`, der
//! geschriebene Stimulus, die erzeugten Eingaben): je Eigenschaft eine
//! Verletzung, je Pruefstelle ihr Fault, je Uebergang sein Nehmen.
//!
//! `UPDATE_PATHS=1` fragt den Solver neu (wie `takt prove`, Tiefe und Frist
//! aus `takt_conformance::paths`) und schreibt Pfade und Beweisdateien;
//! `UPDATE_PATHS=<datei>,<datei>` nur fuer diese Programme. Im Release-Bau
//! (`cargo nextest run --release`): Die Modelle der grossen Programme baut
//! ein Debug-Bau um ein Vielfaches langsamer.

use std::collections::BTreeMap;

use takt_conformance::paths::{self, DEPTH, Found, Path, TIMEOUT_S, Target};
use takt_interp::{CoverKind, Coverage};
use takt_mir::Program;
use takt_mir::analysis::proof::{Proof, Site};
use takt_prove::{CheckReport, CheckVerdict, Solver, Verdict};

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

/// Was die Laeufe ohne Pfad erreichen: die Coverage von `still`, dem
/// geschriebenen Stimulus und den erzeugten Eingaben.
fn reached(name: &str, p: &Program) -> Coverage {
    let mut all = Coverage::default();
    for case in takt_conformance::cases::cases(name, p).into_iter().filter(|c| !c.label.starts_with("pfad")) {
        let Ok(stimulus) = takt_interp::Trace::parse(&case.stimulus) else { continue };
        let options = takt_interp::RunOptions { ticks: case.ticks, ..Default::default() };
        if let Ok(r) = takt_interp::run(p, &stimulus, &options) {
            all.merge(&r.coverage);
        }
    }
    all
}

/// Faellt schon ein Lauf ohne Pfad an der Pruefstelle des Berichts?
fn failed_without_path(reached: &Coverage, c: &CheckReport) -> bool {
    let hit = |kind, machine: &str, key: String| reached.hits.contains_key(&(kind, machine.to_string(), key));
    match c.kind.as_str() {
        "check" | "expect" => hit(CoverKind::CheckFailed, &c.machine, format!("{} @{}", c.kind, c.start)),
        kind => hit(CoverKind::SiteFailed, "", format!("{kind} @{}-{}", c.span.start, c.span.end)),
    }
}

/// Ein Name fuer eine Pfaddatei: was nicht Buchstabe oder Ziffer ist, wird `_`.
fn label(head: &str, tail: &str) -> String {
    format!("{head}-{}", tail.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_"))
}

/// Fragt den Solver wie `takt prove` nach dem, was die Laeufe ohne Pfad nicht
/// erreichen: je Eigenschaft eine Verletzung, je Pruefstelle (13.3, je
/// Maschine) ihr Fault, je Uebergang sein Nehmen. Ein `requires` bekommt
/// keinen Pfad: Vertraege prueft der Interpreter nicht (5.7), sein Pfad
/// stammt allein aus dem Modell.
///
/// Bewiesen ist eine Stelle nur, wenn jedes Urteil ueber sie `unerreichbar`
/// sagt: Eine Funktion, die zwei Maschinen rufen, hat ein Urteil je
/// Maschine, und der Codegen liesse die Pruefung fuer beide aus (FB-496).
fn generate(name: &str, src: &str, p: &Program, solver: &Solver) -> Result<Found, String> {
    let ticks = u64::from(DEPTH) + 2;
    let reached = reached(name, p);
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
    let mut by_site: BTreeMap<(u32, u32, String), Vec<CheckReport>> = BTreeMap::new();
    for c in checks.into_iter().filter(|c| c.kind != "requires") {
        by_site.entry((c.span.start, c.span.end, c.kind.clone())).or_default().push(c);
    }
    let mut sites = Vec::new();
    for ((start, end, kind), reports) in by_site {
        let proven: Option<Vec<u32>> = reports
            .iter()
            .map(|c| match c.verdict {
                CheckVerdict::Unreachable { k } => Some(k),
                _ => None,
            })
            .collect();
        match proven {
            Some(ks) if takt_mir::analysis::walk::tag_of_name(&kind).is_some() => {
                sites.push(Site { start, end, kind, k: ks.into_iter().max().unwrap_or(0) });
            }
            Some(_) => {}
            None => {
                let reachable = reports.iter().find_map(|c| match &c.verdict {
                    CheckVerdict::Reachable { at, stimulus } => Some((c, *at, stimulus.clone())),
                    _ => None,
                });
                if let Some((c, at, stimulus)) = reachable.filter(|(c, _, _)| !failed_without_path(&reached, c)) {
                    let target = Target::Check { kind: kind.clone(), machine: c.machine.clone(), start, end, at };
                    found.push(Path { label: label(&kind, &start.to_string()), target, stimulus, ticks });
                }
            }
        }
    }
    let mut unfired = Vec::new();
    if let Ok(model) = &whole {
        let open: Vec<(String, String)> = model
            .transitions
            .iter()
            .filter(|t| !reached.hits.contains_key(&(CoverKind::Transition, t.machine.clone(), t.kind.clone())))
            .map(|t| (t.machine.clone(), t.kind.clone()))
            .collect();
        for r in takt_prove::classify_transitions(model, p, &open, DEPTH, solver, TIMEOUT_S)? {
            match r.verdict {
                CheckVerdict::Reachable { at, stimulus } => {
                    let label = label("uebergang", &r.key);
                    let target = Target::Transition { machine: r.machine, key: r.key, at };
                    found.push(Path { label, target, stimulus, ticks });
                }
                CheckVerdict::Unreachable { k } => unfired.push((r.machine, r.key, k)),
                CheckVerdict::Undecided { .. } => {}
            }
        }
    }
    let identity = solver.identity().unwrap_or_default();
    Ok(Found {
        proof: paths::proof_text(src, &identity, &sites),
        unfired: paths::unfired_text(src, &identity, &unfired),
        paths: found,
    })
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
        Target::Transition { machine, key, .. } => {
            r.coverage.hits.contains_key(&(CoverKind::Transition, machine.clone(), key.clone()))
        }
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
    let one = |name: &str| -> Result<(), String> {
        let started = std::time::Instant::now();
        let src = source(name);
        let p = compile(name, &src, None);
        let found = generate(name, &src, &p, &solver)?;
        paths::store(name, &found, &identity).unwrap_or_else(|e| panic!("{name}: {e}"));
        eprintln!("{name}: {} Pfade in {:.0?}", found.paths.len(), started.elapsed());
        Ok(())
    };
    let queue = std::sync::Mutex::new(picked);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..((cores - 1) / 2).max(1) {
            let worker = || {
                loop {
                    // Die Sperre gilt nur fuer das Holen, nicht fuer die Arbeit.
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    if let Err(e) = one(name) {
                        failed.lock().expect("Fehlschlaege").push((name, e));
                    }
                }
            };
            // Kodierung und Sema gehen tief: 64 MiB Stapel je Faden.
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
        }
    });
    // Was neben den anderen am Speicher scheiterte, laeuft allein noch
    // einmal. Scheitert es auch dann, bliebe der alte Stand stehen, ohne
    // dass ihn das neue Modell bestaetigt hat: Das ist ein Fehlschlag.
    let mut lasting = Vec::new();
    for (name, first) in failed.into_inner().expect("Fehlschlaege") {
        eprintln!("{name}: der Solver scheiterte neben den anderen ({first}), allein noch einmal");
        let retry = std::thread::scope(|scope| {
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, || one(name)).expect("Faden").join()
        });
        match retry {
            Ok(Ok(())) => {}
            Ok(Err(e)) => lasting.push(format!("{name}: {e}")),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    assert!(lasting.is_empty(), "ohne neue Pfade, der Solver scheiterte:\n{}", lasting.join("\n"));
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
        let hash = takt_mir::review::hash_of(src.as_bytes());
        let proof = paths::proof(name);
        if let Some(proof) = &proof
            && proof.program != hash
        {
            failed.push(format!("{name}: die Beweisdatei gehoert zu einer anderen Quelle; {hint}"));
            continue;
        }
        if paths::unfired(name).is_some_and(|u| u.program != hash) {
            failed.push(format!("{name}: `.unfired` gehoert zu einer anderen Quelle; {hint}"));
            continue;
        }
        let p = compile(name, &src, proof.as_ref());
        for path in paths::load(name) {
            if let Err(e) = reaches(&p, &path) {
                failed.push(format!("{name}: {e}; {hint}"));
            }
        }
        for f in [paths::proof_file(name), paths::unfired_file(name), paths::paths_dir(name)] {
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
