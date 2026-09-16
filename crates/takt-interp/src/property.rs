//! Eigenschaftsmonitore (13.3): jede `property`/`assumption` laeuft ueber
//! die Folge der Tick-Rand-Snapshots — committete Outputs, Ψ, Zustaende,
//! Inputs und Commands des Ticks. Die Formel ist auf dem endlichen Lauf
//! dreiwertig: entschieden wahr, entschieden falsch, offen (ein Fenster
//! reicht ueber das Laufende hinaus). `always` gilt, wenn jede Position
//! entschieden wahr ist; eine Verletzung ist ein FAIL-Befund (13.5). Eine
//! Position wird in dem Tick entschieden, in dem ihre Fenster geschlossen
//! sind — Position plus Zukunftstiefe der Formel —, damit der native
//! Monitor mit seinem Ringpuffer denselben Tick meldet (plan/m6.md 2.8).

use takt_mir::expr::{Builtin, Expr, TProp, TemporalOp};
use takt_mir::program::Property;
use takt_mir::{ChannelId, CommandId, MachineId, ParamId, SignalId};

use crate::env::Outer;
use crate::eval::Ctx;
use crate::image::Image;
use crate::loaded::Loaded;
use crate::value::{EvalResult, Sample, Trap, Value, bug};

/// Ausgang einer Eigenschaft am Laufende.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Jede Position entschieden wahr.
    Satisfied,
    /// Verletzt.
    Violated {
        /// Position der Verletzung.
        at: u64,
        /// Tick, in dem sie entschieden wurde.
        detected: u64,
    },
    /// Weder PASS noch FAIL: ein Fenster reicht ueber das Ende.
    Open {
        /// Erste offene Position.
        since: u64,
    },
}

/// Eine Eigenschaft mit ihrem Ausgang.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyResult {
    /// Name.
    pub name: String,
    /// `assumption` statt `property`.
    pub assumption: bool,
    /// Ausgang.
    pub outcome: Outcome,
}

impl Outcome {
    /// Der Ausgang als Text fuer Berichte.
    pub fn text(&self) -> String {
        match self {
            Outcome::Satisfied => "erfuellt".to_string(),
            Outcome::Violated { at, .. } => format!("verletzt bei t={at}"),
            Outcome::Open { since } => format!("offen ab t={since}"),
        }
    }
}

/// Die Formel; Atome zeigen in die Geschichte.
enum Node {
    Atom(usize),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    /// `always`: nur ausserhalb beschraenkter Operatoren (Pruefung 56), also
    /// immer an Position 0; `next` ist die erste ungeprueft Position,
    /// `pending` die offenen, `violated` die erste falsche, `future` die
    /// Zukunftstiefe des Inneren in Ticks.
    Always {
        inner: Box<Node>,
        next: i64,
        pending: Vec<i64>,
        violated: Option<i64>,
        future: i64,
    },
    /// `eventually`/`stable`/`once` mit Fenster in Ticks.
    Bounded {
        op: TemporalOp,
        ticks: i64,
        inner: Box<Node>,
    },
}

fn build(f: &TProp, tick: i64, atoms: &mut Vec<Expr>) -> Node {
    match f {
        TProp::Atom(e) => {
            atoms.push(e.clone());
            Node::Atom(atoms.len() - 1)
        }
        TProp::Not(a) => Node::Not(Box::new(build(a, tick, atoms))),
        TProp::And(a, b) => Node::And(Box::new(build(a, tick, atoms)), Box::new(build(b, tick, atoms))),
        TProp::Or(a, b) => Node::Or(Box::new(build(a, tick, atoms)), Box::new(build(b, tick, atoms))),
        TProp::Implies(a, b) => {
            Node::Or(Box::new(Node::Not(Box::new(build(a, tick, atoms)))), Box::new(build(b, tick, atoms)))
        }
        TProp::Temporal { op: TemporalOp::Always, inner, .. } => Node::Always {
            inner: Box::new(build(inner, tick, atoms)),
            next: 0,
            pending: Vec::new(),
            violated: None,
            future: future(inner, tick),
        },
        TProp::Temporal { op: TemporalOp::Never, inner, .. } => Node::Always {
            inner: Box::new(Node::Not(Box::new(build(inner, tick, atoms)))),
            next: 0,
            pending: Vec::new(),
            violated: None,
            future: future(inner, tick),
        },
        TProp::Temporal { op, window, inner } => Node::Bounded {
            op: *op,
            ticks: window.unwrap_or(0) / tick.max(1),
            inner: Box::new(build(inner, tick, atoms)),
        },
    }
}

