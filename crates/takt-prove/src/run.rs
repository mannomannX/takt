//! Das Modell als dritter Ausfuehrer (M11 Schritt 28, plan/m11.md 2.13):
//! [`Model::run`] rechnet einen Stimulus Tick fuer Tick mit konkreten Werten
//! und schreibt seinen Trace in der Form des Interpreters, damit derselbe
//! Vergleich ueber alle drei Ausfuehrer urteilt.
//!
//! Was der Rand aus einer Lieferung macht, entscheidet eine Implementierung
//! (`takt-hal`, 12.6), und das Modell sieht seine Urteile als freie
//! Eingaben. Die Abtastungen der skalaren Inputs und die uebernommenen
//! Tunables nimmt der Lauf darum aus dem Lauf des Interpreters
//! (`RunOptions::inputs`); die Annahmen des Modells pruefen sie in jedem
//! Tick. Ein Rand, der anders urteilt, als das Modell zusichert, beendet den
//! Lauf mit [`Stop::Violated`].

use std::collections::{BTreeMap, HashMap};

use takt_interp::run::element_value;
use takt_interp::trace::{LineKind, TraceLine, parse_value, render_line, value_text};
use takt_interp::value::{Quality, Seen, Value};
use takt_interp::{RunResult, Trace};
use takt_mir::machine::FaultKind;
use takt_mir::program::{Channel, Direction};
use takt_mir::types::Type;
use takt_mir::{MachineId, Program};

use crate::encode::{Model, Position, fault_kind, leaves_at, quality, value_at};
use crate::eval::{self, Env, Plan, Val};
use crate::term::{Sort, Term};

/// Warum ein Lauf des Modells endet, bevor er seine Ticks gerechnet hat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Der Stimulus oder ein Wert hat etwas, das das Modell nicht darstellt:
    /// eine Luecke mit Grund, kein Ueberspringen.
    Gap {
        /// Der Tick.
        tick: u64,
        /// Was fehlt.
        what: String,
    },
    /// Eine Invariante oder Annahme des Modells haelt im Lauf nicht: Es
    /// schloesse einen Lauf aus, den es gibt, und ein Beweis daraus waere
    /// keiner.
    Violated {
        /// Der Tick.
        tick: u64,
        /// Welche.
        what: String,
    },
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stop::Gap { tick, what } => write!(f, "t={tick}: Luecke des Modells: {what}"),
            Stop::Violated { tick, what } => write!(f, "t={tick}: {what} verletzt"),
        }
    }
}

/// Die Zeilen, die der Trace des Modells nicht traegt, je mit Grund: Ein
/// Vergleich mit dem Modell laesst sie auf der anderen Seite weg.
pub const UNMODELLED: &[(&str, &str)] = &[
    ("alert", "eine Flanke je Stelle und Schleifendurchlauf (5.6); kein Beweisziel liest sie"),
    ("log", "ein Text ohne Zustand"),
    ("measure", "ein Messwert ohne Zustand (13.5)"),
    ("verify", "das Urteil eines Szenarios (13.5)"),
    ("verdict", "das Urteil eines Szenarios (13.5)"),
    // Eine Verletzung schreibt das Modell, sobald sie entschieden ist, und
    // `mismatches` haelt sie gegen den Interpreter. Was erst das Laufende
    // entscheidet (`Monitor::close`), entscheidet das Modell nicht; der
    // erzeugte Code schreibt auch das.
    ("property", "was erst das Laufende entscheidet, entscheidet das Modell nicht"),
    ("assumption", "wie `property`"),
    // 4.5: Die Fertigstellung eines Jobs ist eine Eingabe des Modells; es
    // nimmt sie aus dem Lauf des Interpreters.
    ("job", "eine Eingabe des Modells, aus dem Lauf des Interpreters"),
];

