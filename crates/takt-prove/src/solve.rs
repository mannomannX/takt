//! Die Solver-Anbindung (plan/m6.md 2.8): z3 oder cvc5 auf dem PATH, wie
//! `clang` fuer `takt build`. Je Eigenschaft zwei Anfragen — BMC bis zur
//! Tiefe, dann der Induktionsschritt — und drei Ausgaenge: bewiesen
//! (k-Induktion), verletzt mit Gegenbeispiel, unbewiesen mit Grund. Ein
//! Gegenbeispiel wird als Stimulus gebaut und im Interpreter nachgespielt:
//! Verletzt es die Eigenschaft dort nicht, ist die Kodierung falsch, nicht
//! das Programm — und der Bericht sagt das (Soundness-Test).

use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_interp::property::Outcome;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::program::{Direction, Program};
use takt_mir::types::{IntWidth, Type};

use crate::encode::Model;
use crate::eval::Val;
use crate::smt::{Query, Target, assumptions_of, at, contract_query, fail_query, horn, houdini, query};
use crate::term::Term;

/// Der gefundene Solver: sein Pfad, der Cache seiner entschiedenen
/// Antworten ([`Solver::cached`]) und die Speichergrenze je Anfrage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solver {
    /// Der Pfad; keiner, wenn kein Solver gefunden ist.
    path: Option<PathBuf>,
    /// Das Verzeichnis der Antworten, dazu Name und Version
    /// ([`Solver::identity`]), Teil jedes Schluessels.
    cache: Option<(PathBuf, String)>,
    /// Die Speichergrenze je Anfrage in MB ([`MEMORY_MB`]).
    memory_mb: u64,
}

/// Wie viel Speicher z3 je Anfrage hoechstens nimmt, in MB. Die langen
/// Laeufe (`solver_paths`) fragen in zwei Faeden; zweimal 4 GB und der
/// Testprozess bleiben unter der Commit-Grenze des Rechners (31 GB, im
/// Leerlauf 14 GB belegt). Ohne Grenze nahm ein z3 6 GB, und der Rechner
/// fror ein (FB-607). Darueber antwortet z3 `(error "out of memory")`, und
/// das Urteil bleibt offen.
pub const MEMORY_MB: u64 = 4096;

/// So tief folgt die Suche einem Pfad, den die Invariantensuche jenseits der
/// Tiefe sieht ([`deeper`]; M11 Strang B2, Schritt 3): so tief, wie die
/// Abnahme ein Programm hoechstens laufen laesst (`cases::MAX_TICKS` in
/// `takt-conformance`).
pub const DEEPEST: u32 = 2000;

/// Sucht `TAKT_SOLVER`, dann `z3` und `cvc5`, je auf dem PATH oder unter
/// `~/.takt/bin`.
pub fn find() -> Solver {
    let given = std::env::var("TAKT_SOLVER").ok().map(PathBuf::from).filter(|p| answers(p));
    match given {
        Some(p) => Solver::at(p),
        None => ["z3", "cvc5"].into_iter().map(named).find(Solver::works).unwrap_or_else(Solver::missing),
    }
}

/// Der Solver `name` (`z3`, `cvc5`) auf dem PATH oder unter `~/.takt/bin`:
/// Wer beide vergleicht, waehlt jeden selbst (M11 Schritt 28c).
pub fn named(name: &str) -> Solver {
    let mut candidates = vec![PathBuf::from(name)];
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        candidates.push(PathBuf::from(home).join(".takt").join("bin").join(name));
    }
    candidates.into_iter().find(|p| answers(p)).map_or_else(Solver::missing, Solver::at)
}

/// Antwortet das Werkzeug unter `path` auf `--version`?
fn answers(path: &Path) -> bool {
    Command::new(path).arg("--version").output().is_ok_and(|o| o.status.success())
}

impl Solver {
    /// Der Solver unter `path`, ohne Cache, mit [`MEMORY_MB`].
    pub fn at(path: impl Into<PathBuf>) -> Solver {
        Solver { path: Some(path.into()), cache: None, memory_mb: MEMORY_MB }
    }

    /// Kein Solver.
    pub fn missing() -> Solver {
        Solver { path: None, cache: None, memory_mb: MEMORY_MB }
    }

    /// Fehlt der Solver?
    pub fn is_missing(&self) -> bool {
        self.path.is_none()
    }

    /// Derselbe Solver mit einer anderen Speichergrenze je Anfrage, in MB.
    pub fn with_memory(self, memory_mb: u64) -> Solver {
        Solver { memory_mb, ..self }
    }

    /// Der Pfad, wenn gefunden.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Derselbe Solver, der entschiedene Antworten in `dir` ablegt und dort
    /// wiederfindet (M11 B2 Schritt 0): Schluessel ist der SHA-256 aus Name
    /// und Version des Solvers, Zeitlimit und Anfrage. Nur eine Ausgabe, in
    /// der jede Anfrage `sat` oder `unsat` bekam, kommt hinein; ein
    /// Zeitlimit, `unknown` oder die Speichergrenze haengt an der Last des
    /// Rechners und wird neu gefragt.
    pub fn cached(self, dir: impl Into<PathBuf>) -> Solver {
        if self.is_missing() {
            return self;
        }
        let identity = self.identity().unwrap_or_default();
        Solver { cache: Some((dir.into(), identity)), ..self }
    }

    /// Antwortet der Solver auf `--version`?
    pub fn works(&self) -> bool {
        self.path().is_some_and(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
    }

    /// Name und Version, wie Bericht und Beweisdatei sie fuehren (FB-380):
    /// `z3 4.13.4`, `cvc5 1.2.0`. Eine andere Version kann anders urteilen.
    pub fn identity(&self) -> Option<String> {
        if let Some((_, identity)) = &self.cache {
            return Some(identity.clone());
        }
        let path = self.path()?;
        let out = Command::new(path).arg("--version").output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().next()?.trim();
        let mut words = line.split_whitespace();
        let version = words.by_ref().find(|w| w.eq_ignore_ascii_case("version")).and_then(|_| words.next());
        let name = path.file_stem()?.to_str()?;
        Some(format!("{name} {}", version.unwrap_or(line)))
    }

    fn is_cvc5(&self) -> bool {
        self.path().and_then(|p| p.file_stem()).and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("cvc5"))
    }

    /// Fuehrt ein Skript aus und liefert die Ausgabe; mit Cache eine
    /// entschiedene Antwort von frueher, ohne den Solver zu fragen.
    fn run(&self, script: &str, timeout_s: u64, tag: &str) -> Result<String, String> {
        // Eindeutig je Aufruf: Tests laufen nebenlaeufig im selben Prozess.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cache = self.cache.as_ref().map(|(dir, identity)| (dir, cache_key(identity, timeout_s, script)));
        if let Some((dir, key)) = &cache
            && let Ok(text) = std::fs::read_to_string(dir.join(format!("{key}.out")))
        {
            return Ok(text);
        }
        let text = self.ask(script, timeout_s, tag, n)?;
        if let Some((dir, key)) = cache
            && decided(script, &text)
        {
            // Erst ganz geschrieben, dann unter dem Schluessel: Ein Leser
            // nebenan sieht nie eine halbe Antwort. Scheitert es, fragt der
            // naechste Lauf den Solver wieder.
            let part = dir.join(format!("{key}.{}.{n}.part", std::process::id()));
            if std::fs::create_dir_all(dir).is_ok()
                && std::fs::write(&part, &text).is_ok()
                && std::fs::rename(&part, dir.join(format!("{key}.out"))).is_err()
            {
                let _ = std::fs::remove_file(&part);
            }
        }
        Ok(text)
    }

