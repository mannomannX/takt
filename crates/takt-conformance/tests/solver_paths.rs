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
    // Eine unveraenderte Anfrage beantwortet der Cache; neu gefragt wird nur,
    // was sich geaendert hat (M11 B2 Schritt 0).
    let solver = solver.cached(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("solver-cache"));
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

/// Die Frist je Anfrage beim Einordnen (FB-511 Schritt 1).
const CLASSIFY_TIMEOUT_S: u64 = 60;

/// Die Programme, deren Stellen der Solver nicht am Teilmodell fragen kann
/// (FB-604): Jede Anfrage ginge ans Gesamtmodell, das er nicht entscheidet.
const WITHOUT_PARTS: &[&str] = &["45_journal_cut.takt", "110_journal_log.takt"];

/// Die Stellen eines Programms, an denen kein Lauf einen Fault sah und die
/// seine Beweisdatei nicht fuehrt (Spalte drei von `UNREACHED`): explizite
/// Pruefungen als (Maschine, Art, Anfang), implizite als (Anfang, Ende, Art).
type Open = (Vec<(String, String, u32)>, Vec<(u32, u32, String)>);

fn never_failed(name: &str, p: &Program) -> Open {
    let mut coverage = Coverage::default();
    for case in takt_conformance::cases::cases(name, p) {
        let Ok(stimulus) = takt_interp::Trace::parse(&case.stimulus) else { continue };
        let options = takt_interp::RunOptions { ticks: case.ticks, inputs: true, ..Default::default() };
        if let Ok(r) = takt_interp::run(p, &stimulus, &options) {
            coverage.merge(&r.coverage);
        }
    }
    let proven = paths::proof(name).map(|pr| pr.tags()).unwrap_or_default();
    let mut out: Open = Default::default();
    for item in takt_interp::coverage::items(p) {
        let kind = item.key.split(' ').next().unwrap_or("").to_string();
        match item.kind {
            CoverKind::Check if kind != "verify" && !coverage.has(CoverKind::CheckFailed, &item) => {
                let start = item.key.rsplit_once('@').and_then(|(_, s)| s.parse().ok()).expect("check @<anfang>");
                out.0.push((item.machine.clone(), kind, start));
            }
            CoverKind::Site if item.silent.is_none() && !coverage.has(CoverKind::SiteFailed, &item) => {
                let tag = takt_mir::analysis::walk::tag_of_name(&kind);
                if !tag.is_some_and(|t| proven.contains(&(item.span.start, item.span.end, t))) {
                    out.1.push((item.span.start, item.span.end, kind));
                }
            }
            _ => {}
        }
    }
    out
}

/// Die Gruppe eines Urteils (FB-511 Schritt 1): erreichbar (auch tiefer,
/// als Spacer zeigt), bewiesen, nicht k-induktiv, ausserhalb des Fragments
/// der Invariantensuche, Frist, oder ein Pfad, den der Interpreter nicht
/// bestaetigt.
fn group(verdict: &CheckVerdict) -> &'static str {
    match verdict {
        CheckVerdict::Reachable { .. } => "erreichbar",
        CheckVerdict::Unreachable { .. } => "bewiesen",
        CheckVerdict::Undecided { reason } if reason.starts_with("ohne Urteil") => "ohne Urteil",
        CheckVerdict::Undecided { reason } if reason.contains("findet einen Pfad") => "erreichbar, tiefer",
        CheckVerdict::Undecided { reason } if reason.contains("ausserhalb ihres Fragments") => "Fragment",
        CheckVerdict::Undecided { reason } if reason.contains("Induktionsschritt offen") => "nicht k-induktiv",
        CheckVerdict::Undecided { reason } if reason.contains("Solver sagt") => "Frist",
        CheckVerdict::Undecided { .. } => "unbestaetigt",
    }
}

/// Eine Zeile des Protokolls (`fb511-<tiefe>.tsv`): Programm, Maschine, Art,
/// Anfang der Anweisung, Spanne, Gruppe, Urteil.
#[derive(Clone)]
struct Row {
    name: String,
    machine: String,
    kind: String,
    start: u32,
    span: (u32, u32),
    group: String,
    why: String,
}