/// Zukunftstiefe in Ticks: wie weit eine Position nach vorn schaut.
fn future(f: &TProp, tick: i64) -> i64 {
    match f {
        TProp::Atom(_) => 0,
        TProp::Not(a)
        | TProp::Temporal { op: TemporalOp::Always | TemporalOp::Never | TemporalOp::Once, inner: a, .. } => {
            future(a, tick)
        }
        TProp::And(a, b) | TProp::Or(a, b) | TProp::Implies(a, b) => future(a, tick).max(future(b, tick)),
        TProp::Temporal { window, inner, .. } => window.unwrap_or(0) / tick.max(1) + future(inner, tick),
    }
}

fn and3(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None,
    }
}

fn or3(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    and3(a.map(|x| !x), b.map(|x| !x)).map(|x| !x)
}

/// Wert der Formel an Position `p` ueber der Geschichte; `None` ist
/// offen. `always` prueft eine Position erst, wenn ihre Fenster
/// geschlossen sind; `finished`: der Lauf ist zu Ende, dann jede Position,
/// und `always` ohne offene ist wahr.
fn eval(node: &mut Node, p: i64, hist: &[Vec<bool>], finished: bool) -> Option<bool> {
    let now = hist.len() as i64 - 1;
    match node {
        Node::Atom(i) => {
            if p < 0 {
                Some(false)
            } else if p > now {
                None
            } else {
                Some(hist[p as usize][*i])
            }
        }
        Node::Not(a) => eval(a, p, hist, finished).map(|b| !b),
        Node::And(a, b) => and3(eval(a, p, hist, finished), eval(b, p, hist, finished)),
        Node::Or(a, b) => or3(eval(a, p, hist, finished), eval(b, p, hist, finished)),
        Node::Bounded { op, ticks, inner } => {
            let (lo, hi) = if *op == TemporalOp::Once { (p - *ticks, p) } else { (p, p + *ticks) };
            let mut open = false;
            for q in lo..=hi {
                match (*op, eval(inner, q, hist, finished)) {
                    (TemporalOp::Stable, Some(false)) => return Some(false),
                    (TemporalOp::Eventually | TemporalOp::Once, Some(true)) => return Some(true),
                    (_, None) => open = true,
                    _ => {}
                }
            }
            if open { None } else { Some(*op == TemporalOp::Stable) }
        }
        Node::Always { inner, next, pending, violated, future } => {
            if violated.is_some() {
                return Some(false);
            }
            let mut positions: Vec<i64> = std::mem::take(pending);
            let closed = if finished { now } else { now - *future };
            while *next <= closed {
                positions.push(*next);
                *next += 1;
            }
            for q in positions {
                match eval(inner, q, hist, finished) {
                    Some(false) => {
                        *violated = Some(q);
                        return Some(false);
                    }
                    None => pending.push(q),
                    Some(true) => {}
                }
            }
            if !pending.is_empty() {
                None
            } else if finished {
                Some(true)
            } else {
                None
            }
        }
    }
}

/// Die erste verletzte Position eines `always`.
fn violated_at(node: &Node) -> Option<i64> {
    match node {
        Node::Atom(_) => None,
        Node::Not(a) | Node::Bounded { inner: a, .. } => violated_at(a),
        Node::And(a, b) | Node::Or(a, b) => violated_at(a).or_else(|| violated_at(b)),
        Node::Always { violated, inner, .. } => violated.or_else(|| violated_at(inner)),
    }
}

/// Die erste offene Position eines `always`.
fn open_since(node: &Node) -> Option<i64> {
    match node {
        Node::Atom(_) => None,
        Node::Not(a) | Node::Bounded { inner: a, .. } => open_since(a),
        Node::And(a, b) | Node::Or(a, b) => match (open_since(a), open_since(b)) {
            (Some(x), Some(y)) => Some(x.min(y)),
            (x, y) => x.or(y),
        },
        Node::Always { pending, inner, .. } => pending.iter().copied().min().or_else(|| open_since(inner)),
    }
}

/// Der Monitor einer Eigenschaft.
pub struct Monitor {
    /// Name.
    pub name: String,
    /// `assumption` statt `property`.
    pub assumption: bool,
    atoms: Vec<Expr>,
    node: Node,
    /// Zukunftstiefe der ganzen Formel: vorher ist Position 0 nicht entschieden.
    future: i64,
    history: Vec<Vec<bool>>,
    outcome: Option<Outcome>,
}