impl Model {
    /// Rechnet `stimulus` ueber `ticks` Ticks und schreibt den Trace des
    /// Modells: Outputs, Zustaende, `pub var`, Signale, Faults, was die
    /// Ausgabestroeme senden, die Zaehler der Stroeme und das Ende eines
    /// Laufs (12.7), je bei Aenderung wie der Interpreter. `seen` ist der
    /// Lauf des Interpreters ueber denselben Stimulus mit
    /// `RunOptions::inputs`.
    pub fn run(&self, p: &Program, stimulus: &Trace, seen: &RunResult, ticks: u64) -> Result<Trace, Stop> {
        if seen.inputs.is_empty() {
            return Err(Stop::Gap { tick: 0, what: "der Lauf des Interpreters ohne `RunOptions::inputs`".into() });
        }
        let inputs = inputs(self, p, stimulus, seen, ticks)?;
        let mut writer = Writer::new(self, p);
        let mut runner = Runner::new(self, |name| writer.reads(name));
        let mut state: Option<Vec<Val>> = None;
        for k in 0..=ticks {
            let next = runner.step(self, k, state.as_deref(), &inputs)?;
            let shown = runner.observed(self, &next);
            writer.tick(k, &shown)?;
            writer.goals(k, self, &runner.held, &shown)?;
            // 12.7: Die Zeile `end` markiert die Entscheidung; die Outputs auf
            // `safe` stehen im Zustand danach und im Trace im selben Tick.
            if shown.get(ENDED) == Some(&Val::Bool(true)) {
                writer.end(k, &shown)?;
                let (after, _) = runner.advance(self, k + 1, Some(&next), &inputs)?;
                writer.changes(k, &runner.observed(self, &after))?;
                break;
            }
            // 13.6: Das Szenario ist am Ende, wenn es in `FAULTED` steht oder
            // in einer Kette ohne Uebergang; ab Tick 1, wie im Interpreter.
            if let Some((name, done)) = &self.scenario
                && k > 0
            {
                let faulted = shown.get(&format!("s.{name}.faulted")) == Some(&Val::Bool(true));
                let leaf = int(&shown, &format!("s.{name}.leaf"), k)?;
                if faulted || done.contains(&leaf) {
                    writer.push(k, LineKind::End { reason: "scenario".into() });
                    break;
                }
            }
            state = Some(next);
        }
        Ok(Trace { lines: writer.lines, ..Trace::default() })
    }
}

/// Die Terme eines Ticks, einmal uebersetzt, und woher ihre Variablen ihre
/// Werte nehmen: der Folgezustand ueber dem Zustand davor und den Eingaben,
/// die Invarianten ueber dem Zustand danach, die Annahmen ueber ihm und den
/// Eingaben. Der Zustand ist ein Vektor in der Folge von `Model::state`.
struct Runner {
    next: (Plan, Vec<Source>),
    invariants: (Plan, Vec<Source>),
    assumptions: (Plan, Vec<Source>),
    /// Die Ziele der Eigenschaften ueber dem Zustand danach und den Eingaben.
    goals: (Plan, Vec<Source>),
    /// Je Ziel, ob es im letzten Tick galt.
    held: Vec<bool>,
    /// Die Plaetze des Zustands, die der Trace liest.
    observed: Vec<usize>,
}

/// Woher eine Variable eines Plans ihren Wert nimmt; eine unbekannte ist
/// null wie in `eval::eval`.
#[derive(Clone, Copy)]
enum Source {
    State(usize),
    Input(usize),
    Zero(Sort),
}

impl Runner {
    fn new(model: &Model, reads: impl Fn(&str) -> bool) -> Runner {
        let state: HashMap<&str, usize> = model.state.iter().enumerate().map(|(i, v)| (v.name.as_str(), i)).collect();
        let input: HashMap<&str, usize> = model.inputs.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
        let plan = |terms: Vec<&Term>, with_inputs: bool| {
            let plan = Plan::new(terms);
            let sources = plan
                .vars()
                .iter()
                .map(|(n, sort)| match (state.get(n.as_str()), input.get(n.as_str())) {
                    (Some(&i), _) => Source::State(i),
                    (None, Some(&j)) if with_inputs => Source::Input(j),
                    _ => Source::Zero(*sort),
                })
                .collect();
            (plan, sources)
        };
        Runner {
            next: plan(model.state.iter().map(|v| &v.next).collect(), true),
            invariants: plan(model.invariants.iter().collect(), false),
            assumptions: plan(model.assumptions.iter().collect(), true),
            goals: plan(model.properties.iter().map(|g| &g.formula).collect(), true),
            held: vec![true; model.properties.len()],
            observed: (0..model.state.len()).filter(|&i| reads(&model.state[i].name)).collect(),
        }
    }

    /// Ein Tick: der Zustand danach. Jede Invariante gilt in ihm, und die
    /// Annahmen gelten ueber ihm und den Eingaben des Ticks.
    fn step(
        &mut self,
        model: &Model,
        k: u64,
        pre: Option<&[Val]>,
        inputs: &BTreeMap<(u64, String), Val>,
    ) -> Result<Vec<Val>, Stop> {
        let (next, given) = self.advance(model, k, pre, inputs)?;
        let holds = self.invariants.0.eval(&gather(&self.invariants.1, &next, &given));
        if let Some(i) = holds.iter().position(|v| *v != Val::Bool(true)) {
            return Err(Stop::Violated { tick: k, what: format!("Invariante {i}") });
        }
        let holds = self.assumptions.0.eval(&gather(&self.assumptions.1, &next, &given));
        if let Some(i) = holds.iter().position(|v| *v != Val::Bool(true)) {
            return Err(Stop::Violated { tick: k, what: format!("Annahme {i}") });
        }
        let goals = self.goals.0.eval(&gather(&self.goals.1, &next, &given));
        self.held = goals.iter().map(|v| *v == Val::Bool(true)).collect();
        Ok(next)
    }

