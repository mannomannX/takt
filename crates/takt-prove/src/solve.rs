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
use crate::smt::{Query, Target, assumptions_of, at, contract_query, horn, houdini, query};
use crate::term::Term;

/// Der gefundene Solver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Solver {
    /// Sein Pfad.
    At(PathBuf),
    /// Keiner gefunden.
    Missing,
}

/// Sucht `TAKT_SOLVER`, dann `z3` und `cvc5`, je auf dem PATH oder unter
/// `~/.takt/bin`.
pub fn find() -> Solver {
    let given = std::env::var("TAKT_SOLVER").ok().map(PathBuf::from).filter(|p| answers(p));
    match given {
        Some(p) => Solver::At(p),
        None => ["z3", "cvc5"].into_iter().map(named).find(Solver::works).unwrap_or(Solver::Missing),
    }
}

/// Der Solver `name` (`z3`, `cvc5`) auf dem PATH oder unter `~/.takt/bin`:
/// Wer beide vergleicht, waehlt jeden selbst (M11 Schritt 28c).
pub fn named(name: &str) -> Solver {
    let mut candidates = vec![PathBuf::from(name)];
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        candidates.push(PathBuf::from(home).join(".takt").join("bin").join(name));
    }
    candidates.into_iter().find(|p| answers(p)).map_or(Solver::Missing, Solver::At)
}

/// Antwortet das Werkzeug unter `path` auf `--version`?
fn answers(path: &Path) -> bool {
    Command::new(path).arg("--version").output().is_ok_and(|o| o.status.success())
}