    /// Fragt den Solver: das Skript als Datei, die Ausgabe als Text.
    fn ask(&self, script: &str, timeout_s: u64, tag: &str, n: usize) -> Result<String, String> {
        let path = self.path().ok_or("kein Solver")?;
        let dir = std::env::temp_dir().join(format!("takt-prove-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let file = dir.join(format!("{tag}-{n}.smt2"));
        std::fs::write(&file, script).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(path);
        if self.is_cvc5() {
            // Die Anfragen arbeiten mit `push`/`pop`; cvc5 lehnt das ohne
            // `--incremental` ab und entschied so nichts (FB-492).
            let limit = format!("--tlimit={}", timeout_s * 1000);
            cmd.args(["--lang=smt2", "--incremental", "--produce-models", &limit]);
        } else {
            // Ueber der Grenze antwortet z3 `(error "out of memory")` (FB-607).
            cmd.args(["-smt2", &format!("-T:{timeout_s}"), &format!("-memory:{}", self.memory_mb)]);
        }
        let out = cmd.arg(&file).output().map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&file);
        // Leer erst nach dem letzten nebenlaeufigen Aufruf; bis dahin scheitert es still.
        let _ = std::fs::remove_dir(&dir);
        // Die Speichergrenze meldet z3 auf stderr (FB-607). Was er davor auf
        // stdout schrieb, gilt nicht: Ein `sat` haette keine Belegung.
        if String::from_utf8_lossy(&out.stderr).contains("out of memory") {
            return Ok("(error \"out of memory\")\n".to_string());
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if text.trim().is_empty() {
            return Err(format!("{}: keine Ausgabe\n{}", path.display(), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(text)
    }
}

/// Der Schluessel einer Antwort im Cache: SHA-256 ueber Solver mit Version,
/// Zeitlimit und Anfrage, hexadezimal.
fn cache_key(identity: &str, timeout_s: u64, script: &str) -> String {
    let mut ctx = takt_native::sha256::Ctx::new();
    ctx.update(format!("{identity}\n{timeout_s}\n").as_bytes());
    ctx.update(script.as_bytes());
    ctx.finish().iter().map(|b| format!("{b:02x}")).collect()
}

/// Hat der Solver entschieden? Jedes `(check-sat)` des Skripts bekam `sat`
/// oder `unsat`. Ein Fehler zaehlt nicht dagegen: `get-value` nach `unsat`
/// ist einer, und ein Abbruch laesst Antworten fehlen. Die Speichergrenze
/// aber schon: Traf sie erst das `get-value` nach `sat`, fehlte dem Pfad
/// seine Belegung (FB-607).
fn decided(script: &str, text: &str) -> bool {
    let asked = script.matches("(check-sat)").count();
    let answered = text.lines().map(str::trim).filter(|l| *l == "sat" || *l == "unsat").count();
    asked > 0 && answered == asked && !text.contains("out of memory")
}

/// Das Urteil ueber eine Eigenschaft.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// k-Induktion oder eine induktive Invariante.
    Proven {
        /// Schritte der Induktion; null, wenn eine induktive Invariante
        /// bewiesen hat (Spacer, FB-375).
        k: u32,
        /// Die `assumption`-Formeln, auf denen der Beweis ruht (13.3).
        assumptions: Vec<String>,
    },
    /// Ein Gegenbeispiel, im Interpreter bestaetigt.
    Violated {
        /// Position der Verletzung.
        at: u64,
        /// Der Stimulus des Gegenbeispiels (12.5).
        stimulus: String,
    },
    /// Weder noch.
    Unproven {
        /// Warum.
        reason: String,
    },
}

/// Eine Eigenschaft mit Urteil.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// Name.
    pub name: String,
    /// `assumption` statt `property`.
    pub assumption: bool,
    /// Urteil.
    pub verdict: Verdict,
}

impl Report {
    /// Der Text fuer den Bericht. Eine Annahme, die das Modell nicht
    /// garantiert, bleibt Annahme: Sie laeuft als Monitor (13.3), und ihr
    /// Gegenbeispiel ist kein Befund ueber das Programm.
    pub fn text(&self) -> String {
        match &self.verdict {
            Verdict::Violated { at, .. } if self.assumption => format!(
                "vom Modell nicht garantiert, bleibt Annahme (Gegenbeispiel bei t={at} im Interpreter bestaetigt)"
            ),
            v => v.text(),
        }
    }
}

/// Wie bewiesen wurde.
fn method(k: u32) -> String {
    if k == 0 { "induktive Invariante, Spacer".into() } else { format!("k-Induktion, k = {k}") }
}

impl Verdict {
    /// Der Text fuer den Bericht.
    pub fn text(&self) -> String {
        match self {
            Verdict::Proven { k, assumptions } if assumptions.is_empty() => format!("bewiesen ({})", method(*k)),
            Verdict::Proven { k, assumptions } => {
                let names: Vec<String> = assumptions.iter().map(|a| format!("`{a}`")).collect();
                let word = if names.len() == 1 { "der Annahme" } else { "den Annahmen" };
                format!("bewiesen ({}) unter {word} {}", method(*k), names.join(", "))
            }
            Verdict::Violated { at, .. } => format!("verletzt bei t={at} (Gegenbeispiel im Interpreter bestaetigt)"),
            Verdict::Unproven { reason } => format!("unbewiesen: {reason}"),
        }
    }
}

/// Die Klassifikation einer Pruefstelle (B3).
#[derive(Clone, Debug, PartialEq)]
pub enum CheckVerdict {
    /// Bewiesen unerreichbar: kein Pfad bis zur Tiefe, und induktiv.
    Unreachable {
        /// Schritte der Induktion; null wie bei [`Verdict::Proven`].
        k: u32,
    },
    /// Erreichbar mit Pfad, im Interpreter bestaetigt.
    Reachable {
        /// Tick des Faults im Interpreter.
        at: u64,
        /// Der Stimulus (12.5).
        stimulus: String,
    },
    /// Unentschieden.
    Undecided {
        /// Warum.
        reason: String,
    },
}

/// Eine Pruefstelle mit Klassifikation.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckReport {
    /// Anfang der Anweisung.
    pub start: u32,
    /// Position.
    pub span: takt_diag::Span,
    /// Maschine.
    pub machine: String,
    /// `check` oder `expect`.
    pub kind: String,
    /// Klassifikation.
    pub verdict: CheckVerdict,
}

impl CheckReport {
    /// Der Text fuer den Bericht; ein `requires` bestaetigt das Modell,
    /// weil der Interpreter Vertraege nicht prueft (5.7).
    pub fn text(&self) -> String {
        match &self.verdict {
            CheckVerdict::Unreachable { k } => format!("bewiesen unerreichbar ({})", method(*k)),
            CheckVerdict::Reachable { at, .. } if self.kind == "requires" => format!(
                "erreichbar mit Pfad (verletzt bei t={at}; Vertraege prueft der Interpreter nicht, der Pfad ist aus dem Modell)"
            ),
            CheckVerdict::Reachable { at, .. } => {
                format!("erreichbar mit Pfad (Fault bei t={at}, im Interpreter bestaetigt)")
            }
            CheckVerdict::Undecided { reason } => format!("unentschieden: {reason}"),
        }
    }
}

/// Klassifiziert jede Pruefstelle (B3): bewiesen unerreichbar, erreichbar
/// mit Pfad, unentschieden.
pub fn classify(
    model: &Model,
    program: &Program,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<Vec<CheckReport>, String> {
    let model = &strengthened(model, solver, timeout_s)?;
    (0..model.checks.len())
        .map(|i| Ok(report(&model.checks[i], classify_site(model, program, i, depth, solver, timeout_s)?)))
        .collect()
}

/// Pruefstellen je Maschine (13.3): Ψ, fremde Outputs und ein fremder
/// `abort` sind freie Eingaben unter den Annahmen ihrer Typen. Was so
/// unerreichbar ist, ist es im Ganzen; ein Pfad im Maschinenmodell wird
/// am Gesamtmodell gesucht und dort vom Interpreter bestaetigt.
pub fn classify_compositional(
    program: &Program,
    whole: Option<&Model>,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<(Vec<CheckReport>, Vec<String>), String> {
    classify_sites(program, whole, depth, solver, timeout_s, &|_| true)
}

/// Wie [`classify_compositional`], nur ueber die Pruefstellen, die `keep`
/// nennt (FB-511): Was ein Lauf schon erreicht, fragt der Solver nicht, und
/// eine Maschine ohne solche Stelle bekommt keine Lemmata.
pub fn classify_sites(
    program: &Program,
    whole: Option<&Model>,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
    keep: &dyn Fn(&crate::encode::CheckSite) -> bool,
) -> Result<(Vec<CheckReport>, Vec<String>), String> {
    let whole = whole.map(|w| strengthened(w, solver, timeout_s)).transpose()?;
    let whole = whole.as_ref();
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let in_whole = |site: &crate::encode::CheckSite| {
        whole.and_then(|w| w.checks.iter().position(|s| s.span == site.span && s.kind == site.kind))
    };
    // Je laufende Maschine: Eine Vorlage hat ihre Zustaende erst in den
    // Instanzen, ein Szenario laeuft nur unter `takt test` (FB-491).
    for id in takt_mir::analysis::schedule::runnable(program) {
        let machine = &program.machines[id.index()];
        let model = match crate::encode::encode_machine(program, id) {
            Ok(m) if !m.checks.iter().any(keep) => continue,
            Ok(m) => strengthened(&m, solver, timeout_s)?,
            Err(e) => {
                notes.push(format!("`{}` nicht kodierbar: {}", machine.name, e.what));
                if let Some(w) = whole {
                    let mine = w.checks.iter().enumerate().filter(|(_, s)| s.machine == machine.name && keep(s));
                    for (j, site) in mine {
                        out.push(report(site, classify_site(w, program, j, depth, solver, timeout_s)?));
                    }
                }
                continue;
            }
        };
        notes.extend(model.notes.iter().cloned());
        for (j, site) in model.checks.iter().enumerate().filter(|(_, s)| keep(s)) {
            let path = match (whole, in_whole(site)) {
                (Some(w), Some(k)) => PathOn::Whole(w, k),
                _ => PathOn::Nowhere,
            };
            if let Some(verdict) = shallow(&model, program, j, depth, solver, timeout_s, path)? {
                out.push(report(site, verdict));
                continue;
            }
            let bmc = solver.run(&query(&model, depth, Target::Check(j), Query::Bmc), timeout_s, "check-bmc")?;
            let verdict = match answer(&bmc) {
                "unsat" => induction_verdict(&model, program, j, depth, solver, timeout_s, path)?,
                "sat" => match (whole, in_whole(site)) {
                    (Some(w), Some(k)) => classify_site(w, program, k, depth, solver, timeout_s)?,
                    _ => CheckVerdict::Undecided {
                        reason: format!(
                            "Pfad bis Tiefe {depth} im Maschinenmodell (Ψ frei), ohne Gesamtmodell nicht bestaetigbar"
                        ),
                    },
                },
                other => CheckVerdict::Undecided { reason: format!("BMC: Solver sagt `{other}`") },
            };
            out.push(report(site, verdict));
        }
    }
    out.sort_by_key(|c| c.start);
    notes.sort();
    notes.dedup();
    Ok((out, notes))
}

/// Ein Uebergang mit Urteil (M11 Schritt 29): ein Pfad, auf dem er genommen
/// wird, bewiesen nie genommen, oder unentschieden.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionReport {
    /// Die Maschine.
    pub machine: String,
    /// Der Schluessel der Coverage (`VON->NACH @<anfang>`).
    pub key: String,
    /// Das Urteil; `Reachable` traegt den Tick, in dem der Interpreter ihn
    /// nimmt.
    pub verdict: CheckVerdict,
}

/// Klassifiziert die Uebergaenge `which` (Maschine, Schluessel der Coverage)
/// wie Pruefstellen: Einen Pfad bestaetigt der Interpreter an seiner
/// Coverage; was nicht im Modell steht, fehlt im Ergebnis.
pub fn classify_transitions(
    model: &Model,
    program: &Program,
    which: &[(String, String)],
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<Vec<TransitionReport>, String> {
    let model = &strengthened(model, solver, timeout_s)?;
    let mut out = Vec::new();
    for (i, goal) in model.transitions.iter().enumerate() {
        if !which.iter().any(|(m, k)| *m == goal.machine && *k == goal.kind) {
            continue;
        }
        let target = Target::Transition(i);
        let bmc = solver.run(&query(model, depth, target, Query::Bmc), timeout_s, "transition-bmc")?;
        let verdict = match answer(&bmc) {
            "sat" => {
                let values = parse_values(&bmc, "@");
                let stimulus = stimulus(&values, program, depth);
                match taken_at(program, &goal.machine, &goal.kind, &stimulus, depth) {
                    Some(at) => CheckVerdict::Reachable { at, stimulus },
                    None => CheckVerdict::Undecided {
                        reason: format!(
                            "der Solver fand einen Pfad bis Tiefe {depth}, der Interpreter nimmt den Uebergang nicht — {}:\n{stimulus}",
                            unconfirmed(model)
                        ),
                    },
                }
            }
            "unsat" => {
                let ind = solver.run(&query(model, depth, target, Query::Induction), timeout_s, "transition-ind")?;
                match answer(&ind) {
                    "unsat" => CheckVerdict::Unreachable { k: depth },
                    "sat" => match invariant(model, target, solver, timeout_s)? {
                        Search::Invariant => CheckVerdict::Unreachable { k: 0 },
                        found => CheckVerdict::Undecided {
                            reason: format!("kein Pfad bis Tiefe {depth}, {}", open(model, depth, &found)),
                        },
                    },
                    other => CheckVerdict::Undecided { reason: format!("Induktionsschritt: Solver sagt `{other}`") },
                }
            }
            other => CheckVerdict::Undecided { reason: format!("BMC: Solver sagt `{other}`") },
        };
        out.push(TransitionReport { machine: goal.machine.clone(), key: goal.kind.clone(), verdict });
    }
    Ok(out)
}

/// Der erste Tick bis `depth`, nach dem der Interpreter mit `stimulus` den
/// Uebergang `key` der Maschine `machine` genommen hat.
pub fn taken_at(program: &Program, machine: &str, key: &str, stimulus: &str, depth: u32) -> Option<u64> {
    let stim = takt_interp::Trace::parse(stimulus).ok()?;
    (0..=u64::from(depth)).find(|&ticks| {
        let options = takt_interp::RunOptions { ticks, ..Default::default() };
        takt_interp::run(program, &stim, &options).is_ok_and(|r| {
            r.coverage.hits.contains_key(&(takt_interp::CoverKind::Transition, machine.to_string(), key.to_string()))
        })
    })
}

fn report(site: &crate::encode::CheckSite, verdict: CheckVerdict) -> CheckReport {
    CheckReport { start: site.start, span: site.span, machine: site.machine.clone(), kind: site.kind.clone(), verdict }
}

fn classify_site(
    model: &Model,
    program: &Program,
    i: usize,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<CheckVerdict, String> {
    if let Some(verdict) = shallow(model, program, i, depth, solver, timeout_s, PathOn::Here)? {
        return Ok(verdict);
    }
    let site = &model.checks[i];
    let bmc = solver.run(&query(model, depth, Target::Check(i), Query::Bmc), timeout_s, "check-bmc")?;
    Ok(match answer(&bmc) {
        "sat" => {
            let values = parse_values(&bmc, "@");
            let stimulus = stimulus(&values, program, depth);
            let confirmed = if site.kind == "requires" {
                confirm_requires(model, site, &values, depth)
            } else {
                confirm_check(program, site, &stimulus, depth)
            };
            match confirmed {
                Some(at) => CheckVerdict::Reachable { at, stimulus },
                None => CheckVerdict::Undecided {
                    reason: format!(
                        "der Solver fand einen Pfad bis Tiefe {depth}, der Interpreter bestaetigt ihn nicht — {}:\n{stimulus}",
                        unconfirmed(model)
                    ),
                },
            }
        }
        "unsat" => induction_verdict(model, program, i, depth, solver, timeout_s, PathOn::Here)?,
        other => CheckVerdict::Undecided { reason: format!("BMC: Solver sagt `{other}`") },
    })
}

/// Wo ein Pfad, den `shallow` findet, bestaetigt wird.
#[derive(Clone, Copy)]
enum PathOn<'a> {
    /// Am Modell selbst.
    Here,
    /// Ein Maschinenmodell, dessen Ψ frei ist: am Gesamtmodell, Stelle `k`.
    Whole(&'a Model, usize),
    /// Gar nicht: ein Maschinenmodell ohne Gesamtmodell.
    Nowhere,
}

/// Die kleinen Anfragen vor den tiefen (FB-511 Schritt 2): zuerst die
/// Fehlerbedingung allein (`smt::fail_query`), dann BMC und Induktion mit
/// k = 1. Eine Anfrage der Tiefe `depth` rollt den Tick `depth`-mal aus, und
/// in einem Programm aus Fliesskomma kostet jeder Tick den Solver Hunderte
/// MB — eine Division in `f32` etwa 80 MB, in `f64` 160 MB, eine `fma` in
/// `f64` 11 MB. Was sich so schon entscheidet, braucht die tiefen Anfragen
/// nicht; sonst `None`, und es gilt das Urteil der Tiefe `depth`. Reicht
/// der Speicher schon fuer k = 1 nicht, reicht er fuer die Tiefe nicht: Die
/// Stelle bleibt mit diesem Grund offen, ohne die tiefen Anfragen.
fn shallow(
    model: &Model,
    program: &Program,
    i: usize,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
    path: PathOn<'_>,
) -> Result<Option<CheckVerdict>, String> {
    let target = Target::Check(i);
    if let Some(q) = fail_query(model, target)
        && answer(&solver.run(&q, timeout_s, "check-fail")?) == "unsat"
    {
        return Ok(Some(CheckVerdict::Unreachable { k: 1 }));
    }
    if depth <= 1 {
        return Ok(None);
    }
    let memory = |what: &str, said: &str| CheckVerdict::Undecided {
        reason: format!("{what}: Solver sagt `{said}`, schon mit k = 1"),
    };
    let bmc = solver.run(&query(model, 1, target, Query::Bmc), timeout_s, "check-bmc")?;
    Ok(match answer(&bmc) {
        said if said.contains("out of memory") => Some(memory("BMC", said)),
        "unsat" => {
            let ind = solver.run(&query(model, 1, target, Query::Induction), timeout_s, "check-ind")?;
            match answer(&ind) {
                "unsat" => Some(CheckVerdict::Unreachable { k: 1 }),
                said if said.contains("out of memory") => Some(memory("Induktionsschritt", said)),
                _ => None,
            }
        }
        "sat" => match path {
            PathOn::Here => confirmed(model, program, i, &bmc, 1),
            PathOn::Whole(w, k) => {
                let bmc = solver.run(&query(w, 1, Target::Check(k), Query::Bmc), timeout_s, "check-bmc")?;
                if answer(&bmc) == "sat" { confirmed(w, program, k, &bmc, 1) } else { None }
            }
            PathOn::Nowhere => None,
        },
        _ => None,
    })
}

/// Der Pfad einer BMC-Antwort `sat` bis `depth`, wenn der Interpreter ihn
/// bestaetigt (ein `requires` das Modell, 5.7).
fn confirmed(model: &Model, program: &Program, i: usize, bmc: &str, depth: u32) -> Option<CheckVerdict> {
    let site = &model.checks[i];
    let values = parse_values(bmc, "@");
    let stimulus = stimulus(&values, program, depth);
    let at = if site.kind == "requires" {
        confirm_requires(model, site, &values, depth)
    } else {
        confirm_check(program, site, &stimulus, depth)
    }?;
    Some(CheckVerdict::Reachable { at, stimulus })
}

/// Die Invariantensuche nach einem offenen Induktionsschritt (FB-375):
/// `Some(true)`, wenn z3 eine induktive Invariante findet, `Some(false)`,
/// wenn es einen Pfad ueber jede Tiefe hinaus gibt, `None` ohne Urteil —
/// auch ausserhalb des ganzzahligen Fragments und mit cvc5, das keine
/// Horn-Klauseln liest.
fn invariant(model: &Model, target: Target, solver: &Solver, timeout_s: u64) -> Result<Search, String> {
    if solver.is_cvc5() {
        return Ok(Search::Outside("cvc5 sucht keine induktive Invariante (Spacer ist ein Teil von z3)".into()));
    }
    let q = match horn(model, target) {
        Ok(q) => q,
        Err(why) => return Ok(Search::Outside(why)),
    };
    let text = solver.run(&q, timeout_s, "horn")?;
    Ok(match answer(&text) {
        "sat" => Search::Invariant,
        "unsat" => Search::Path,
        _ => Search::Open,
    })
}

/// Was die Invariantensuche (Spacer) ergab.
enum Search {
    /// Eine induktive Invariante: Das Ziel gilt in jeder Tiefe.
    Invariant,
    /// Ein Pfad zur Verletzung, tiefer als die Anfrage.
    Path,
    /// Weder noch, etwa im Zeitlimit.
    Open,
    /// Nicht gesucht: Der Kegel des Ziels liegt ausserhalb des Fragments.
    Outside(String),
}

/// Warum ein Induktionsschritt offen bleibt, und was er braucht.
fn open(model: &Model, depth: u32, search: &Search) -> String {
    if let Search::Path = search {
        return format!(
            "die Invariantensuche findet einen Pfad ueber Tiefe {depth} hinaus; eine groessere Tiefe zeigt ihn"
        );
    }
    let deadline = if depth > model.horizon {
        String::new()
    } else {
        format!("; die laengste Frist braucht k = {} (`--depth auto`)", model.horizon + 1)
    };
    let outside = match search {
        Search::Outside(why) => format!("; keine Invariantensuche, ausserhalb ihres Fragments: {why}"),
        _ => String::new(),
    };
    format!("Induktionsschritt offen (k = {depth}){deadline}{outside}")
}

fn induction_verdict(
    model: &Model,
    program: &Program,
    i: usize,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
    path: PathOn<'_>,
) -> Result<CheckVerdict, String> {
    let ind = solver.run(&query(model, depth, Target::Check(i), Query::Induction), timeout_s, "check-ind")?;
    Ok(match answer(&ind) {
        "unsat" => CheckVerdict::Unreachable { k: depth },
        "sat" => match invariant(model, Target::Check(i), solver, timeout_s)? {
            Search::Invariant => CheckVerdict::Unreachable { k: 0 },
            found => {
                let deep = match found {
                    Search::Path => deeper(model, program, i, depth, solver, timeout_s, path)?,
                    _ => None,
                };
                deep.unwrap_or_else(|| CheckVerdict::Undecided {
                    reason: format!("kein Pfad bis Tiefe {depth}, {}", open(model, depth, &found)),
                })
            }
        },
        other => CheckVerdict::Undecided { reason: format!("Induktionsschritt: Solver sagt `{other}`") },
    })
}

/// Der Pfad, den die Invariantensuche jenseits der Tiefe `depth` sieht (M11
/// Strang B2, Schritt 3): BMC mit jeweils doppelter Tiefe bis [`DEEPEST`],
/// am Modell oder — fuer ein Maschinenmodell — am Gesamtmodell; der erste
/// Pfad, den der Interpreter bestaetigt. Die Frist waechst mit der Tiefe:
/// BMC rollt den Tick so oft aus, wie die Tiefe sagt (`99_exit_fault`, ein
/// Pfad bei t = 72: Tiefe 40 in 18 s, Tiefe 80 in 106 s). Eine Frist, der
/// Speicher oder ein Pfad, den der Interpreter nicht geht, beenden die
/// Suche: tiefer wird keine Anfrage kleiner.
fn deeper(
    model: &Model,
    program: &Program,
    i: usize,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
    path: PathOn<'_>,
) -> Result<Option<CheckVerdict>, String> {
    let (model, i) = match path {
        PathOn::Here => (model, i),
        PathOn::Whole(w, k) => (w, k),
        PathOn::Nowhere => return Ok(None),
    };
    let mut d = depth.max(1);
    while d < DEEPEST {
        d = d.saturating_mul(2).min(DEEPEST);
        let budget = timeout_s.saturating_mul(u64::from(d / depth.max(1)));
        let bmc = solver.run(&query(model, d, Target::Check(i), Query::Bmc), budget, "check-bmc")?;
        match answer(&bmc) {
            "unsat" => {}
            "sat" => return Ok(confirmed(model, program, i, &bmc, d)),
            _ => return Ok(None),
        }
    }
    Ok(None)
}

/// Das Urteil ueber einen Block-Vertrag (5.7, B2).
#[derive(Clone, Debug, PartialEq)]
pub enum ContractVerdict {
    /// Aus jedem typkonformen Zustand gilt `ensures` unter `requires`.
    Proven,
    /// Eine Belegung, unter der der Schritt sein `ensures` bricht.
    Violated {
        /// Die Belegung als `name = wert`.
        values: String,
    },
    /// Weder noch.
    Unknown {
        /// Warum.
        reason: String,
    },
}

/// Ein Block mit Urteil.
#[derive(Clone, Debug, PartialEq)]
pub struct ContractReport {
    /// Der Block.
    pub block: String,
    /// Position.
    pub span: takt_diag::Span,
    /// Urteil.
    pub verdict: ContractVerdict,
}

impl ContractVerdict {
    /// Der Text fuer den Bericht.
    pub fn text(&self) -> String {
        match self {
            ContractVerdict::Proven => "bewiesen (jeder typkonforme Zustand, ein Schritt)".to_string(),
            ContractVerdict::Violated { values } => format!("verletzbar: {values}"),
            ContractVerdict::Unknown { reason } => format!("unentschieden: {reason}"),
        }
    }
}

/// Prueft die Vertraege der Bloecke.
pub fn verify_contracts(model: &Model, solver: &Solver, timeout_s: u64) -> Result<Vec<ContractReport>, String> {
    let mut out = Vec::new();
    for goal in &model.contracts {
        let text = solver.run(&contract_query(goal), timeout_s, "contract")?;
        let verdict = match answer(&text) {
            "unsat" => ContractVerdict::Proven,
            "sat" => {
                let values = parse_plain_values(&text);
                let shown: Vec<String> = goal
                    .vars
                    .iter()
                    .filter_map(|(n, _)| {
                        values.get(n).map(|v| format!("{} = {}", n.trim_start_matches("c."), val_text(v)))
                    })
                    .collect();
                ContractVerdict::Violated { values: shown.join(", ") }
            }
            other => ContractVerdict::Unknown { reason: format!("Solver sagt `{other}`") },
        };
        out.push(ContractReport { block: goal.block.clone(), span: goal.span, verdict });
    }
    Ok(out)
}

fn val_text(v: &Val) -> String {
    match v {
        Val::Bool(b) => b.to_string(),
        Val::Int(i) => i.to_string(),
        Val::F32(f) => format!("{f:?}"),
        Val::F64(f) => format!("{f:?}"),
        Val::Arr(a) => format!("{a:?}"),
    }
}

/// `(get-value …)` ohne Schrittsuffix: Name → Wert.
fn parse_plain_values(text: &str) -> BTreeMap<String, Val> {
    let mut out = BTreeMap::new();
    let Some(start) = text.find("((") else { return out };
    let toks = tokens(&text[start..]);
    let mut pos = 0;
    let Some(Sx::List(pairs)) = parse_sx(&toks, &mut pos) else { return out };
    for pair in pairs {
        let Sx::List(items) = pair else { continue };
        let [Sx::Atom(name), value] = items.as_slice() else { continue };
        if let Some(v) = value_of(value) {
            out.insert(name.trim_matches('|').to_string(), v);
        }
    }
    out
}

/// Ein `requires` an der Aufrufstelle prueft der Interpreter nicht (5.7);
/// den Pfad bestaetigt das Modell selbst: der erste Schritt, in dem die
/// Stelle feuert.
fn confirm_requires(
    model: &Model,
    site: &crate::encode::CheckSite,
    values: &BTreeMap<(u32, String), Val>,
    depth: u32,
) -> Option<u64> {
    let states = model.simulate(u64::from(depth), &|k, n| values.get(&(k as u32, n.to_string())).cloned());
    let mut env: crate::eval::Env = BTreeMap::new();
    for (name, _) in &model.inputs {
        if let Some(v) = values.get(&(0, name.clone())) {
            env.insert(name.clone(), v.clone());
        }
    }
    if crate::eval::eval(&site.init, &env) == Val::Bool(true) {
        return Some(0);
    }
    for k in 0..depth {
        let mut env = states[k as usize].clone();
        for (name, _) in &model.inputs {
            if let Some(v) = values.get(&(k + 1, name.clone())) {
                env.insert(name.clone(), v.clone());
            }
        }
        if crate::eval::eval(&site.fires, &env) == Val::Bool(true) {
            return Some(u64::from(k + 1));
        }
    }
    None
}

/// Warum ein Pfad des Solvers im Interpreter nicht traegt: Sieht der Solver
/// Funktionen uninterpretiert, kann er ihnen Werte geben, die sie nie
/// annehmen (4.2); sonst weicht die Kodierung ab.
fn unconfirmed(model: &Model) -> String {
    if model.uninterpreted.is_empty() {
        "die Kodierung weicht ab, bitte melden".to_string()
    } else {
        format!(
            "der Solver sieht {} uninterpretiert, der Pfad kann an ihrem wahren Wert scheitern",
            model.uninterpreted.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
        )
    }
}

/// Spielt den Pfad nach: der Tick, in dem die Stelle im Interpreter feuert.
///
/// Bestaetigt wird nur die Stelle selbst: eine implizite Pruefung durch
/// einen Fault ihrer Art *an ihrer Stelle* (INT-020). Ein Fault derselben
/// Art an einer anderen Stelle der Maschine hiesse, dass das Modell einen
/// Pfad fand, den der Interpreter nicht geht — die Kodierung weicht ab.
fn confirm_check(program: &Program, site: &crate::encode::CheckSite, stimulus: &str, depth: u32) -> Option<u64> {
    let trace = Trace::parse(stimulus).ok()?;
    let r = run(program, &trace, &RunOptions { ticks: u64::from(depth), ..Default::default() }).ok()?;
    fault_at(&r, &site.machine, &site.kind, site.span)
}

/// Faultet der Lauf `r` an der Pruefstelle der Art `kind` an `span` in
/// `machine`? Der Tick des Faults; so bestaetigt `takt prove` einen Pfad,
/// und so prueft der Vergleich, dass ein gespeicherter Pfad sein Ziel noch
/// erreicht (M11 Schritt 28c).
pub fn fault_at(r: &takt_interp::RunResult, machine: &str, kind: &str, span: takt_diag::Span) -> Option<u64> {
    match kind {
        "check" | "expect" => {
            let name = format!("{kind} @{}", span.start);
            let fired = r.coverage.hits.get(&(takt_interp::CoverKind::CheckFailed, machine.to_string(), name));
            if fired.copied().unwrap_or(0) == 0 {
                return None;
            }
            let word = if kind == "check" { "CheckFailed" } else { "Expect" };
            return r
                .trace
                .lines
                .iter()
                .find(|l| matches!(&l.kind, takt_interp::trace::LineKind::Fault { machine: m, kind: k, .. } if m == machine && k == word))
                .map(|l| l.tick);
        }
        _ => {}
    }
    // Dieselbe Regel wie die Coverage der Pruefstellen: Division und
    // Definitionsbereich prueft der Knoten am Operanden, der Interpreter
    // meldet den Fault an der Operation, die ihn umschliesst.
    let tag = takt_mir::analysis::walk::tag_of_name(kind)?;
    let site = takt_interp::coverage::Site::at(span.start, span.end, tag, matches!(kind, "div" | "dom"));
    r.faults.iter().find(|f| f.machine == machine && site.takes(f.kind, f.span)).map(|f| f.tick)
}

/// Prueft jede Eigenschaft des Modells.
pub fn prove(
    model: &Model,
    program: &Program,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<Vec<Report>, String> {
    // Eine Eigenschaft steht unter den Annahmen, eine Annahme nicht (13.3):
    // Die Lemmata der Eigenschaften duerfen die Annahmen nutzen.
    let plain = strengthened(model, solver, timeout_s)?;
    let assumed = if model.properties.iter().any(|g| g.assumption) {
        strengthened_with(model, solver, timeout_s, true)?
    } else {
        plain.clone()
    };
    let mut out = Vec::new();
    for (i, prop) in model.properties.iter().enumerate() {
        let model = if prop.assumption { &plain } else { &assumed };
        let bmc = solver.run(&query(model, depth, Target::Property(i), Query::Bmc), timeout_s, "bmc")?;
        let verdict = match answer(&bmc) {
            "sat" => {
                let values = parse_values(&bmc, "@");
                let stimulus = stimulus(&values, program, depth);
                match confirm(program, &prop.name, &stimulus, depth) {
                    Some(at) => Verdict::Violated { at, stimulus },
                    None => Verdict::Unproven {
                        reason: format!(
                            "der Solver fand ein Gegenbeispiel bis Tiefe {depth}, der Interpreter bestaetigt es nicht — {}:\n{stimulus}",
                            unconfirmed(model)
                        ),
                    },
                }
            }
            "unsat" => {
                let ind = solver.run(&query(model, depth, Target::Property(i), Query::Induction), timeout_s, "ind")?;
                match answer(&ind) {
                    "unsat" => Verdict::Proven { k: depth, assumptions: assumptions_of(model, Target::Property(i)) },
                    "sat" => match invariant(model, Target::Property(i), solver, timeout_s)? {
                        Search::Invariant => {
                            Verdict::Proven { k: 0, assumptions: assumptions_of(model, Target::Property(i)) }
                        }
                        found => Verdict::Unproven {
                            reason: format!("kein Gegenbeispiel bis Tiefe {depth}, {}", open(model, depth, &found)),
                        },
                    },
                    other => Verdict::Unproven { reason: format!("Induktionsschritt: Solver sagt `{other}`") },
                }
            }
            other => Verdict::Unproven { reason: format!("BMC: Solver sagt `{other}`") },
        };
        out.push(Report { name: prop.name.clone(), assumption: prop.assumption, verdict });
    }
    Ok(out)
}

/// Die Hilfslemmata (Houdini, FB-375): die Kandidaten des Modells, die im
/// Anfangszustand gelten und zusammen induktiv sind. Was in einer Runde
/// fallen kann, faellt, bis der Rest haelt; was bleibt, gilt in jedem
/// erreichbaren Zustand.
pub fn lemmas(model: &Model, solver: &Solver, timeout_s: u64, assumed: bool) -> Result<Vec<Term>, String> {
    let mut cands = surviving(model, solver, timeout_s, model.candidates.iter().collect(), false, assumed)?;
    loop {
        let before = cands.len();
        cands = surviving(model, solver, timeout_s, cands, true, assumed)?;
        if cands.len() == before {
            return Ok(cands.into_iter().cloned().collect());
        }
    }
}

/// Die Kandidaten, die eine Runde nicht fallen laesst (`smt::houdini`).
fn surviving<'a>(
    model: &Model,
    solver: &Solver,
    timeout_s: u64,
    cands: Vec<&'a Term>,
    step: bool,
    assumed: bool,
) -> Result<Vec<&'a Term>, String> {
    if cands.is_empty() {
        return Ok(cands);
    }
    let text = solver.run(&houdini(model, &cands, step, assumed), timeout_s, "lemma")?;
    let answers: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // Ohne Antwort faellt ein Kandidat: Nur was bewiesen haelt, wird Lemma.
    Ok(cands.into_iter().enumerate().filter(|(i, _)| answers.get(*i) == Some(&"unsat")).map(|(_, c)| c).collect())
}

/// Das Modell mit seinen Hilfslemmata unter den Invarianten.
pub fn strengthened(model: &Model, solver: &Solver, timeout_s: u64) -> Result<Model, String> {
    strengthened_with(model, solver, timeout_s, false)
}

/// Wie [`strengthened`]; mit `assumed` unter den `assumption`-Formeln — nur
/// fuer Eigenschaften, die unter ihnen stehen.
fn strengthened_with(model: &Model, solver: &Solver, timeout_s: u64, assumed: bool) -> Result<Model, String> {
    let mut out = model.clone();
    out.invariants.extend(lemmas(model, solver, timeout_s, assumed)?);
    out.candidates.clear();
    Ok(out)
}

/// Die erste Antwortzeile: `sat`, `unsat`, `unknown`, `timeout`.
fn answer(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// S-Ausdruck der Solver-Ausgabe.
#[derive(Clone, Debug)]
enum Sx {
    Atom(String),
    List(Vec<Sx>),
}

fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '|' => {
                quoted = !quoted;
                cur.push(c);
            }
            '(' | ')' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_sx(tokens: &[String], pos: &mut usize) -> Option<Sx> {
    let t = tokens.get(*pos)?;
    *pos += 1;
    if t == "(" {
        let mut items = Vec::new();
        while tokens.get(*pos).is_some_and(|t| t != ")") {
            items.push(parse_sx(tokens, pos)?);
        }
        *pos += 1;
        Some(Sx::List(items))
    } else if t == ")" {
        None
    } else {
        Some(Sx::Atom(t.clone()))
    }
}

/// Ein Wert der Solver-Ausgabe.
fn value_of(sx: &Sx) -> Option<Val> {
    value_in(sx, &BTreeMap::new())
}

/// Ein Wert der Solver-Ausgabe unter den Bindungen `bound`: z3 schreibt
/// lange Ketten von `store` mit `let`.
fn value_in(sx: &Sx, bound: &BTreeMap<String, Val>) -> Option<Val> {
    match sx {
        Sx::Atom(a) if bound.contains_key(a) => bound.get(a).cloned(),
        Sx::List(items) if matches!(items.first(), Some(Sx::Atom(a)) if a == "let") => {
            let [_, Sx::List(binds), body] = items.as_slice() else { return None };
            let mut inner = bound.clone();
            for b in binds {
                let Sx::List(pair) = b else { return None };
                let [Sx::Atom(name), value] = pair.as_slice() else { return None };
                let v = value_in(value, bound)?;
                inner.insert(name.clone(), v);
            }
            value_in(body, &inner)
        }
        // Ein Array (FB-497): z3 und cvc5 schreiben `store` ueber einem
        // konstanten Array.
        Sx::List(items) if matches!(items.first(), Some(Sx::Atom(a)) if a == "store") => {
            let [_, a, i, x] = items.as_slice() else { return None };
            let (Val::Arr(a), Val::Int(i)) = (value_in(a, bound)?, value_in(i, bound)?) else { return None };
            Some(Val::Arr(a.set(i, value_in(x, bound)?)))
        }
        _ => value_flat(sx),
    }
}

/// Ein Wert ohne Bindungen: Wahrheitswert, Bitvektor, Fliesskommazahl,
/// konstantes Array.
fn value_flat(sx: &Sx) -> Option<Val> {
    match sx {
        Sx::Atom(a) => match a.as_str() {
            "true" => Some(Val::Bool(true)),
            "false" => Some(Val::Bool(false)),
            _ => {
                if let Some(hex) = a.strip_prefix("#x") {
                    return u64::from_str_radix(hex, 16).ok().map(|u| Val::Int(u as i64));
                }
                if let Some(bin) = a.strip_prefix("#b") {
                    return u64::from_str_radix(bin, 2).ok().map(|u| Val::Int(u as i64));
                }
                None
            }
        },
        Sx::List(items) if matches!(items.first(), Some(Sx::List(head)) if matches!(head.as_slice(), [Sx::Atom(a), Sx::Atom(c), _] if a == "as" && c == "const")) =>
        {
            let [_, x] = items.as_slice() else { return None };
            Some(Val::Arr(crate::eval::Array::new(value_of(x)?)))
        }
        Sx::List(items) => {
            let atoms: Vec<&str> = items.iter().map(|i| if let Sx::Atom(a) = i { a.as_str() } else { "" }).collect();
            match atoms.as_slice() {
                ["_", bv, "64"] => {
                    bv.strip_prefix("bv").and_then(|n| n.parse::<u64>().ok()).map(|u| Val::Int(u as i64))
                }
                ["fp", s, e, m] => {
                    // z3 schreibt ein Feld hexadezimal, wenn seine Breite durch vier teilbar ist.
                    let bits = |x: &str| -> Option<(u64, usize)> {
                        if let Some(b) = x.strip_prefix("#b") {
                            return u64::from_str_radix(b, 2).ok().map(|v| (v, b.len()));
                        }
                        let h = x.strip_prefix("#x")?;
                        u64::from_str_radix(h, 16).ok().map(|v| (v, 4 * h.len()))
                    };
                    let ((sb, _), (eb, ew), (mb, _)) = (bits(s)?, bits(e)?, bits(m)?);
                    if ew == 11 {
                        Some(Val::F64(f64::from_bits((sb << 63) | (eb << 52) | mb)))
                    } else {
                        Some(Val::F32(f32::from_bits(((sb as u32) << 31) | ((eb as u32) << 23) | mb as u32)))
                    }
                }
                ["_", kind, e, _] => {
                    let f64_ = e == &"11";
                    let v = match *kind {
                        "+zero" => 0.0,
                        "-zero" => -0.0,
                        "+oo" => f64::INFINITY,
                        "-oo" => f64::NEG_INFINITY,
                        _ => f64::NAN,
                    };
                    Some(if f64_ { Val::F64(v) } else { Val::F32(v as f32) })
                }
                _ => None,
            }
        }
    }
}

/// `(get-value …)`: `((|i.x@0| v) …)` als (Schritt, Name) → Wert.
fn parse_values(text: &str, tag: &str) -> BTreeMap<(u32, String), Val> {
    let mut out = BTreeMap::new();
    let start = match text.find("((") {
        Some(i) => i,
        None => return out,
    };
    let toks = tokens(&text[start..]);
    let mut pos = 0;
    let Some(Sx::List(pairs)) = parse_sx(&toks, &mut pos) else { return out };
    for pair in pairs {
        let Sx::List(items) = pair else { continue };
        let [Sx::Atom(name), value] = items.as_slice() else { continue };
        let name = name.trim_matches('|');
        let Some((base, step)) = name.rsplit_once(tag) else { continue };
        let Ok(step) = step.parse::<u32>() else { continue };
        if let Some(v) = value_of(value) {
            out.insert((step, base.to_string()), v);
        }
    }
    out
}

/// Das Gegenbeispiel als Stimulus (12.5): Inputs mit ihrer Qualitaet,
/// Stromelemente mit Zeitstempel, Tunables und Commands je Tick.
///
/// Die Qualitaet liefert der Stimulus so, wie der Rand des Interpreters sie
/// entstehen laesst (3.5): `Good` als Wert, `Suspect` und `Stale` vom
/// Treiber mit dem Wert, den der Rand durchliess (12.6), `Suspect` vom Rand
/// als echte Verletzung unter `debounce` (der Rand haelt dann den letzten
/// guten Wert), `Stale` sonst ohne Wert, `Bad` mit Wert vom Treiber, der den
/// Bezugspunkt loescht.
pub fn stimulus(values: &BTreeMap<(u32, String), Val>, program: &Program, depth: u32) -> String {
    use crate::encode::quality;
    let edges = crate::encode::input_edges(program).unwrap_or_default();
    // Je Kanal der letzte gute Wert und die Ticks seither, wie der Rand sie fuehrt.
    let mut gates: BTreeMap<String, (Option<Val>, i64)> = BTreeMap::new();
    let mut out = String::new();
    // `sys/outer_fault` stellt der Kern aus Abort und Runtime-Faults (13.3).
    let outer = takt_mir::sys::outer_fault(program);
    for k in 0..=depth {
        for (_, c) in
            program.channels.iter().enumerate().filter(|(i, c)| c.dir == Direction::Input && outer != Some(*i))
        {
            if let Type::Samples { elem, len } = program.types.get(c.ty) {
                samples_line(values, program, k, &c.name, *elem, *len, &mut out);
                continue;
            }
            if composite_type(program, c.ty) {
                composite_line(values, program, k, c, &mut out);
                continue;
            }
            let Some(v) = values.get(&(k, format!("i.{}", c.name))) else { continue };
            let q = match values.get(&(k, format!("i.{}.q", c.name))) {
                Some(Val::Int(q)) => *q,
                _ => quality::GOOD,
            };
            let (last, since) = gates.entry(c.name.clone()).or_insert((None, 0));
            // Ohne Lieferung gilt die vorige Abtastung weiter (3.5).
            if values.get(&(k, format!("i.{}.held", c.name))) == Some(&Val::Bool(true)) {
                *since += 1;
                continue;
            }
            let passed = values.get(&(k, format!("i.{}.pass", c.name))) == Some(&Val::Bool(true));
            let delivered = values.get(&(k, format!("i.{}.d", c.name))).cloned().unwrap_or_else(|| v.clone());
            let line = match q {
                quality::SUSPECT | quality::STALE if passed => {
                    let text = value_text(program, c.ty, &delivered);
                    (*last, *since) = (Some(delivered), -1);
                    let flag = if q == quality::SUSPECT { "suspect" } else { "stale" };
                    format!("{text} {flag}")
                }
                quality::SUSPECT => {
                    let edge = edges.iter().find(|e| e.name == c.name);
                    let wrong = edge.and_then(|e| e.violation(last.as_ref(), *since)).unwrap_or_else(|| v.clone());
                    value_text(program, c.ty, &wrong)
                }
                quality::STALE => "stale".to_string(),
                quality::BAD => {
                    *last = None;
                    format!("{} bad", value_text(program, c.ty, v))
                }
                _ => {
                    (*last, *since) = (Some(v.clone()), -1);
                    value_text(program, c.ty, v)
                }
            };
            *since += 1;
            let _ = writeln!(out, "t={k} in {} {line}", c.name);
        }
        // Ein Element, das sich nicht dekodieren laesst, steht als leere
        // Byteform (8.6).
        for c in program.channels.iter().filter(|c| c.dir == Direction::Input) {
            let Type::Stream(elem) = program.types.get(c.ty) else { continue };
            let n = match values.get(&(k, format!("i.stream.{}.n", c.name))) {
                Some(Val::Int(n)) => *n,
                _ => 0,
            };
            for j in 0..n {
                let base = format!("i.stream.{}.{j}", c.name);
                let t = match values.get(&(k, format!("{base}.t"))) {
                    Some(Val::Int(t)) => *t,
                    _ => 0,
                };
                let text = if values.get(&(k, format!("{base}.bad"))) == Some(&Val::Bool(true)) {
                    "0x".to_string()
                } else if let Some(wire) = record_wire(program, *elem, &format!("{base}.v"), values, k) {
                    wire
                } else {
                    element_text(program, *elem, &format!("{base}.v"), &|at| lookup(values, k, at))
                };
                let _ = writeln!(out, "t={k} in {} {text} t={t}", c.name);
            }
        }
        for p in program.params.iter().filter(|p| p.tunable) {
            if let Some(v) = values.get(&(k, format!("i.tune.{}", p.name))) {
                let _ = writeln!(out, "t={k} tune {} {}", p.name, value_text(program, p.ty, v));
            }
        }
        for c in &program.commands {
            if values.get(&(k, format!("i.cmd.{}", c.name))) == Some(&Val::Bool(true)) {
                let _ = writeln!(out, "t={k} cmd {}", c.name);
            }
        }
        if values.get(&(k, crate::encode::OPERATOR_ABORT.to_string())) == Some(&Val::Bool(true)) {
            let _ = writeln!(out, "t={k} abort");
        }
        for kind in crate::encode::RUNTIME_KINDS {
            let outputs: Vec<Option<&str>> = if kind == takt_mir::machine::RuntimeKind::Driver {
                program.channels.iter().filter(|c| c.dir == Direction::Output).map(|c| Some(c.name.as_str())).collect()
            } else {
                vec![None]
            };
            for output in outputs {
                if values.get(&(k, crate::encode::runtime_input(kind, output))) == Some(&Val::Bool(true)) {
                    let _ =
                        writeln!(out, "t={k} runtime {kind:?}{}", output.map(|o| format!(" {o}")).unwrap_or_default());
                }
            }
        }
    }
    job_records(values, program, depth, &mut out);
    out
}

/// Ein Blatt der Belegung im Schritt `k`. Steht es in einem Array (FB-497),
/// heisst sein Ort dort `…[*]…`: Die Indizes `[i]` von links werden zu
/// `[*]`, bis das Array gefunden ist, und waehlen dann in dieser Folge.
fn lookup(values: &BTreeMap<(u32, String), Val>, k: u32, at: &str) -> Option<Val> {
    if let Some(v) = values.get(&(k, at.to_string())) {
        return Some(v.clone());
    }
    let mut name = at.to_string();
    let mut indices = Vec::new();
    while let Some((start, end, i)) = first_index(&name) {
        name.replace_range(start..end, "[*]");
        indices.push(i);
        if let Some(found) = values.get(&(k, name.clone())) {
            return indices.iter().try_fold(found.clone(), |v, i| match v {
                Val::Arr(a) => Some(a.get(*i)),
                _ => None,
            });
        }
    }
    None
}

/// Das erste `[i]` mit einer Zahl in einem Ort: Anfang, Ende, Zahl.
fn first_index(name: &str) -> Option<(usize, usize, i64)> {
    let mut from = 0;
    while let Some(open) = name[from..].find('[').map(|o| o + from) {
        let close = open + name[open..].find(']')?;
        if let Ok(i) = name[open + 1..close].parse::<i64>() {
            return Some((open, close + 1, i));
        }
        from = close + 1;
    }
    None
}

/// Hat ein Input einen zusammengesetzten Wert (`Enc::composite`)?
fn composite_type(program: &Program, ty: takt_mir::TypeId) -> bool {
    match program.types.get(ty) {
        Type::Enum(e) => program.enums[e.index()].variants.iter().any(|v| !v.fields.is_empty()),
        Type::Int { .. } | Type::Float { .. } | Type::Bool | Type::Duration { .. } => false,
        _ => true,
    }
}

/// Eine Lieferung eines Inputs zusammengesetzten Typs: der Wert aus seinen
/// Blaettern, ohne Lieferung keine Zeile, ohne Wert `stale`, vom Treiber `bad`.
fn composite_line(
    values: &BTreeMap<(u32, String), Val>,
    program: &Program,
    k: u32,
    c: &takt_mir::program::Channel,
    out: &mut String,
) {
    let get = |at: &str| lookup(values, k, at);
    let Some(Val::Int(q)) = get(&format!("i.{}.q", c.name)) else { return };
    if get(&format!("i.{}.held", c.name)) == Some(Val::Bool(true)) {
        return;
    }
    let text = element_text(program, c.ty, &format!("i.{}", c.name), &get);
    // `Suspect` kommt hier nur vom Treiber, mit Wert (12.6 Zeile 2).
    let line = match q {
        crate::encode::quality::SUSPECT => format!("{text} suspect"),
        crate::encode::quality::STALE => "stale".to_string(),
        crate::encode::quality::BAD => format!("{text} bad"),
        _ => text,
    };
    let _ = writeln!(out, "t={k} in {} {line}", c.name);
}

/// Eine Lieferung eines oversampelten Inputs (8.9): die Abtastwerte, die der
/// Rand prueft; ohne Lieferung keine Zeile, ohne Werte `stale`, vom Treiber
/// `bad`.
fn samples_line(
    values: &BTreeMap<(u32, String), Val>,
    program: &Program,
    k: u32,
    name: &str,
    elem: takt_mir::TypeId,
    len: u32,
    out: &mut String,
) {
    let get = |at: String| lookup(values, k, &at);
    let Some(Val::Int(q)) = get(format!("i.{name}.q")) else { return };
    if get(format!("i.{name}.held")) == Some(Val::Bool(true)) {
        return;
    }
    let zero = if matches!(program.types.get(elem), Type::Float { .. }) { Val::F64(0.0) } else { Val::Int(0) };
    let delivered: Vec<Val> =
        (0..len).map(|j| get(format!("i.{name}.d[{j}]")).unwrap_or_else(|| zero.clone())).collect();
    let items: Vec<String> = delivered.iter().map(|d| value_text(program, elem, d)).collect();
    let checked = get(format!("i.{name}.pass")) == Some(Val::Bool(true));
    // Zeigt das Array die Lieferung selbst, liess der Rand sie durch: Ein
    // `Suspect` des Rands haelt das letzte gute, das die Lieferung nicht ist.
    let shown = get(format!("i.{name}.v")) == Some(Val::Bool(true))
        && delivered.iter().enumerate().all(|(j, d)| get(format!("i.{name}[{j}]")).as_ref() == Some(d));
    let line = match q {
        crate::encode::quality::SUSPECT if shown => format!("[{}] suspect", items.join(", ")),
        crate::encode::quality::STALE if checked => format!("[{}] stale", items.join(", ")),
        crate::encode::quality::STALE => "stale".to_string(),
        crate::encode::quality::BAD => format!("[{}] bad", items.join(", ")),
        _ => format!("[{}]", items.join(", ")),
    };
    let _ = writeln!(out, "t={k} in {name} {line}");
}

/// Die Aufzeichnungen der Jobs (4.5) aus den Faelligkeiten des
/// Gegenbeispiels. Eine Aufzeichnung gilt fuer jeden Lauf, dessen Modell-Tick
/// nicht nach ihr liegt (`job_start`): Ein puenktlicher Lauf vor einem
/// verspaeteten steht darum mit seinem eigenen Tick im Stimulus.
fn job_records(values: &BTreeMap<(u32, String), Val>, program: &Program, depth: u32, out: &mut String) {
    let tick = program.config.tick.max(1);
    for m in &program.machines {
        for slot in &m.layout.job_slots {
            let handle = &m.vars[slot.handle.index()].name;
            let at = |part: &str| format!("s.{}.job.{handle}.{part}", m.name);
            let d = program.natives[slot.native.index()].duration.unwrap_or(0).max(0);
            let span = d.saturating_add(tick - 1) / tick;
            // 4.5: Je Lauf sein Start und seine Faelligkeit; ein Lauf, der den
            // Tick des Modells ueberschreitet, steht mit seinem Start in der
            // Aufzeichnung. Wird er vorher abgebrochen, bleibt die Zeile
            // ohne Wirkung.
            let mut prev = -1;
            for k in 0..=depth {
                let (Some(Val::Int(start)), Some(Val::Int(due))) =
                    (values.get(&(k, at("start"))), values.get(&(k, at("due"))))
                else {
                    continue;
                };
                if *start < 0 || *start == prev {
                    continue;
                }
                prev = *start;
                let s = start / tick;
                if due / tick > s + span {
                    let _ = writeln!(out, "t={} job {} {handle} done start={s}", due / tick, m.name);
                }
            }
        }
    }
}

/// Ein Record-Element mit einem Feld variabler Laenge in seiner Byteform
/// (3.7, 5.9), wie der Rand es nimmt: Als `Name(…)` hat ein solches Feld
/// keine Schreibweise. `None` fuer jeden anderen Typ, der lesbar bleibt.
fn record_wire(
    program: &Program,
    ty: takt_mir::TypeId,
    base: &str,
    values: &BTreeMap<(u32, String), Val>,
    k: u32,
) -> Option<String> {
    if !matches!(program.types.get(ty), Type::Record(_)) || !variable_length(program, ty) {
        return None;
    }
    let env: crate::eval::Env = values
        .range((k, base.to_string())..(k + 1, String::new()))
        .filter(|((_, name), _)| name.starts_with(base))
        .map(|((_, name), v)| (name.clone(), v.clone()))
        .collect();
    let v = crate::encode::value_at(program, ty, base, &env)?;
    let bytes = takt_interp::bytes::encode(program, &v, ty).ok()?;
    Some(format!("0x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

/// Hat ein Wert des Typs eine Laenge, die erst er selbst festlegt (Text,
/// Bytes, Vektor, auch in einem Feld)?
fn variable_length(program: &Program, ty: takt_mir::TypeId) -> bool {
    match program.types.get(ty) {
        Type::Bytes { .. } | Type::Str { .. } | Type::Line { .. } | Type::Vec { .. } => true,
        Type::Record(r) => program.records[r.index()].fields.iter().any(|f| variable_length(program, f.ty)),
        Type::Array { elem, .. } | Type::Optional(elem) => variable_length(program, *elem),
        Type::Enum(e) => {
            program.enums[e.index()].variants.iter().flat_map(|v| &v.fields).any(|f| variable_length(program, f.ty))
        }
        _ => false,
    }
}

/// Ein Wert aus den Blaettern des Modells in der Textform des Stimulus
/// (`takt_interp::trace::parse_value`): `Name(a, b)` fuer Records und
/// Varianten mit Feldern, `[a, b]`, `none`, Text in Anfuehrungszeichen,
/// Bytes als `0x…`; ein fehlendes Blatt ist null.
fn element_text(program: &Program, ty: takt_mir::TypeId, base: &str, get: &dyn Fn(&str) -> Option<Val>) -> String {
    let leaf = |at: &str| get(at).unwrap_or(Val::Int(0));
    // Die Bytes als Blaetter oder, bei einer grossen Kapazitaet, als Array
    // (FB-497).
    let bytes = |at: &str| {
        let len = match leaf(&format!("{at}.len")) {
            Val::Int(n) => usize::try_from(n).unwrap_or(0),
            _ => 0,
        };
        let array = get(&format!("{at}[*]"));
        (0..len)
            .map(|i| match &array {
                Some(Val::Arr(a)) => a.get(i as i64),
                _ => leaf(&format!("{at}[{i}]")),
            })
            .map(|b| match b {
                Val::Int(b) => u8::try_from(b).unwrap_or(0),
                _ => 0,
            })
            .collect::<Vec<u8>>()
    };
    match program.types.get(ty) {
        Type::Bytes { .. } => format!("0x{}", bytes(base).iter().map(|b| format!("{b:02x}")).collect::<String>()),
        // Eine gekuerzte Zeile: Der Rand schneidet an der letzten
        // Zeichengrenze bis zur Kapazitaet (`parse_value`), also folgt ein
        // Zeichen, das genau dahinter endet.
        Type::Str { cap } | Type::Line { cap } => {
            let b = bytes(base);
            let mut s = String::from_utf8_lossy(&b).into_owned();
            if get(&format!("{base}.truncated")) == Some(Val::Bool(true)) {
                let filler = match (*cap as usize + 1).saturating_sub(b.len()) {
                    1 => Some('x'),
                    2 => Some('\u{e9}'),
                    3 => Some('\u{20ac}'),
                    4 => Some('\u{1f600}'),
                    _ => None,
                };
                s.extend(filler);
            }
            format!("{s:?}")
        }
        Type::Record(r) => {
            let def = &program.records[r.index()];
            let parts: Vec<String> =
                def.fields.iter().map(|f| element_text(program, f.ty, &format!("{base}.{}", f.name), get)).collect();
            format!("{}({})", def.name, parts.join(", "))
        }
        Type::Array { elem, len } => {
            let parts: Vec<String> =
                (0..*len).map(|i| element_text(program, *elem, &format!("{base}[{i}]"), get)).collect();
            format!("[{}]", parts.join(", "))
        }
        // 8.9: `t;pre;post;rate;[s1, …]`.
        Type::Capture { elem, len } => {
            let head = |part: &str| match leaf(&format!("{base}.{part}")) {
                Val::Int(i) => i.to_string(),
                Val::F64(f) => format!("{f:?}"),
                other => format!("{other:?}"),
            };
            let samples: Vec<String> =
                (0..*len).map(|i| element_text(program, *elem, &format!("{base}.samples[{i}]"), get)).collect();
            format!("{};{};{};{};[{}]", head("t"), head("pre"), head("post"), head("rate"), samples.join(", "))
        }
        Type::Optional(inner) => match get(&format!("{base}.has")) {
            Some(Val::Bool(true)) => element_text(program, *inner, &format!("{base}.value"), get),
            _ => "none".to_string(),
        },
        Type::Enum(e) if program.enums[e.index()].variants.iter().any(|v| !v.fields.is_empty()) => {
            let def = &program.enums[e.index()];
            let tag = match leaf(&format!("{base}.tag")) {
                Val::Int(i) => usize::try_from(i).unwrap_or(0),
                _ => 0,
            };
            let Some(v) = def.variants.get(tag) else { return tag.to_string() };
            if v.fields.is_empty() {
                return v.name.clone();
            }
            let parts: Vec<String> = v
                .fields
                .iter()
                .map(|f| element_text(program, f.ty, &format!("{base}.{}.{}", v.name, f.name), get))
                .collect();
            format!("{}({})", v.name, parts.join(", "))
        }
        _ => {
            let zero = match program.types.get(ty) {
                Type::Bool => Val::Bool(false),
                Type::Float { width: takt_mir::types::FloatWidth::F32, .. } => Val::F32(0.0),
                Type::Float { .. } => Val::F64(0.0),
                _ => Val::Int(0),
            };
            value_text(program, ty, &get(base).unwrap_or(zero))
        }
    }
}

/// Ein Wert in der Textform des Stimulus (12.5).
fn value_text(program: &Program, ty: takt_mir::TypeId, v: &Val) -> String {
    match (program.types.get(ty), v) {
        (Type::Bool, Val::Bool(b)) => b.to_string(),
        (Type::Enum(e), Val::Int(i)) => {
            program.enums[e.index()].variants.get(*i as usize).map(|v| v.name.clone()).unwrap_or_else(|| i.to_string())
        }
        (Type::Duration { .. }, Val::Int(i)) => format!("{i} ns"),
        (Type::Int { width: IntWidth::U64, .. }, Val::Int(i)) => (*i as u64).to_string(),
        (_, Val::Int(i)) => i.to_string(),
        (_, Val::F64(f)) => format!("{f:?}"),
        (_, Val::F32(f)) => format!("{f:?}"),
        (_, Val::Bool(b)) => b.to_string(),
        // Ein Array ist kein Wert eines Inputs; die Elemente schreibt `element_text`.
        (_, Val::Arr(a)) => format!("{a:?}"),
    }
}

/// Spielt das Gegenbeispiel im Interpreter nach: die Verletzungsposition,
/// wenn die Eigenschaft dort tatsaechlich verletzt ist.
fn confirm(program: &Program, name: &str, stimulus: &str, depth: u32) -> Option<u64> {
    let trace = Trace::parse(stimulus).ok()?;
    let r = run(program, &trace, &RunOptions { ticks: u64::from(depth), ..Default::default() }).ok()?;
    match r.properties.iter().find(|p| p.name == name)?.outcome {
        Outcome::Violated { at, .. } => Some(at),
        _ => None,
    }
}

/// Der Name einer Eingabe im Export, fuer Tests.
pub fn input_name(name: &str, step: u32) -> String {
    at(name, step, "@")
}