    /// Der Zustand nach dem Tick `k` und die Eingaben, aus denen er folgt,
    /// ohne Pruefung: Nach dem Ende eines Laufs (12.7) laeuft kein Tick
    /// mehr, fuer den ein Rand etwas zusicherte.
    fn advance(
        &mut self,
        model: &Model,
        k: u64,
        pre: Option<&[Val]>,
        inputs: &BTreeMap<(u64, String), Val>,
    ) -> Result<(Vec<Val>, Vec<Val>), Stop> {
        let now: HashMap<&str, Val> =
            inputs.range((k, String::new())..(k + 1, String::new())).map(|((_, n), v)| (n.as_str(), *v)).collect();
        let mut given = Vec::with_capacity(model.inputs.len());
        for (n, sort) in &model.inputs {
            let v = now.get(n.as_str()).copied().unwrap_or(Val::zero(*sort));
            if sort_of(v) != *sort {
                return Err(Stop::Gap {
                    tick: k,
                    what: format!("die Eingabe `{n}` hat die Sorte {sort:?}, nicht {v:?}"),
                });
            }
            given.push(v);
        }
        let next = match pre {
            None => {
                let env: Env = model.inputs.iter().map(|(n, _)| n.clone()).zip(given.iter().copied()).collect();
                eval::eval_all(model.state.iter().map(|v| &v.init), &env)
            }
            Some(pre) => self.next.0.eval(&gather(&self.next.1, pre, &given)),
        };
        Ok((next, given))
    }

    /// Die Orte des Zustands, die der Trace liest, mit ihrem Wert.
    fn observed(&self, model: &Model, state: &[Val]) -> Env {
        self.observed.iter().map(|&i| (model.state[i].name.clone(), state[i])).collect()
    }
}

/// Die Werte der Variablen eines Plans.
fn gather(sources: &[Source], state: &[Val], given: &[Val]) -> Vec<Val> {
    sources
        .iter()
        .map(|s| match *s {
            Source::State(i) => state[i],
            Source::Input(j) => given[j],
            Source::Zero(sort) => Val::zero(sort),
        })
        .collect()
}

/// Der Ort, an dem das Modell das Ende eines Laufs fuehrt (12.7).
const ENDED: &str = "s.run.ended";

/// Wo der Trace des Interpreters und der des Modells auseinandergehen: je
/// Tick die Zeilen der Arten, die das Modell schreibt, ein Fault mit Maschine
/// und Art. Strenger als der Vergleich mit dem erzeugten Code — Zustaende
/// und `pub var` schreibt der nicht, und die Werte stehen hier in derselben
/// Textform (`value_text`), also Zeichen fuer Zeichen. Eine Verletzung, die
/// der Interpreter erst im letzten Tick meldet, kann das Laufende
/// entschieden haben (`Monitor::close`); das Modell entscheidet sie nicht,
/// sie zaehlt nur, wenn es sie auch meldet.
pub fn mismatches(interpreter: &Trace, model: &Trace) -> Vec<String> {
    let by_tick = |t: &Trace| {
        let mut out: BTreeMap<u64, Vec<String>> = BTreeMap::new();
        for l in &t.lines {
            let text = match &l.kind {
                LineKind::Fault { machine, kind, .. } => format!("t={} fault {machine} {kind}", l.tick),
                LineKind::Output { .. }
                | LineKind::State { .. }
                | LineKind::Published { .. }
                | LineKind::Signal { .. }
                | LineKind::Stream { .. }
                | LineKind::Property { .. }
                | LineKind::End { .. } => render_line(l),
                _ => continue,
            };
            out.entry(l.tick).or_default().push(text);
        }
        out.values_mut().for_each(|v| v.sort());
        out
    };
    let (mut a, b) = (by_tick(interpreter), by_tick(model));
    if let Some(last) = interpreter.lines.iter().map(|l| l.tick).max()
        && let Some(lines) = a.get_mut(&last)
    {
        let theirs = b.get(&last).cloned().unwrap_or_default();
        let closing = |l: &String| l.contains(" violated ") && !theirs.contains(l);
        lines.retain(|l| !closing(l));
    }
    let ticks: std::collections::BTreeSet<u64> = a.keys().chain(b.keys()).copied().collect();
    let mut out = Vec::new();
    for t in ticks {
        let (x, y) = (a.get(&t).cloned().unwrap_or_default(), b.get(&t).cloned().unwrap_or_default());
        let only = |p: &[String], q: &[String]| p.iter().filter(|l| !q.contains(l)).cloned().collect::<Vec<_>>();
        let (left, right) = (only(&x, &y), only(&y, &x));
        if !left.is_empty() || !right.is_empty() {
            out.push(format!("Interpreter {left:?}, Modell {right:?}"));
        }
    }
    out
}

