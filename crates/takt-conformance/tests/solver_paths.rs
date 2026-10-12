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
//!
//! Die Pfade aus dem langen Lauf (`takt_conformance::paths::from_run`,
//! M11 Strang B2 Schritt 3) brauchen keinen Solver: Der Test rechnet sie je
//! Programm des Vergleichs neu und haelt sie gegen die gespeicherten;
//! `UPDATE_RUN_PATHS=1` (oder `=<datei>,<datei>`) schreibt sie.

use std::collections::BTreeMap;

use takt_conformance::paths::{self, DEPTH, Found, Path, TIMEOUT_S, Target, label};
use takt_interp::coverage::Silent;
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

/// Die Programme mit Pfaden: die Suite `beweiser` und die Beispiele, die
/// der Vergleich neben dem Korpus faehrt (FB-511 Schritt 4). Ihre Beweise
/// laesst der Codegen aus, der Interpreter prueft sie weiter.
fn programs() -> Vec<&'static str> {
    let mut out = takt_conformance::suites::programs("beweiser");
    out.extend(takt_conformance::suites::EXAMPLES);
    out
}

/// Was die Laeufe ohne Pfad des Solvers erreichen: die Coverage von `still`,
/// dem geschriebenen Stimulus, den erzeugten Eingaben und den Pfaden aus dem
/// langen Lauf.
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
                    // Einen Pfad jenseits der Tiefe fand die Vertiefung (`takt_prove::DEEPEST`).
                    let ticks = ticks.max(at + 2);
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
                    found.push(Path { label, target, stimulus, ticks: ticks.max(at + 2) });
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
    let found = Some(takt_prove::find()).filter(|s| !s.is_missing());
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
        panic!("`{name}` steht weder in der Suite `beweiser` noch unter den Beispielen");
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

/// Was in den Laeufen eines Programms offen bleibt (die Spalten eins bis
/// drei von `UNREACHED`): nie genommene Uebergaenge, die `.unfired` nicht
/// fuehrt, als (Maschine, Schluessel); nie bestandene Stellen als (Anfang,
/// Ende, Art); nie verletzte Stellen, die die Beweisdatei nicht fuehrt —
/// explizite als (Maschine, Art, Anfang), implizite als (Anfang, Ende, Art).
/// `without_model` nennt je Stelle (Anfang, Ende, Art) die Gruppe, die gilt,
/// wenn kein Modell sie fuehrt ([`WITHOUT_MODEL`]).
#[derive(Default)]
struct Open {
    transitions: Vec<(String, String)>,
    passes: Vec<(u32, u32, String)>,
    checks: Vec<(String, String, u32)>,
    sites: Vec<(u32, u32, String)>,
    without_model: BTreeMap<(u32, u32, String), &'static str>,
}

/// Die Gruppen einer Stelle, die kein Modell fuehrt, und warum keines sie
/// fuehrt: eine Funktion, die keine Maschine ruft (die Analyse nach 3.4
/// kennt sie nicht); eine bewiesene Range, die Modell und Codegen nicht
/// pruefen; der Platzhalter einer Meldung, die nur beim Verletzen entsteht;
/// Bedingung oder Wert einer Beobachtung (13.2).
const WITHOUT_MODEL: [&str; 4] = ["nie gerufen", "Range bewiesen", "in einer Meldung", "in einer Beobachtung"];

impl Open {
    fn is_empty(&self) -> bool {
        self.transitions.is_empty() && self.passes.is_empty() && self.checks.is_empty() && self.sites.is_empty()
    }
}