impl Row {
    fn of(name: &str, r: &CheckReport) -> Row {
        let why = r.text().lines().next().unwrap_or_default().to_string();
        Row {
            name: name.to_string(),
            machine: r.machine.clone(),
            kind: r.kind.clone(),
            start: r.start,
            span: (r.span.start, r.span.end),
            group: group(&r.verdict).to_string(),
            why,
        }
    }

    fn line(&self) -> String {
        let Row { name, machine, kind, start, span: (a, b), group, why } = self;
        format!("{name}\t{machine}\t{kind}\t{start}\t{a}-{b}\t{group}\t{why}\n")
    }

    fn parse(line: &str) -> Option<Row> {
        let mut f = line.splitn(7, '\t');
        let (name, machine, kind) = (f.next()?, f.next()?, f.next()?);
        let start = f.next()?.parse().ok()?;
        let (a, b) = f.next()?.split_once('-')?;
        let (group, why) = (f.next()?, f.next()?);
        let span = (a.parse().ok()?, b.parse().ok()?);
        Some(Row {
            name: name.into(),
            machine: machine.into(),
            kind: kind.into(),
            start,
            span,
            group: group.into(),
            why: why.into(),
        })
    }

    /// Entschieden: erreichbar mit bestaetigtem Pfad oder bewiesen.
    fn decided(&self) -> bool {
        self.group == "erreichbar" || self.group == "bewiesen"
    }
}