fn sort_of(v: Val) -> Sort {
    match v {
        Val::Bool(_) => Sort::Bool,
        Val::Int(_) => Sort::Int,
        Val::F32(_) => Sort::F32,
        Val::F64(_) => Sort::F64,
    }
}

/// Die Eingaben des Modells je Tick und Name.
///
/// Commands gelten im Tick ihrer Zeile, Stromelemente stehen je Tick in
/// ihrer Reihenfolge (8.6) — ohne eigenen Zeitstempel mit der Tickgrenze,
/// was der Rand nicht dekodiert als `bad`. Die skalaren Inputs sind die
/// Abtastungen des Interpreters, die Tunables die Werte, die er uebernahm
/// (8.4), und die Verspaetung eines Jobs rechnet sich aus den Zeilen `job`
/// (4.5).
fn inputs(
    model: &Model,
    p: &Program,
    stimulus: &Trace,
    seen: &RunResult,
    ticks: u64,
) -> Result<BTreeMap<(u64, String), Val>, Stop> {
    let mut out = BTreeMap::new();
    let mut tunes = BTreeMap::new();
    for (name, text) in &seen.start_params {
        if let Some(v) = tune(p, name, text, 0)? {
            tunes.insert(name.clone(), v);
        }
    }
    for k in 0..=ticks {
        let mut delivered: BTreeMap<&str, i64> = BTreeMap::new();
        for l in stimulus.at(k) {
            match &l.kind {
                LineKind::Command { name } => {
                    out.insert((k, format!("i.cmd.{name}")), Val::Bool(true));
                }
                LineKind::Abort => {
                    out.insert((k, crate::encode::OPERATOR_ABORT.to_string()), Val::Bool(true));
                }
                // 7.3: wie der Interpreter (`pend_runtime`); `Driver` nennt den Output.
                LineKind::Runtime { kind, output } => {
                    let found = crate::encode::RUNTIME_KINDS.iter().find(|r| format!("{r:?}") == *kind);
                    let Some(&r) = found else { return gap(k, format!("`runtime {kind}`")) };
                    let output = output.as_deref().filter(|_| r == takt_mir::machine::RuntimeKind::Driver);
                    out.insert((k, crate::encode::runtime_input(r, output)), Val::Bool(true));
                }
                LineKind::Input { channel, sample } => {
                    let c = p.channels.iter().find(|c| c.name == *channel && c.dir == Direction::Input);
                    let Some(c) = c else { return gap(k, format!("`in {channel}`: kein Input des Programms")) };
                    // Die Abtastung eines skalaren Inputs kommt vom Rand des Interpreters.
                    let Type::Stream(elem) = p.types.get(c.ty) else { continue };
                    if sample.quality.is_some()
                        || sample.reason.is_some()
                        || sample.age.is_some()
                        || sample.seq.is_some()
                    {
                        return gap(k, format!("`in {channel}`: Qualitaet, Alter und Folgenummer eines Stromelements"));
                    }
                    // Was das Modell nicht vom Rand nimmt, ginge sonst still verloren.
                    let count = format!("i.stream.{channel}.n");
                    if !model.inputs.iter().any(|(n, _)| *n == count) {
                        return gap(k, format!("`in {channel}`: ein Element, das das Modell nicht vom Rand nimmt"));
                    }
                    let j = delivered.entry(channel.as_str()).or_insert(0);
                    let base = format!("i.stream.{channel}.{j}");
                    let boundary = i64::try_from(k).unwrap_or(i64::MAX).saturating_mul(p.config.tick);
                    out.insert((k, format!("{base}.t")), Val::Int(sample.t.unwrap_or(boundary)));
                    let text = sample.value.as_deref().unwrap_or_default();
                    match element_value(text, *elem, p) {
                        Ok(Some(v)) => {
                            let leaves = leaves_at(p, *elem, &v, &format!("{base}.v"));
                            let Some(leaves) = leaves else { return gap(k, format!("`in {channel}`: `{text}`")) };
                            out.extend(leaves.into_iter().map(|(n, v)| ((k, n), v)));
                        }
                        Ok(None) => {
                            out.insert((k, format!("{base}.bad")), Val::Bool(true));
                        }
                        Err(e) => return gap(k, format!("`in {channel}`: {e}")),
                    }
                    *j += 1;
                }
                LineKind::Tune { .. } | LineKind::Job { .. } => {}
                other => {
                    let line = TraceLine { tick: k, kind: other.clone() };
                    return gap(
                        k,
                        format!(
                            "die Stimuluszeile `{}`",
                            Trace { lines: vec![line], ..Trace::default() }.render().trim()
                        ),
                    );
                }
            }
        }
        for (s, n) in delivered {
            out.insert((k, format!("i.stream.{s}.n")), Val::Int(n));
        }
        // 8.4: ein Tunable ab dem Tick, in dem der Interpreter es uebernahm.
        for l in seen.trace.at(k) {
            if let LineKind::Tune { name, value, accepted: true } = &l.kind
                && let Some(v) = tune(p, name, value, k)?
            {
                tunes.insert(name.clone(), v);
            }
        }
        out.extend(tunes.iter().map(|(n, v)| ((k, format!("i.tune.{n}")), *v)));
        let samples = usize::try_from(k).ok().and_then(|k| seen.inputs.get(k));
        for (c, s) in p.channels.iter().zip(samples.into_iter().flatten()) {
            if c.dir == Direction::Input && !matches!(p.types.get(c.ty), Type::Stream(_)) {
                sample_inputs(p, c, s, k, &mut out)?;
            }
        }
    }
    job_delays(model, p, stimulus, ticks, &mut out)?;
    Ok(out)
}