impl Monitor {
    /// Ein Monitor fuer `p` bei Tick `tick` (Nanosekunden).
    pub fn new(p: &Property, tick: i64) -> Monitor {
        let mut atoms = Vec::new();
        let node = build(&p.formula, tick, &mut atoms);
        let future = future(&p.formula, tick);
        Monitor {
            name: p.name.clone(),
            assumption: p.assumption,
            atoms,
            node,
            future,
            history: Vec::new(),
            outcome: None,
        }
    }

    /// Der Snapshot des Ticks; liefert die Verletzungsposition, wenn die
    /// Eigenschaft jetzt entschieden falsch ist. Ein Atom, das nicht
    /// auswertbar ist (ungueltiger Input, 3.5), zaehlt als falsch (13.5).
    pub fn observe(&mut self, loaded: &Loaded<'_>, image: &Image, tick: u64) -> Option<u64> {
        if self.outcome.is_some() {
            return None;
        }
        let mut env = Snapshot { image, tick, tick_ns: loaded.program.config.tick };
        let values: Vec<bool> = self
            .atoms
            .iter()
            .map(|a| matches!(Ctx::new(loaded, &mut env, tick).eval(a), Ok(Value::Bool(true))))
            .collect();
        self.history.push(values);
        self.decide(tick, false)
    }

    /// Das Laufende: was noch offen ist, wird mit `finished` entschieden.
    pub fn close(&mut self, last: u64) -> Option<u64> {
        if self.outcome.is_some() {
            return None;
        }
        let at = self.decide(last, true);
        if self.outcome.is_none() {
            self.outcome = Some(match eval(&mut self.node, 0, &self.history, true) {
                Some(true) => Outcome::Satisfied,
                _ => Outcome::Open { since: open_since(&self.node).unwrap_or(0).max(0) as u64 },
            });
        }
        at
    }

    fn decide(&mut self, tick: u64, finished: bool) -> Option<u64> {
        if !finished && (self.history.len() as i64) <= self.future {
            return None;
        }
        if eval(&mut self.node, 0, &self.history, finished) == Some(false) {
            let at = violated_at(&self.node).unwrap_or(0).max(0) as u64;
            self.outcome = Some(Outcome::Violated { at, detected: tick });
            return Some(at);
        }
        None
    }

    /// Der Ausgang; nach `close`.
    pub fn result(self) -> PropertyResult {
        PropertyResult {
            name: self.name,
            assumption: self.assumption,
            outcome: self.outcome.unwrap_or(Outcome::Open { since: 0 }),
        }
    }
}

/// Der Tick-Rand-Snapshot als Umgebung: committete Outputs, Ψ, Inputs und
/// Commands des Ticks, Parameter.
struct Snapshot<'a> {
    image: &'a Image,
    tick: u64,
    tick_ns: i64,
}

impl Outer for Snapshot<'_> {
    fn param(&self, p: ParamId) -> EvalResult<&Value> {
        self.image.params.get(p.index()).ok_or_else(|| Trap::Bug(format!("Parameter {} fehlt", p.0)))
    }

    fn command(&self, c: CommandId) -> EvalResult<bool> {
        Ok(self.image.command(c))
    }

    fn input(&self, c: ChannelId) -> EvalResult<&Sample> {
        Ok(self.image.input(c))
    }

    fn output(&self, c: ChannelId) -> EvalResult<&Value> {
        Ok(self.image.committed_output(c))
    }

    fn published(&self, m: MachineId, v: VarId) -> EvalResult<&Value> {
        self.image
            .published_var(m, v, false)
            .ok_or_else(|| Trap::Bug(format!("`pub var` {} von Maschine {} fehlt", v.0, m.0)))
    }

    fn state_of(&self, m: MachineId) -> EvalResult<Value> {
        self.image
            .published_state(m, false)
            .cloned()
            .ok_or_else(|| Trap::Bug(format!("Zustand von Maschine {} fehlt", m.0)))
    }

    fn signal(&self, m: MachineId, s: SignalId) -> EvalResult<bool> {
        Ok(self.image.published_signal(m, s, false))
    }

    fn builtin(&self, b: Builtin) -> EvalResult<Value> {
        match b {
            Builtin::Now => {
                Ok(Value::Duration(i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(self.tick_ns)))
            }
            Builtin::Tick => Ok(Value::Duration(self.tick_ns)),
            other => bug(format!("`{other:?}` in einer Eigenschaft")),
        }
    }
}

use takt_mir::VarId;