fn open_items(name: &str, p: &Program) -> Open {
    let mut coverage = Coverage::default();
    for case in takt_conformance::cases::cases(name, p) {
        let Ok(stimulus) = takt_interp::Trace::parse(&case.stimulus) else { continue };
        let options = takt_interp::RunOptions { ticks: case.ticks, inputs: true, ..Default::default() };
        if let Ok(r) = takt_interp::run(p, &stimulus, &options) {
            coverage.merge(&r.coverage);
        }
    }
    let proven = paths::proof(name).map(|pr| pr.tags()).unwrap_or_default();
    let unfired = paths::unfired(name).map(|u| u.transitions).unwrap_or_default();
    let source = |f: &&takt_mir::fns::Fn| f.span.file.0 == takt_mir::analysis::proof::SOURCE;
    let called: Vec<takt_diag::Span> =
        takt_mir::analysis::reachable_fns(p).iter().map(|f| p.fns[f.index()].span).collect();
    let inside = |s: &takt_diag::Span, a: u32, b: u32| s.start <= a && b <= s.end;
    let uncalled = |a: u32, b: u32| {
        p.fns.iter().filter(source).any(|f| inside(&f.span, a, b))
            && !called.iter().any(|s| s.file.0 == takt_mir::analysis::proof::SOURCE && inside(s, a, b))
    };
    let mut out = Open::default();
    for item in takt_interp::coverage::items(p) {
        let kind = item.key.split(' ').next().unwrap_or("").to_string();
        let (a, b) = (item.span.start, item.span.end);
        let reason = match item.silent {
            _ if uncalled(a, b) => Some(WITHOUT_MODEL[0]),
            Some(Silent::Proven) => Some(WITHOUT_MODEL[1]),
            Some(Silent::Message) => Some(WITHOUT_MODEL[2]),
            Some(Silent::Observe) => Some(WITHOUT_MODEL[3]),
            None => None,
        };
        if let Some(why) = reason.filter(|_| item.kind != CoverKind::Transition) {
            out.without_model.insert((a, b, kind.clone()), why);
        }
        match item.kind {
            CoverKind::Transition
                if !coverage.has(CoverKind::Transition, &item)
                    && !unfired.iter().any(|(m, k)| *m == item.machine && *k == item.key) =>
            {
                out.transitions.push((item.machine.clone(), item.key.clone()));
            }
            CoverKind::Check => {
                if !coverage.has(CoverKind::Check, &item) {
                    out.passes.push((item.span.start, item.span.end, kind.clone()));
                }
                if kind != "verify" && !coverage.has(CoverKind::CheckFailed, &item) {
                    let start = item.key.rsplit_once('@').and_then(|(_, s)| s.parse().ok()).expect("check @<anfang>");
                    out.checks.push((item.machine.clone(), kind, start));
                }
            }
            CoverKind::Site => {
                if !coverage.has(CoverKind::Site, &item) {
                    out.passes.push((item.span.start, item.span.end, kind.clone()));
                }
                if item.silent.is_none() && !coverage.has(CoverKind::SiteFailed, &item) {
                    let tag = takt_mir::analysis::walk::tag_of_name(&kind);
                    if !tag.is_some_and(|t| proven.contains(&(item.span.start, item.span.end, t))) {
                        out.sites.push((item.span.start, item.span.end, kind));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Die Art einer Zeile fuer das Bestehen einer Stelle (Spalte zwei).
const PASS: &str = "besteht ";

/// Die Art einer Zeile fuer einen Uebergang (Spalte eins), vor seinem
/// Schluessel: Uebergaenge aus einer Klausel, die fuer mehrere Schritte
/// gilt, teilen ihren Anfang.
const TRANSITION: &str = "uebergang ";

/// Die Art der Zeile eines Uebergangs.
fn transition_kind(key: &str) -> String {
    format!("{TRANSITION}{key}")
}

/// Der Anfang eines Uebergangs aus seinem Schluessel (`VON->NACH @<anfang>`).
fn transition_start(key: &str) -> u32 {
    key.rsplit_once('@').and_then(|(_, s)| s.parse().ok()).expect("Uebergang @<anfang>")
}

/// Die Gruppe eines Urteils (FB-511 Schritt 1): erreichbar (auch tiefer,
/// als Spacer zeigt), bewiesen, nicht k-induktiv, ausserhalb des Fragments
/// der Invariantensuche, Frist, Speichergrenze (FB-607), oder ein Pfad, den
/// der Interpreter nicht bestaetigt.
fn group(verdict: &CheckVerdict) -> &'static str {
    match verdict {
        CheckVerdict::Reachable { .. } => "erreichbar",
        CheckVerdict::Unreachable { .. } => "bewiesen",
        CheckVerdict::Undecided { reason } if reason.starts_with("ohne Urteil") => "ohne Urteil",
        CheckVerdict::Undecided { reason } if reason.contains("findet einen Pfad") => "erreichbar, tiefer",
        CheckVerdict::Undecided { reason } if reason.contains("ausserhalb ihres Fragments") => "Fragment",
        CheckVerdict::Undecided { reason } if reason.contains("Induktionsschritt offen") => "nicht k-induktiv",
        CheckVerdict::Undecided { reason } if reason.contains("out of memory") => "Speicher",
        CheckVerdict::Undecided { reason } if reason.contains("Solver sagt") => "Frist",
        CheckVerdict::Undecided { .. } => "unbestaetigt",
    }
}

/// Eine Zeile des Protokolls (`fb511-<tiefe>.tsv`): Programm, Maschine, Art,
/// Anfang der Anweisung, Spanne, Gruppe, Urteil. Ein Uebergang hat als Art
/// seinen Schluessel mit [`TRANSITION`] davor, das Bestehen einer Stelle die
/// Art mit [`PASS`] davor.
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

    /// Ein Uebergang als Zeile: Anfang und Spanne aus seinem Schluessel.
    fn of_transition(name: &str, r: &takt_prove::TransitionReport) -> Row {
        let start = transition_start(&r.key);
        let report = CheckReport {
            start,
            span: takt_diag::Span::new(start, start),
            machine: r.machine.clone(),
            kind: transition_kind(&r.key),
            verdict: r.verdict.clone(),
        };
        Row::of(name, &report)
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

    /// Entschieden: erreichbar mit bestaetigtem Pfad, bewiesen, oder eine
    /// Stelle, die kein Modell fuehrt ([`WITHOUT_MODEL`]).
    fn decided(&self) -> bool {
        self.group == "erreichbar" || self.group == "bewiesen" || WITHOUT_MODEL.contains(&self.group.as_str())
    }
}

/// Was eine Einordnung fragt (`CLASSIFY_WHAT`, Vorgabe alles): die nie
/// verletzten Stellen (`stellen`), die nie genommenen Uebergaenge
/// (`uebergaenge`), die nie bestandenen Stellen (`bestehen`; Schritt 5).
struct What {
    sites: bool,
    transitions: bool,
    passes: bool,
}

impl What {
    fn from_env() -> What {
        let v = std::env::var("CLASSIFY_WHAT").unwrap_or_else(|_| "stellen,uebergaenge,bestehen".into());
        let has = |w: &str| v.split(',').any(|x| x.trim() == w);
        let what = What { sites: has("stellen"), transitions: has("uebergaenge"), passes: has("bestehen") };
        assert!(what.sites || what.transitions || what.passes, "CLASSIFY_WHAT: `{v}` nennt nichts");
        what
    }
}

/// Die Stellen ohne Fault eines Programms (Spalte drei) als Zeilen.
fn site_rows(
    name: &str,
    p: &Program,
    open: &Open,
    whole: Option<&takt_prove::Model>,
    depth: u32,
    solver: &Solver,
) -> Result<Vec<Row>, String> {
    let (checks, sites) = (&open.checks, &open.sites);
    let keep = |c: &takt_prove::CheckSite| {
        checks.iter().any(|(m, k, s)| *m == c.machine && *k == c.kind && *s == c.start)
            || sites.iter().any(|(a, b, k)| *a == c.span.start && *b == c.span.end && *k == c.kind)
    };
    let (reports, notes) = takt_prove::classify_sites(p, whole, depth, solver, CLASSIFY_TIMEOUT_S, &keep)?;
    // Eine Stelle ohne Urteil: Kein Modell fuehrt sie.
    let none = |kind: &str, machine: &str, start: u32, end: u32| CheckReport {
        start,
        span: takt_diag::Span::new(start, end),
        machine: machine.to_string(),
        kind: kind.to_string(),
        verdict: CheckVerdict::Undecided { reason: format!("ohne Urteil; {}", notes.join("; ")) },
    };
    let mut missing: Vec<CheckReport> = checks
        .iter()
        .filter(|(m, k, s)| !reports.iter().any(|r| r.machine == *m && r.kind == *k && r.start == *s))
        .map(|(m, k, s)| none(k, m, *s, *s))
        .collect();
    missing.extend(
        sites
            .iter()
            .filter(|(a, b, k)| !reports.iter().any(|r| r.span.start == *a && r.span.end == *b && r.kind == *k))
            .map(|(a, b, k)| none(k, "", *a, *b)),
    );
    let mut rows: Vec<Row> = reports.iter().map(|r| Row::of(name, r)).collect();
    for r in missing.iter().map(|r| Row::of(name, r)) {
        let group = open.without_model.get(&(r.span.0, r.span.1, r.kind.clone()));
        rows.push(Row { group: group.map_or(r.group.clone(), |g| (*g).to_string()), ..r });
    }
    Ok(rows)
}

/// Die nie genommenen Uebergaenge und nie bestandenen Stellen eines
/// Programms (Spalten eins und zwei, FB-511 Schritt 5) als Zeilen: am
/// Gesamtmodell, ohne es an den Modellen der Maschinen. Was kein Modell
/// fuehrt, steht ohne Urteil.
fn transition_and_pass_rows(
    name: &str,
    p: &Program,
    open: &Open,
    whole: Result<&takt_prove::Model, &takt_prove::Unsupported>,
    what: &What,
    depth: u32,
    solver: &Solver,
) -> Result<Vec<Row>, String> {
    let mut notes = Vec::new();
    let models: Vec<takt_prove::Model> = match whole {
        Ok(w) => vec![w.clone()],
        Err(e) => {
            notes.push(format!("Gesamtmodell: {}", e.what));
            let mut models = Vec::new();
            for id in takt_mir::analysis::schedule::runnable(p) {
                match takt_prove::encode_machine(p, id) {
                    Ok(m) => models.push(m),
                    Err(e) => notes.push(format!("`{}` nicht kodierbar: {}", p.machines[id.index()].name, e.what)),
                }
            }
            models
        }
    };
    let notes = if notes.is_empty() { String::new() } else { format!("; {}", notes.join("; ")) };
    let mut rows = Vec::new();
    for model in &models {
        if what.transitions && !open.transitions.is_empty() {
            let reports =
                takt_prove::classify_transitions(model, p, &open.transitions, depth, solver, CLASSIFY_TIMEOUT_S)?;
            rows.extend(reports.iter().map(|r| Row::of_transition(name, r)));
        }
        if what.passes && !open.passes.is_empty() {
            let reports = takt_prove::classify_passes(model, p, &open.passes, depth, solver, CLASSIFY_TIMEOUT_S)?;
            rows.extend(reports.iter().map(|r| Row { kind: format!("{PASS}{}", r.kind), ..Row::of(name, r) }));
        }
    }
    // Eine Stelle in einer Funktion steht in jedem Maschinenmodell, das sie
    // ruft: erreichbar, wenn eines sie erreicht, bewiesen nur, wenn alle es
    // beweisen.
    let rank = |r: &Row| match r.group.as_str() {
        "erreichbar" => 0,
        "bewiesen" => 2,
        _ => 1,
    };
    let key = |r: &Row| {
        let machine = if r.kind.starts_with(TRANSITION) { r.machine.clone() } else { String::new() };
        (r.kind.clone(), machine, r.span)
    };
    rows.sort_by_key(|r| (key(r), rank(r)));
    rows.dedup_by(|b, a| key(a) == key(b));
    let none = |machine: &str, kind: String, start: u32, end: u32, why: String| Row {
        name: name.to_string(),
        machine: machine.to_string(),
        kind,
        start,
        span: (start, end),
        group: "ohne Urteil".into(),
        why: format!("{why}{notes}"),
    };
    if what.transitions {
        for (m, key) in &open.transitions {
            let (start, kind) = (transition_start(key), transition_kind(key));
            if !rows.iter().any(|r| r.kind == kind && r.machine == *m) {
                rows.push(none(m, kind, start, start, "unentschieden: kein Modell".into()));
            }
        }
    }
    if what.passes {
        for (a, b, k) in &open.passes {
            let kind = format!("{PASS}{k}");
            if !rows.iter().any(|r| r.kind == kind && r.span == (*a, *b)) {
                let row = none("", kind, *a, *b, "unentschieden: kein Modell".into());
                let group = open.without_model.get(&(*a, *b, k.clone()));
                rows.push(Row { group: group.map_or(row.group.clone(), |g| (*g).to_string()), ..row });
            }
        }
    }
    Ok(rows)
}

/// **FB-511 Schritte 1 und 5: das Unerreichte einordnen.** Je Tiefe aus
/// `depths` (`10,20`) fragt der Solver mit [`CLASSIFY_TIMEOUT_S`] je Anfrage,
/// was die vorige Tiefe nicht entschied — die Stellen ohne Fault, die nie
/// genommenen Uebergaenge und die nie bestandenen Stellen (`CLASSIFY_WHAT`);
/// das Ergebnis steht je Stelle in `CARGO_TARGET_TMPDIR/fb511-<tiefe>.tsv`
/// und gezaehlt im Protokoll. Eine entschiedene Antwort nimmt der Cache. Im
/// Release-Bau, wie `UPDATE_PATHS`.
///
/// Weitere Variablen: `CLASSIFY_PROGRAMS=<datei>,<datei>` beschraenkt den
/// Lauf; `CLASSIFY_SHARD=i/n` nimmt aus den offenen Programmen, nach Namen
/// sortiert, jedes n-te ab i (ab null), fuer parallele Laeufe auf mehreren
/// Rechnern, und haengt `-s<i>von<n>` an die Dateinamen;
/// `CLASSIFY_MEMORY_MB` ist die Speichergrenze je z3-Anfrage (Vorgabe 2048,
/// FB-607); `CLASSIFY_WORKERS` die Zahl der Faeden (Vorgabe: die Haelfte der
/// Kerne ohne einen); `CLASSIFY_RESUME=1` setzt einen abgebrochenen Lauf fort.
fn classify_unreached(depths: &str) {
    let found = Some(takt_prove::find()).filter(|s| !s.is_missing());
    let Some(solver) = takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren") else {
        return;
    };
    // 2 GB je Anfrage statt `MEMORY_MB`: Neben den zwei Faeden laufen
    // WSL, Editoren und Sprachserver, und frei sind oft nur 9 GB Commit
    // (FB-607). Was mehr braucht, entscheidet in der Frist ohnehin kaum.
    let number = |var: &str, default: u64| {
        std::env::var(var).map_or(default, |v| v.trim().parse().unwrap_or_else(|_| panic!("{var}: Zahl erwartet")))
    };
    let memory = number("CLASSIFY_MEMORY_MB", 2048);
    let solver =
        solver.cached(std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("solver-cache")).with_memory(memory);
    let what = What::from_env();
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let workers = number("CLASSIFY_WORKERS", ((cores - 1) / 2).max(1) as u64).max(1) as usize;
    let shard = std::env::var("CLASSIFY_SHARD").ok().map(|v| {
        let parsed = v
            .split_once('/')
            .and_then(|(i, n)| Some((i.trim().parse::<usize>().ok()?, n.trim().parse::<usize>().ok()?)));
        match parsed {
            Some((i, n)) if i < n => (i, n),
            _ => panic!("CLASSIFY_SHARD: `i/n` mit 0 <= i < n erwartet, nicht `{v}`"),
        }
    });
    let suffix = shard.map_or(String::new(), |(i, n)| format!("-s{i}von{n}"));
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
        let mut items = open_items(name, &p);
        if !what.sites {
            (items.checks, items.sites) = (Vec::new(), Vec::new());
        }
        if !what.transitions {
            items.transitions.clear();
        }
        if !what.passes {
            items.passes.clear();
        }
        if !items.is_empty() {
            open.insert(name, items);
        }
    }
    let all = open.len();
    if let Some((i, n)) = shard {
        let mut k = 0;
        open.retain(|_, _| {
            k += 1;
            (k - 1) % n == i
        });
    }
    eprintln!(
        "FB-511: {all} offene Programme, davon {} in diesem Lauf{}; {workers} Faeden, {memory} MB je Anfrage",
        open.len(),
        shard.map_or(String::new(), |(i, n)| format!(" (Teil {i} von {n})"))
    );
    let tmp = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    for depth in depths.split(',').map(|d| d.trim().parse::<u32>().expect("Tiefe")) {
        let started = std::time::Instant::now();
        // `CLASSIFY_RESUME=1` nimmt, was ein abgebrochener Lauf dieser Tiefe
        // schon eingeordnet hat; sonst beginnt die Tiefe neu.
        let progress = tmp.join(format!("fb511-{depth}{suffix}.laufend.tsv"));
        let resumed: Vec<Row> = match std::env::var("CLASSIFY_RESUME") {
            Ok(_) => std::fs::read_to_string(&progress).unwrap_or_default().lines().filter_map(Row::parse).collect(),
            Err(_) => {
                let _ = std::fs::remove_file(&progress);
                Vec::new()
            }
        };
        let done: Vec<&str> = resumed.iter().map(|r| r.name.as_str()).collect();
        let queue = std::sync::Mutex::new(
            open.iter().filter(|(n, o)| !done.contains(n) && !o.is_empty()).map(|(n, _)| *n).collect::<Vec<_>>(),
        );
        let rows = std::sync::Mutex::new(resumed.clone());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let worker = || {
                    loop {
                        let next = queue.lock().expect("Warteschlange").pop();
                        let Some(name) = next else { break };
                        let items = &open[name];
                        let src = source(name);
                        let p = compile(name, &src, paths::proof(name).as_ref());
                        let whole = takt_prove::encode(&p);
                        let mut mine = Vec::new();
                        if !items.checks.is_empty() || !items.sites.is_empty() {
                            match site_rows(name, &p, items, whole.as_ref().ok(), depth, &solver) {
                                Ok(r) => mine.extend(r),
                                Err(e) => eprintln!("{name}: {e}"),
                            }
                        }
                        if !items.transitions.is_empty() || !items.passes.is_empty() {
                            match transition_and_pass_rows(name, &p, items, whole.as_ref(), &what, depth, &solver) {
                                Ok(r) => mine.extend(r),
                                Err(e) => eprintln!("{name}: {e}"),
                            }
                        }
                        // Je Programm gleich ins Protokoll: Ein abgebrochener Lauf
                        // verliert so nur das laufende.
                        let lines: String = mine.iter().map(Row::line).collect();
                        let mut rows = rows.lock().expect("Zeilen");
                        let appended = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&progress)
                            .and_then(|mut f| std::io::Write::write_all(&mut f, lines.as_bytes()));
                        appended.unwrap_or_else(|e| panic!("{}: {e}", progress.display()));
                        eprintln!("{name}: {} Zeilen nach {:.0?}", mine.len(), started.elapsed());
                        rows.extend(mine);
                    }
                };
                std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
            }
        });
        let mut rows = rows.into_inner().expect("Zeilen");
        rows.sort_by(|a, b| (&a.name, a.start, &a.kind).cmp(&(&b.name, b.start, &b.kind)));
        let mut text = String::new();
        let mut tally: BTreeMap<(&str, &str), usize> = BTreeMap::new();
        for r in &rows {
            let kind = if r.kind.starts_with(TRANSITION) { TRANSITION.trim_end() } else { r.kind.as_str() };
            *tally.entry((r.group.as_str(), kind)).or_default() += 1;
            text.push_str(&r.line());
        }
        let file = tmp.join(format!("fb511-{depth}{suffix}.tsv"));
        std::fs::write(&file, text).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        eprintln!("Tiefe {depth}: {} Zeilen in {:.0?}, {}", rows.len(), started.elapsed(), file.display());
        let mut by_group: BTreeMap<&str, usize> = BTreeMap::new();
        for ((g, kind), n) in &tally {
            *by_group.entry(g).or_default() += n;
            eprintln!("  {g:<20} {kind:<16} {n}");
        }
        eprintln!("  {by_group:?}");
        // Tiefer gefragt wird nur, was diese Tiefe offen liess.
        for (name, items) in open.iter_mut() {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.name == *name && r.decided()).collect();
            items.checks.retain(|(m, k, s)| !mine.iter().any(|r| r.machine == *m && r.kind == *k && r.start == *s));
            items.sites.retain(|(a, b, k)| !mine.iter().any(|r| r.span == (*a, *b) && r.kind == *k));
            items
                .transitions
                .retain(|(m, key)| !mine.iter().any(|r| r.machine == *m && r.kind == transition_kind(key)));
            items.passes.retain(|(a, b, k)| !mine.iter().any(|r| r.span == (*a, *b) && r.kind == format!("{PASS}{k}")));
        }
    }
}