fn gap<T>(tick: u64, what: String) -> Result<T, Stop> {
    Err(Stop::Gap { tick, what })
}

/// Der Wert eines Tunables, `None` fuer einen anderen Parameter.
fn tune(p: &Program, name: &str, text: &str, k: u64) -> Result<Option<Val>, Stop> {
    let Some(param) = p.params.iter().find(|x| x.tunable && x.name == name) else { return Ok(None) };
    let v = parse_value(text, param.ty, p).ok().and_then(|v| val_of(&v));
    match v {
        Some(v) => Ok(Some(v)),
        None => gap(k, format!("der Wert `{text}` des Tunables `{name}`")),
    }
}

/// Eine Abtastung, wie die Maschinen sie lesen: `i.<c>.held` ohne Lieferung
/// in diesem Tick, `i.<c>.q` die Qualitaet, `i.<c>` der Wert, wenn es einen
/// gibt; bei `samples<T, N>` dazu, was der Rand an Werten pruefte.
fn sample_inputs(
    p: &Program,
    c: &Channel,
    seen: &Seen,
    k: u64,
    out: &mut BTreeMap<(u64, String), Val>,
) -> Result<(), Stop> {
    let name = &c.name;
    let s = &seen.sample;
    out.insert((k, format!("i.{name}.held")), Val::Bool(seen.delivery.is_none()));
    let q = match s.quality {
        Quality::Good => quality::GOOD,
        Quality::Suspect => quality::SUSPECT,
        Quality::Stale => quality::STALE,
        Quality::Bad => quality::BAD,
    };
    out.insert((k, format!("i.{name}.q")), Val::Int(q));
    // Der Wert einer ungueltigen Lieferung ist unbeobachtbar und im Modell
    // null; eine gehaltene, die `Stale` wurde, traegt ihren weiter.
    let carried = s.quality == Quality::Stale && seen.delivery.is_none();
    let visible = matches!(s.quality, Quality::Good | Quality::Suspect) || carried;
    let unreadable = || Stop::Gap { tick: k, what: format!("die Abtastung von `{name}`") };
    match &s.value {
        // 8.9: das Array, das die Maschinen lesen.
        Some(Value::Samples(items)) if visible => {
            out.insert((k, format!("i.{name}.v")), Val::Bool(true));
            for (j, x) in items.iter().enumerate() {
                out.insert((k, format!("i.{name}[{j}]")), val_of(x).ok_or_else(unreadable)?);
            }
        }
        // Ein Skalar ist ein Blatt an `i.<c>`, ein zusammengesetzter Wert
        // hat seine Blaetter darunter — nach dem Typ, nicht nach dem Wert:
        // `BUSY` eines Enums mit Feldern steht an `i.<c>.tag`.
        Some(v) if visible => {
            let leaves = leaves_at(p, c.ty, v, &format!("i.{name}")).ok_or_else(unreadable)?;
            out.extend(leaves.into_iter().map(|(n, x)| ((k, n), x)));
        }
        _ => {}
    }
    // Was der Rand prueft (12.6, Zeilen 3 und 4): eine Lieferung mit Wert
    // ausser `Bad`, ein `samples`-Array nur ganz. Ein Skalar kam durch, wenn
    // er Qualitaet und Wert seines Treibers behielt (`pass`); bei einem Array
    // heisst `pass`, dass der Rand es pruefte.
    let Some(delivery) = seen.delivery.as_ref() else { return Ok(()) };
    let Some(value) = delivery.value.as_ref().filter(|_| delivery.quality != Quality::Bad) else { return Ok(()) };
    match (value, p.types.get(c.ty)) {
        (Value::Samples(items), Type::Samples { len, .. }) if items.len() == *len as usize => {
            out.insert((k, format!("i.{name}.pass")), Val::Bool(true));
            for (j, x) in items.iter().enumerate() {
                out.insert((k, format!("i.{name}.d[{j}]")), val_of(x).ok_or_else(unreadable)?);
            }
        }
        (Value::Samples(_), _) => {}
        (v, _) if s.quality == delivery.quality && s.value.as_ref() == Some(v) => {
            out.insert((k, format!("i.{name}.pass")), Val::Bool(true));
            if let Some(x) = val_of(v) {
                out.insert((k, format!("i.{name}.d")), x);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Ein skalarer Wert des Interpreters als Blatt; ein `u64` als Bitmuster.
fn val_of(v: &Value) -> Option<Val> {
    Some(match v {
        Value::Bool(b) => Val::Bool(*b),
        Value::Int(x) | Value::Duration(x) => Val::Int(*x),
        Value::UInt(x) => Val::Int(*x as i64),
        Value::F32(x) => Val::F32(*x),
        Value::F64(x) => Val::F64(*x),
        Value::Enum { variant, fields } if fields.is_empty() => Val::Int(i64::from(*variant)),
        _ => return None,
    })
}

/// 4.5: Die Verspaetung eines Jobs, der im Tick k startet, aus den Zeilen
/// `job … start=k` (`job_start`): bis zu `done`, mit `late` allein so weit,
/// dass er im Lauf nicht mehr fertig wird.
fn job_delays(
    model: &Model,
    p: &Program,
    stimulus: &Trace,
    ticks: u64,
    out: &mut BTreeMap<(u64, String), Val>,
) -> Result<(), Stop> {
    for (n, _) in &model.inputs {
        let Some((machine, handle)) =
            n.strip_prefix("i.job.").and_then(|r| r.strip_suffix(".delay")).and_then(|r| r.split_once('.'))
        else {
            continue;
        };
        let no_slot = || Stop::Gap { tick: 0, what: format!("der Job `{machine}.{handle}`") };
        let def = p.machines.iter().find(|m| m.name == machine).ok_or_else(no_slot)?;
        let slot =
            def.layout.job_slots.iter().find(|s| def.vars[s.handle.index()].name == handle).ok_or_else(no_slot)?;
        let t0 = p.config.tick.max(1);
        let span = (p.natives[slot.native.index()].duration.unwrap_or(0).max(0) + t0 - 1) / t0;
        let span = u64::try_from(span).unwrap_or(0);
        let mut records: BTreeMap<u64, Option<u64>> = BTreeMap::new();
        for l in &stimulus.lines {
            if let LineKind::Job { machine: m, handle: h, start, late } = &l.kind
                && m == machine
                && h == handle
            {
                let done = records.entry(*start).or_default();
                *done = done.or((!late).then_some(l.tick));
            }
        }
        let never = crate::encode::job::LATEST / t0;
        for k in 0..=ticks {
            let late = match records.get(&k) {
                None => 0,
                Some(Some(done)) => i64::try_from(done.saturating_sub(k + span)).unwrap_or(never),
                Some(None) => never,
            };
            out.insert((k, n.clone()), Val::Int(late));
        }
    }
    Ok(())
}

/// Schreibt den Trace des Modells wie `Writer` im Interpreter: im Tick 0
/// alle Anfangswerte, danach nur Aenderungen; die Zaehler der Stroeme erst
/// ab Tick 1, weil ihr Anfangsstand keine Beobachtung ist.
struct Writer<'a> {
    model: &'a Model,
    p: &'a Program,
    /// Die Maschinen des Modells in der Folge ihrer Nummern.
    machines: Vec<MachineId>,
    lines: Vec<TraceLine>,
    shown: BTreeMap<String, String>,
    /// Die Eigenschaften, deren Verletzung gemeldet ist.
    violated: Vec<bool>,
    /// Je Zaehler einer Position `Oldest` sein Stand in jedem Tick.
    counters: BTreeMap<String, Vec<i64>>,
}

impl<'a> Writer<'a> {
    fn new(model: &'a Model, p: &'a Program) -> Writer<'a> {
        let machines = (0..p.machines.len())
            .map(|i| MachineId(i as u32))
            .filter(|m| model.paths.contains_key(&p.machines[m.index()].name))
            .collect();
        let violated = vec![false; model.properties.len()];
        let counters = model
            .properties
            .iter()
            .filter_map(|g| match &g.position {
                Position::Oldest { counter, .. } => Some((counter.clone(), Vec::new())),
                Position::Lag(_) => None,
            })
            .collect();
        Writer { model, p, machines, lines: Vec::new(), shown: BTreeMap::new(), violated, counters }
    }

    /// 13.3: die erste Verletzung jeder Eigenschaft, an der Position, die
    /// der Tick entscheidet — wie `Monitor::observe`, das danach schweigt.
    fn goals(&mut self, k: u64, model: &Model, held: &[bool], s: &Env) -> Result<(), Stop> {
        for (loc, seen) in &mut self.counters {
            seen.push(int(s, loc, k)?);
        }
        for (i, g) in model.properties.iter().enumerate() {
            if held.get(i) != Some(&false) || self.violated[i] {
                continue;
            }
            self.violated[i] = true;
            let at = match &g.position {
                Position::Lag(n) => k.saturating_sub(*n),
                Position::Oldest { counter, window } => {
                    let seen = &self.counters[counter];
                    let first = k.saturating_sub(*window);
                    (first..=k).find(|t| usize::try_from(*t).ok().and_then(|t| seen.get(t)) == Some(&0)).unwrap_or(k)
                }
            };
            self.push(k, LineKind::Property { assumption: g.assumption, name: g.name.clone(), at });
        }
        Ok(())
    }

    /// Liest der Trace den Ort `name` des Zustands? Nur diese Orte gehen je
    /// Tick an den Schreiber.
    fn reads(&self, name: &str) -> bool {
        let under = |base: &str| {
            name.strip_prefix(base)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.') || rest.starts_with('['))
        };
        if name == ENDED || self.counters.contains_key(name) {
            return true;
        }
        for &m in &self.machines {
            let def = &self.p.machines[m.index()];
            let Some(rest) = name.strip_prefix(&format!("s.{}.", def.name)) else { continue };
            if matches!(rest, "leaf" | "faulted" | "fault") || rest.starts_with("sig.") {
                return true;
            }
            if def.vars.iter().any(|v| v.public && under(&format!("s.{}.v.{}", def.name, v.name))) {
                return true;
            }
        }
        for c in self.p.channels.iter().filter(|c| c.dir == Direction::Output) {
            let base = match self.p.types.get(c.ty) {
                Type::Stream(_) => format!("s.tx.{}.sent", c.name),
                _ => format!("s.out.{}", c.name),
            };
            if under(&base) {
                return true;
            }
        }
        name.strip_prefix("s.stream.").is_some_and(|rest| {
            rest.ends_with(".dropped") || rest.ends_with(".overflowed") || rest.ends_with(".malformed")
        })
    }

    /// Was ein Tick beobachten laesst: Faults und Signale, dann die
    /// Aenderungen.
    fn tick(&mut self, k: u64, s: &Env) -> Result<(), Stop> {
        let p = self.p;
        for &m in &self.machines.clone() {
            let def = &p.machines[m.index()];
            let code = int(s, &format!("s.{}.fault", def.name), k)?;
            if code != 0 {
                let kind: FaultKind = fault_kind(code).ok_or_else(|| missing(k, "die Art eines Faults"))?;
                let target = self.path(m, s, k)?;
                let kind = kind.name();
                self.push(k, LineKind::Fault { machine: def.name.clone(), kind, message: String::new(), target });
            }
            for sig in &def.signals {
                if s.get(&format!("s.{}.sig.{}", def.name, sig.name)) == Some(&Val::Bool(true)) {
                    self.push(k, LineKind::Signal { machine: def.name.clone(), name: sig.name.clone() });
                }
            }
        }
        self.changes(k, s)
    }

    /// Zustaende, `pub var`, Outputs, Gesendetes, Zaehler — je bei Aenderung.
    fn changes(&mut self, k: u64, s: &Env) -> Result<(), Stop> {
        let p = self.p;
        for &m in &self.machines.clone() {
            let def = &p.machines[m.index()];
            let path = self.path(m, s, k)?;
            self.changed(
                k,
                format!("state {}", def.name),
                path.clone(),
                LineKind::State { machine: def.name.clone(), path },
            );
            for v in def.vars.iter().filter(|v| v.public) {
                let value = self.text(v.ty, &format!("s.{}.v.{}", def.name, v.name), s, k)?;
                let kind = LineKind::Published { machine: def.name.clone(), var: v.name.clone(), value: value.clone() };
                self.changed(k, format!("pub {}.{}", def.name, v.name), value, kind);
            }
        }
        for c in p.channels.iter().filter(|c| c.dir == Direction::Output) {
            if matches!(p.types.get(c.ty), Type::Stream(_)) {
                let sent = format!("s.tx.{}.sent", c.name);
                let n = int(s, &format!("{sent}.len"), k)?;
                if n > 0 {
                    let bytes = (0..n)
                        .map(|j| u8::try_from(int(s, &format!("{sent}[{j}]"), k)?).map_err(|_| missing(k, &sent)));
                    let value = value_text(&Value::Bytes(bytes.collect::<Result<_, _>>()?), c.ty, p);
                    self.push(k, LineKind::Output { channel: c.name.clone(), value });
                }
                continue;
            }
            let value = self.text(c.ty, &format!("s.out.{}", c.name), s, k)?;
            let kind = LineKind::Output { channel: c.name.clone(), value: value.clone() };
            self.changed(k, format!("out {}", c.name), value, kind);
        }
        let inputs =
            p.channels.iter().filter(|c| c.dir == Direction::Input && matches!(p.types.get(c.ty), Type::Stream(_)));
        let names: Vec<String> =
            inputs.map(|c| c.name.clone()).chain(p.streams.iter().map(|d| d.name.clone())).collect();
        for name in names {
            let count = |part: &str| {
                int(s, &format!("s.stream.{name}.{part}"), k).map(|n| u32::try_from(n).unwrap_or(u32::MAX))
            };
            let (dropped, overflowed, malformed) = (count("dropped")?, count("overflowed")?, count("malformed")?);
            let key = format!("stream {name}");
            let now = format!("{dropped} {overflowed} {malformed}");
            if k == 0 {
                self.shown.insert(key, now);
                continue;
            }
            self.changed(k, key, now, LineKind::Stream { name, dropped, overflowed, malformed });
        }
        Ok(())
    }

    /// 12.7: die Zeile `end` mit dem Grund aus `sys/next_run`.
    fn end(&mut self, k: u64, s: &Env) -> Result<(), Stop> {
        let (index, variants) = takt_mir::sys::next_run(self.p).ok_or_else(|| missing(k, "`sys/next_run`"))?;
        let c = &self.p.channels[index];
        let variant = match value_at(self.p, c.ty, &format!("s.out.{}", c.name), s) {
            Some(Value::Enum { variant, .. }) => variant,
            _ => return Err(missing(k, &format!("s.out.{}", c.name))),
        };
        let next = variants.get(variant as usize).and_then(|v| v.1);
        let next = next.ok_or_else(|| missing(k, "der Grund des Endes"))?;
        self.push(k, LineKind::End { reason: next.word().to_string() });
        Ok(())
    }

    /// Der Pfad wie `MachineState::path`: `FAULTED` vor jedem Blatt — ohne
    /// Zustandsenum bleibt der Blattcode einer gefaulteten Maschine stehen.
    fn path(&self, m: MachineId, s: &Env, k: u64) -> Result<String, Stop> {
        let name = &self.p.machines[m.index()].name;
        if s.get(&format!("s.{name}.faulted")) == Some(&Val::Bool(true)) {
            return Ok("FAULTED".into());
        }
        let code = int(s, &format!("s.{name}.leaf"), k)?;
        let paths = self.model.paths.get(name).ok_or_else(|| missing(k, name))?;
        let path = paths.iter().find(|(c, _)| *c == code).map(|(_, p)| p.clone());
        path.ok_or_else(|| missing(k, &format!("der Zustand {code} von `{name}`")))
    }

    /// Der Wert an `base` in seiner Textform (`value_text`).
    fn text(&self, ty: takt_mir::TypeId, base: &str, s: &Env, k: u64) -> Result<String, Stop> {
        let v = value_at(self.p, ty, base, s)
            .ok_or_else(|| Stop::Gap { tick: k, what: format!("`{base}` hat im Modell keinen Wert") })?;
        Ok(value_text(&v, ty, self.p))
    }

    /// Eine Zeile, wenn sich `now` gegenueber dem zuletzt Geschriebenen
    /// unter `key` aendert.
    fn changed(&mut self, k: u64, key: String, now: String, kind: LineKind) {
        if self.shown.get(&key) != Some(&now) {
            self.shown.insert(key, now);
            self.push(k, kind);
        }
    }

    fn push(&mut self, tick: u64, kind: LineKind) {
        self.lines.push(TraceLine { tick, kind });
    }
}

fn int(s: &Env, loc: &str, k: u64) -> Result<i64, Stop> {
    match s.get(loc) {
        Some(Val::Int(x)) => Ok(*x),
        _ => Err(missing(k, loc)),
    }
}

fn missing(tick: u64, what: &str) -> Stop {
    Stop::Gap { tick, what: format!("`{what}` fehlt im Modell") }
}
