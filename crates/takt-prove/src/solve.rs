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
use takt_mir::types::Type;

use crate::encode::Model;
use crate::eval::Val;
use crate::smt::{Query, Target, at, contract_query, query};

/// Der gefundene Solver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Solver {
    /// Sein Pfad.
    At(PathBuf),
    /// Keiner gefunden.
    Missing,
}

/// Sucht `TAKT_SOLVER`, dann `z3` und `cvc5` auf dem PATH.
pub fn find() -> Solver {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("TAKT_SOLVER") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(PathBuf::from("z3"));
    candidates.push(PathBuf::from("cvc5"));
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let bin = PathBuf::from(home).join(".takt").join("bin");
        candidates.extend(["z3", "cvc5"].map(|n| bin.join(n)));
    }
    for path in candidates {
        if Command::new(&path).arg("--version").output().is_ok_and(|o| o.status.success()) {
            return Solver::At(path);
        }
    }
    Solver::Missing
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
            cmd.args(["--lang=smt2", "--produce-models", &format!("--tlimit={}", timeout_s * 1000)]);
        } else {
            cmd.args(["-smt2", &format!("-T:{timeout_s}")]);
        }
        let out = cmd.arg(&file).output().map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&file);
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
    /// k-Induktion.
    Proven {
        /// Schritte der Induktion.
        k: u32,
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

impl Verdict {
    /// Der Text fuer den Bericht.
    pub fn text(&self) -> String {
        match self {
            Verdict::Proven { k } => format!("bewiesen (k-Induktion, k = {k})"),
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
        /// Schritte der Induktion.
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
            CheckVerdict::Unreachable { k } => format!("bewiesen unerreichbar (k-Induktion, k = {k})"),
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
    let mut out = Vec::new();
    for (i, site) in model.checks.iter().enumerate() {
        let bmc = solver.run(&query(model, depth, Target::Check(i), Query::Bmc), timeout_s, "check-bmc")?;
        let verdict = match answer(&bmc) {
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
                            "der Solver fand einen Pfad bis Tiefe {depth}, der Interpreter bestaetigt ihn nicht — die Kodierung weicht ab, bitte melden:\n{stimulus}"
                        ),
                    },
                }
            }
            "unsat" => {
                let ind =
                    solver.run(&query(model, depth, Target::Check(i), Query::Induction), timeout_s, "check-ind")?;
                match answer(&ind) {
                    "unsat" => CheckVerdict::Unreachable { k: depth },
                    "sat" => CheckVerdict::Undecided {
                        reason: format!("kein Pfad bis Tiefe {depth}, Induktionsschritt offen (k = {depth})"),
                    },
                    other => CheckVerdict::Undecided { reason: format!("Induktionsschritt: Solver sagt `{other}`") },
                }
            }
            other => CheckVerdict::Undecided { reason: format!("BMC: Solver sagt `{other}`") },
        };
        out.push(CheckReport {
            start: site.start,
            span: site.span,
            machine: site.machine.clone(),
            kind: site.kind.clone(),
            verdict,
        });
    }
    Ok(out)
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

/// Spielt den Pfad nach: der Tick, in dem die Stelle im Interpreter feuert.
fn confirm_check(program: &Program, site: &crate::encode::CheckSite, stimulus: &str, depth: u32) -> Option<u64> {
    let trace = Trace::parse(stimulus).ok()?;
    let r = run(program, &trace, &RunOptions { ticks: u64::from(depth), ..Default::default() }).ok()?;
    // Eine implizite Pruefung (11.3) bestaetigt der Fault ihrer Art.
    let fault_kind = match site.kind.as_str() {
        "check" => "CheckFailed",
        "expect" => "Expect",
        "range" => "RangeFault",
        "div" => "Arithmetic(DivZero)",
        "ovf" => "Arithmetic(Overflow)",
        "fin" => "Arithmetic(NonFinite)",
        "dom" => "Arithmetic(Domain)",
        _ => return None,
    };
    if matches!(site.kind.as_str(), "check" | "expect") {
        let name = format!("{} @{}", site.kind, site.start);
        let fired = r.coverage.hits.get(&(takt_interp::CoverKind::CheckFailed, site.machine.clone(), name)).copied();
        if fired.unwrap_or(0) == 0 {
            return None;
        }
    }
    r.trace
        .lines
        .iter()
        .find(|l| matches!(&l.kind, takt_interp::trace::LineKind::Fault { machine, kind, .. } if *machine == site.machine && kind == fault_kind))
        .map(|l| l.tick)
}

/// Prueft jede Eigenschaft des Modells.
pub fn prove(
    model: &Model,
    program: &Program,
    depth: u32,
    solver: &Solver,
    timeout_s: u64,
) -> Result<Vec<Report>, String> {
    let mut out = Vec::new();
    for (i, prop) in model.properties.iter().enumerate() {
        let bmc = solver.run(&query(model, depth, Target::Property(i), Query::Bmc), timeout_s, "bmc")?;
        let verdict = match answer(&bmc) {
            "sat" => {
                let values = parse_values(&bmc, "@");
                let stimulus = stimulus(&values, program, depth);
                match confirm(program, &prop.name, &stimulus, depth) {
                    Some(at) => Verdict::Violated { at, stimulus },
                    None => Verdict::Unproven {
                        reason: format!(
                            "der Solver fand ein Gegenbeispiel bis Tiefe {depth}, der Interpreter bestaetigt es nicht — die Kodierung weicht ab, bitte melden:\n{stimulus}"
                        ),
                    },
                }
            }
            "unsat" => {
                let ind = solver.run(&query(model, depth, Target::Property(i), Query::Induction), timeout_s, "ind")?;
                match answer(&ind) {
                    "unsat" => Verdict::Proven { k: depth },
                    "sat" => Verdict::Unproven {
                        reason: format!("kein Gegenbeispiel bis Tiefe {depth}, Induktionsschritt offen (k = {depth})"),
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

/// Das Gegenbeispiel als Stimulus (12.5): Inputs und Commands je Tick.
pub fn stimulus(values: &BTreeMap<(u32, String), Val>, program: &Program, depth: u32) -> String {
    let mut out = String::new();
    for k in 0..=depth {
        for c in program.channels.iter().filter(|c| c.dir == Direction::Input) {
            let Some(v) = values.get(&(k, format!("i.{}", c.name))) else { continue };
            let text = match (program.types.get(c.ty), v) {
                (Type::Bool, Val::Bool(b)) => b.to_string(),
                (Type::Enum(e), Val::Int(i)) => program.enums[e.index()]
                    .variants
                    .get(*i as usize)
                    .map(|v| v.name.clone())
                    .unwrap_or_else(|| i.to_string()),
                (Type::Duration { .. }, Val::Int(i)) => format!("{i} ns"),
                (_, Val::Int(i)) => i.to_string(),
                (_, Val::F64(f)) => format!("{f:?}"),
                (_, Val::F32(f)) => format!("{f:?}"),
                (_, Val::Bool(b)) => b.to_string(),
            };
            let _ = writeln!(out, "t={k} in {} {text}", c.name);
        }
        for c in &program.commands {
            if values.get(&(k, format!("i.cmd.{}", c.name))) == Some(&Val::Bool(true)) {
                let _ = writeln!(out, "t={k} cmd {}", c.name);
            }
        }
    }
    out
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