/// **FB-511 Schritt 1: die unerreichten Stellen einordnen.** Je Tiefe aus
/// `depths` (`10,20`) fragt der Solver mit [`CLASSIFY_TIMEOUT_S`] je Anfrage
/// die Stellen ohne Fault, die die vorige Tiefe nicht entschied; das
/// Ergebnis steht je Stelle in `CARGO_TARGET_TMPDIR/fb511-<tiefe>.tsv` und
/// gezaehlt im Protokoll. Eine entschiedene Antwort nimmt der Cache. Im
/// Release-Bau, wie `UPDATE_PATHS`.
fn classify_unreached(depths: &str) {
    let found = Some(takt_prove::find()).filter(|s| !matches!(s, Solver::Missing));
    let Some(solver) = takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren") else {
        return;
    };
    let solver = solver.cached(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("solver-cache"));
    let mut names = takt_conformance::suites::programs("vergleich");
    names.extend(takt_conformance::suites::EXAMPLES);
    names.retain(|n| !WITHOUT_PARTS.contains(n));
    // `CLASSIFY_PROGRAMS=<datei>,<datei>` beschraenkt den Lauf, etwa zur Probe.
    if let Ok(only) = std::env::var("CLASSIFY_PROGRAMS") {
        names.retain(|n| only.split(',').any(|o| o.trim() == *n));
    }
    let mut open: BTreeMap<&str, Open> = BTreeMap::new();
    for name in &names {
        let src = source(name);
        let p = compile(name, &src, paths::proof(name).as_ref());
        let sites = never_failed(name, &p);
        if !sites.0.is_empty() || !sites.1.is_empty() {
            open.insert(name, sites);
        }
    }
    let tmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    for depth in depths.split(',').map(|d| d.trim().parse::<u32>().expect("Tiefe")) {
        let started = std::time::Instant::now();
        // `CLASSIFY_RESUME=1` nimmt, was ein abgebrochener Lauf dieser Tiefe
        // schon eingeordnet hat; sonst beginnt die Tiefe neu.
        let progress = tmp.join(format!("fb511-{depth}.laufend.tsv"));
        let resumed: Vec<Row> = match std::env::var("CLASSIFY_RESUME") {
            Ok(_) => std::fs::read_to_string(&progress).unwrap_or_default().lines().filter_map(Row::parse).collect(),
            Err(_) => {
                let _ = std::fs::remove_file(&progress);
                Vec::new()
            }
        };
        let done: Vec<&str> = resumed.iter().map(|r| r.name.as_str()).collect();
        let queue = std::sync::Mutex::new(open.keys().copied().filter(|n| !done.contains(n)).collect::<Vec<_>>());
        let rows = std::sync::Mutex::new(resumed.clone());
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        std::thread::scope(|scope| {
            for _ in 0..((cores - 1) / 2).max(1) {
                let worker = || {
                    loop {
                        let next = queue.lock().expect("Warteschlange").pop();
                        let Some(name) = next else { break };
                        let (checks, sites) = &open[name];
                        let src = source(name);
                        let p = compile(name, &src, paths::proof(name).as_ref());
                        let whole = takt_prove::encode(&p).ok();
                        let keep = |c: &takt_prove::CheckSite| {
                            checks.iter().any(|(m, k, s)| *m == c.machine && *k == c.kind && *s == c.start)
                                || sites.iter().any(|(a, b, k)| *a == c.span.start && *b == c.span.end && *k == c.kind)
                        };
                        match takt_prove::classify_sites(&p, whole.as_ref(), depth, &solver, CLASSIFY_TIMEOUT_S, &keep)
                        {
                            Ok((reports, notes)) => {
                                // Eine Stelle ohne Urteil: Kein Modell fuehrt sie.
                                let none = |kind: &str, machine: &str, start: u32, end: u32| CheckReport {
                                    start,
                                    span: takt_diag::Span::new(start, end),
                                    machine: machine.to_string(),
                                    kind: kind.to_string(),
                                    verdict: CheckVerdict::Undecided {
                                        reason: format!("ohne Urteil; {}", notes.join("; ")),
                                    },
                                };
                                let mut missing: Vec<CheckReport> = checks
                                    .iter()
                                    .filter(|(m, k, s)| {
                                        !reports.iter().any(|r| r.machine == *m && r.kind == *k && r.start == *s)
                                    })
                                    .map(|(m, k, s)| none(k, m, *s, *s))
                                    .collect();
                                missing.extend(
                                    sites
                                        .iter()
                                        .filter(|(a, b, k)| {
                                            !reports
                                                .iter()
                                                .any(|r| r.span.start == *a && r.span.end == *b && r.kind == *k)
                                        })
                                        .map(|(a, b, k)| none(k, "", *a, *b)),
                                );
                                // Je Programm gleich ins Protokoll: Ein abgebrochener Lauf
                                // verliert so nur das laufende.
                                let mine: Vec<Row> = reports.iter().chain(&missing).map(|r| Row::of(name, r)).collect();
                                let lines: String = mine.iter().map(Row::line).collect();
                                let mut rows = rows.lock().expect("Zeilen");
                                let appended = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&progress)
                                    .and_then(|mut f| std::io::Write::write_all(&mut f, lines.as_bytes()));
                                appended.unwrap_or_else(|e| panic!("{}: {e}", progress.display()));
                                eprintln!("{name}: {} Stellen nach {:.0?}", mine.len(), started.elapsed());
                                rows.extend(mine);
                            }
                            Err(e) => eprintln!("{name}: {e}"),
                        }
                    }
                };
                std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
            }
        });
        let mut rows = rows.into_inner().expect("Zeilen");
        rows.sort_by(|a, b| (&a.name, a.start).cmp(&(&b.name, b.start)));
        let mut text = String::new();
        let mut tally: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for r in &rows {
            *tally.entry((r.group.as_str(), r.kind.as_str())).or_default() += 1;
            text.push_str(&r.line());
        }
        let file = tmp.join(format!("fb511-{depth}.tsv"));
        std::fs::write(&file, text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        eprintln!("Tiefe {depth}: {} Stellen in {:.0?}, {}", rows.len(), started.elapsed(), file.display());
        let mut by_group: BTreeMap<&str, usize> = BTreeMap::new();
        for ((g, kind), n) in &tally {
            *by_group.entry(g).or_default() += n;
            eprintln!("  {g:<20} {kind:<10} {n}");
        }
        eprintln!("  {by_group:?}");
        // Tiefer gefragt wird nur, was diese Tiefe offen liess.
        for (name, (checks, sites)) in open.iter_mut() {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.name == *name && r.decided()).collect();
            checks.retain(|(m, k, s)| !mine.iter().any(|r| r.machine == *m && r.kind == *k && r.start == *s));
            sites.retain(|(a, b, k)| !mine.iter().any(|r| r.span == (*a, *b) && r.kind == *k));
        }
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
    if let Ok(depths) = std::env::var("CLASSIFY_UNREACHED") {
        classify_unreached(&depths);
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