impl Solver {
    /// Der Pfad, wenn gefunden.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Solver::At(p) => Some(p),
            Solver::Missing => None,
        }
    }

    /// Antwortet der Solver auf `--version`?
    pub fn works(&self) -> bool {
        self.path().is_some_and(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
    }

    /// Name und Version, wie Bericht und Beweisdatei sie fuehren (FB-380):
    /// `z3 4.13.4`, `cvc5 1.2.0`. Eine andere Version kann anders urteilen.
    pub fn identity(&self) -> Option<String> {
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

    /// Fuehrt ein Skript aus und liefert die Ausgabe.
    fn run(&self, script: &str, timeout_s: u64, tag: &str) -> Result<String, String> {
        let path = self.path().ok_or("kein Solver")?;
        let dir = std::env::temp_dir().join(format!("takt-prove-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Eindeutig je Aufruf: Tests laufen nebenlaeufig im selben Prozess.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = dir.join(format!("{tag}-{n}.smt2"));
        std::fs::write(&file, script).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(path);
        if self.is_cvc5() {
            // Die Anfragen arbeiten mit `push`/`pop`; cvc5 lehnt das ohne
            // `--incremental` ab und entschied so nichts (FB-492).
            let limit = format!("--tlimit={}", timeout_s * 1000);
            cmd.args(["--lang=smt2", "--incremental", "--produce-models", &limit]);
        } else {
            cmd.args(["-smt2", &format!("-T:{timeout_s}")]);
        }
        let out = cmd.arg(&file).output().map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&file);
        // Leer erst nach dem letzten nebenlaeufigen Aufruf; bis dahin scheitert es still.
        let _ = std::fs::remove_dir(&dir);
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        if text.trim().is_empty() {
            return Err(format!("{}: keine Ausgabe\n{}", path.display(), String::from_utf8_lossy(&out.stderr)));
        }
        Ok(text)
    }
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
            Ok(m) => strengthened(&m, solver, timeout_s)?,
            Err(e) => {
                notes.push(format!("`{}` nicht kodierbar: {}", machine.name, e.what));
                if let Some(w) = whole {
                    for (j, site) in w.checks.iter().enumerate().filter(|(_, s)| s.machine == machine.name) {
                        out.push(report(site, classify_site(w, program, j, depth, solver, timeout_s)?));
                    }
                }
                continue;
            }
        };
        notes.extend(model.notes.iter().cloned());
        for (j, site) in model.checks.iter().enumerate() {
            let bmc = solver.run(&query(&model, depth, Target::Check(j), Query::Bmc), timeout_s, "check-bmc")?;
            let verdict = match answer(&bmc) {
                "unsat" => induction_verdict(&model, j, depth, solver, timeout_s)?,
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
                        Some(true) => CheckVerdict::Unreachable { k: 0 },
                        found => CheckVerdict::Undecided {
                            reason: format!("kein Pfad bis Tiefe {depth}, {}", open(model, depth, found)),
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
        "unsat" => induction_verdict(model, i, depth, solver, timeout_s)?,
        other => CheckVerdict::Undecided { reason: format!("BMC: Solver sagt `{other}`") },
    })
}

/// Die Invariantensuche nach einem offenen Induktionsschritt (FB-375):
/// `Some(true)`, wenn z3 eine induktive Invariante findet, `Some(false)`,
/// wenn es einen Pfad ueber jede Tiefe hinaus gibt, `None` ohne Urteil —
/// auch ausserhalb des ganzzahligen Fragments und mit cvc5, das keine
/// Horn-Klauseln liest.
fn invariant(model: &Model, target: Target, solver: &Solver, timeout_s: u64) -> Result<Option<bool>, String> {
    let Some(q) = horn(model, target).filter(|_| !solver.is_cvc5()) else { return Ok(None) };
    let text = solver.run(&q, timeout_s, "horn")?;
    Ok(match answer(&text) {
        "sat" => Some(true),
        "unsat" => Some(false),
        _ => None,
    })
}

/// Warum ein Induktionsschritt offen bleibt, und was er braucht.
fn open(model: &Model, depth: u32, found: Option<bool>) -> String {
    if found == Some(false) {
        return format!(
            "die Invariantensuche findet einen Pfad ueber Tiefe {depth} hinaus; eine groessere Tiefe zeigt ihn"
        );
    }
    let deadline = if depth > model.horizon {
        String::new()
    } else {
        format!("; die laengste Frist braucht k = {} (`--depth auto`)", model.horizon + 1)
    };
    format!("Induktionsschritt offen (k = {depth}){deadline}")
}

fn induction_verdict(
    model: &Model,
    i: usize,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<CheckVerdict, String> {
    let ind = solver.run(&query(model, depth, Target::Check(i), Query::Induction), timeout_s, "check-ind")?;
    Ok(match answer(&ind) {
        "unsat" => CheckVerdict::Unreachable { k: depth },
        "sat" => match invariant(model, Target::Check(i), solver, timeout_s)? {
            Some(true) => CheckVerdict::Unreachable { k: 0 },
            found => CheckVerdict::Undecided {
                reason: format!("kein Pfad bis Tiefe {depth}, {}", open(model, depth, found)),
            },
        },
        other => CheckVerdict::Undecided { reason: format!("Induktionsschritt: Solver sagt `{other}`") },
    })
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
                        values.get(n).map(|v| format!("{} = {}", n.trim_start_matches("c."), val_text(*v)))
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

fn val_text(v: Val) -> String {
    match v {
        Val::Bool(b) => b.to_string(),
        Val::Int(i) => i.to_string(),
        Val::F32(f) => format!("{f:?}"),
        Val::F64(f) => format!("{f:?}"),
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
    let states = model.simulate(u64::from(depth), &|k, n| values.get(&(k as u32, n.to_string())).copied());
    let mut env: crate::eval::Env = BTreeMap::new();
    for (name, _) in &model.inputs {
        if let Some(v) = values.get(&(0, name.clone())) {
            env.insert(name.clone(), *v);
        }
    }
    if crate::eval::eval(&site.init, &env) == Val::Bool(true) {
        return Some(0);
    }
    for k in 0..depth {
        let mut env = states[k as usize].clone();
        for (name, _) in &model.inputs {
            if let Some(v) = values.get(&(k + 1, name.clone())) {
                env.insert(name.clone(), *v);
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
                        Some(true) => Verdict::Proven { k: 0, assumptions: assumptions_of(model, Target::Property(i)) },
                        found => Verdict::Unproven {
                            reason: format!("kein Gegenbeispiel bis Tiefe {depth}, {}", open(model, depth, found)),
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
            let delivered = values.get(&(k, format!("i.{}.d", c.name))).copied().unwrap_or(*v);
            let line = match q {
                quality::SUSPECT | quality::STALE if passed => {
                    (*last, *since) = (Some(delivered), -1);
                    let flag = if q == quality::SUSPECT { "suspect" } else { "stale" };
                    format!("{} {flag}", value_text(program, c.ty, &delivered))
                }
                quality::SUSPECT => {
                    let edge = edges.iter().find(|e| e.name == c.name);
                    let wrong = edge.and_then(|e| e.violation(last.as_ref(), *since)).unwrap_or(*v);
                    value_text(program, c.ty, &wrong)
                }
                quality::STALE => "stale".to_string(),
                quality::BAD => {
                    *last = None;
                    format!("{} bad", value_text(program, c.ty, v))
                }
                _ => {
                    (*last, *since) = (Some(*v), -1);
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
                    element_text(program, *elem, &format!("{base}.v"), &|at| values.get(&(k, at.to_string())).copied())
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
    let get = |at: &str| values.get(&(k, at.to_string())).copied();
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
    let get = |at: String| values.get(&(k, at)).copied();
    let Some(Val::Int(q)) = get(format!("i.{name}.q")) else { return };
    if get(format!("i.{name}.held")) == Some(Val::Bool(true)) {
        return;
    }
    let zero = if matches!(program.types.get(elem), Type::Float { .. }) { Val::F64(0.0) } else { Val::Int(0) };
    let delivered: Vec<Val> = (0..len).map(|j| get(format!("i.{name}.d[{j}]")).unwrap_or(zero)).collect();
    let items: Vec<String> = delivered.iter().map(|d| value_text(program, elem, d)).collect();
    let checked = get(format!("i.{name}.pass")) == Some(Val::Bool(true));
    // Zeigt das Array die Lieferung selbst, liess der Rand sie durch: Ein
    // `Suspect` des Rands haelt das letzte gute, das die Lieferung nicht ist.
    let shown = get(format!("i.{name}.v")) == Some(Val::Bool(true))
        && delivered.iter().enumerate().all(|(j, d)| get(format!("i.{name}[{j}]")) == Some(*d));
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
        .map(|((_, name), v)| (name.clone(), *v))
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
    let bytes = |at: &str| {
        let len = match leaf(&format!("{at}.len")) {
            Val::Int(n) => usize::try_from(n).unwrap_or(0),
            _ => 0,
        };
        (0..len)
            .map(|i| match leaf(&format!("{at}[{i}]")) {
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