/// **Die Pfade und Beweise des Solvers passen zum Korpus.** Jede
/// Beweisdatei ist an ihre Quelle gebunden, jeder Pfad erreicht sein Ziel,
/// und Pfade des Solvers hat nur ein Programm der Suite `beweiser` oder ein
/// Beispiel.
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
    // Pfade aus dem langen Lauf hat jedes Programm des Vergleichs, Pfade des
    // Solvers nur eines der Suite `beweiser` oder ein Beispiel.
    for name in compared().into_iter().filter(|n| !programs().contains(n)) {
        for path in paths::load(name).into_iter().filter(|p| !p.from_run()) {
            failed.push(format!(
                "{name}: `{}` ist ein Pfad des Solvers, das Programm steht nicht in `beweiser`, ist kein Beispiel",
                path.label
            ));
        }
        known.push(paths::paths_dir(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    }
    for entry in std::fs::read_dir(paths::dir()).into_iter().flatten().flatten() {
        let n = entry.file_name().to_string_lossy().into_owned();
        if !known.contains(&n) {
            failed.push(format!("`corpus-try/paths/{n}` gehoert zu keinem Programm des Vergleichs"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Kein Pfad ist tiefer, als der Vergleich laeuft**: Der Solver vertieft
/// einen Pfad bis `takt_prove::DEEPEST`, der lange Lauf geht bis
/// `cases::MAX_TICKS` (M11 Strang B2, Schritt 3). Der Beweiser kennt diese
/// Crate nicht; die beiden Zahlen haelt dieser Test gleich.
#[test]
fn the_deepest_path_fits_the_comparison() {
    assert_eq!(u64::from(takt_prove::DEEPEST), takt_conformance::cases::MAX_TICKS);
}

/// Die Programme des Vergleichs (`differential.rs`): die Suite `vergleich`
/// und die Beispiele.
fn compared() -> Vec<&'static str> {
    let mut out = takt_conformance::suites::programs("vergleich");
    out.extend(takt_conformance::suites::EXAMPLES);
    out
}

/// **Die Pfade aus dem langen Lauf sind die, die er heute findet** (M11
/// Strang B2, Schritt 3; FB-613). Je Programm des Vergleichs rechnet der
/// Interpreter den Lauf ohne Stimulus neu (`paths::from_run`); jeder
/// gespeicherte Pfad dieser Quelle muss herauskommen und jeder gefundene
/// gespeichert sein. Ein Programm, dessen langer Lauf eine neue Stelle
/// erreicht oder eine alte nicht mehr, faellt so auf, auch wenn die Zahlen
/// von `UNREACHED` gleich blieben. `UPDATE_RUN_PATHS=1` schreibt sie neu,
/// `=<datei>,<datei>` nur fuer diese Programme.
#[test]
fn the_stored_run_paths_are_current() {
    let which = std::env::var("UPDATE_RUN_PATHS").ok();
    let chosen: Vec<String> = which.iter().flat_map(|w| w.split(',')).map(|n| n.trim().to_string()).collect();
    let all = compared();
    if let Some(name) = chosen.iter().find(|n| *n != "1" && !all.contains(&n.as_str())) {
        panic!("`{name}` steht nicht im Vergleich");
    }
    let update = |name: &str| chosen.iter().any(|n| n == "1" || n == name);
    let hint = "`UPDATE_RUN_PATHS=<datei> cargo nextest run -p takt-conformance --test solver_paths` schreibt sie neu";
    let queue = std::sync::Mutex::new(all);
    let failed = std::sync::Mutex::new(Vec::new());
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..(cores - 1).max(1) {
            let worker = || {
                loop {
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    let src = source(name);
                    let p = compile(name, &src, paths::proof(name).as_ref());
                    let found = paths::from_run(name, &p);
                    if update(name) {
                        paths::store_run(name, &found).unwrap_or_else(|e| panic!("{name}: {e}"));
                        if !found.is_empty() {
                            eprintln!("{name}: {} Pfade aus dem langen Lauf", found.len());
                        }
                        continue;
                    }
                    let stored: Vec<Path> = paths::load(name).into_iter().filter(Path::from_run).collect();
                    let mut out = failed.lock().expect("Fehlschlaege");
                    for p in found.iter().filter(|p| !stored.contains(p)) {
                        out.push(format!("{name}: `{}` ({:?}) fehlt; {hint}", p.label, p.target));
                    }
                    for p in stored.iter().filter(|p| !found.contains(p)) {
                        out.push(format!("{name}: `{}` findet der lange Lauf nicht mehr; {hint}", p.label));
                    }
                }
            };
            // Sema und Interpreter gehen tief: 64 MiB Stapel je Faden.
            std::thread::Builder::new().stack_size(64 << 20).spawn_scoped(scope, worker).expect("Faden");
        }
    });
    let mut failed = failed.into_inner().expect("Fehlschlaege");
    failed.sort();
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
