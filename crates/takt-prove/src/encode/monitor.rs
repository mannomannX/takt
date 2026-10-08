//! Zeitoperatoren als Zustand (13.3, M11 Schritt 27b). Der Monitor der
//! Laufzeit wird Teil des Modells, statt die Operatoren ein zweites Mal zu
//! definieren; eine Eigenschaft ist danach eine Invariante wie jede andere.
//!
//! Drei Formen, alle mit der Semantik von `takt_interp::property`:
//! - **Vergangenheit** (Atome und `once`): Die Formel gilt im Tick selbst;
//!   je `once[d](ψ)` ein Zaehler der Ticks seit ψ, gesaettigt hinter `d`.
//! - **Antwort** `a implies eventually[d](b)`: ein Zaehler der Ticks seit
//!   der aeltesten offenen Pflicht; die Eigenschaft ist verletzt, wenn er
//!   `d` erreicht, im selben Tick, in dem der Interpreter die Position
//!   entscheidet. `a implies stable[d](b)` ist auf unendlichen Laeufen
//!   gleich `b or not once[d](a)` und hat damit Zaehler.
//! - **Ring** sonst: je Atom seine Werte der letzten `F + P` Ticks, die
//!   Position `k − F` im Tick `k` entschieden wie im nativen Monitor; `F`
//!   ist die Zukunfts-, `P` die Vergangenheitstiefe der Formel.
//!
//! Eine Position vor dem Start ist falsch wie im Interpreter.

use std::collections::BTreeMap;
use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Expr, TProp, TemporalOp};
use takt_mir::program::Property;

use super::{Cx, Edge, Enc, Env, Flow, Mode, R, UNROLL_LIMIT, no};
use crate::term::{Op, Term};

/// Eine Formel unter `always` mit Fenstern in Ticks; jedes `once` hat
/// eine Nummer fuer seinen Zaehler.
#[derive(Clone, Debug)]
enum Mon {
    Atom(usize),
    Not(Box<Mon>),
    And(Box<Mon>, Box<Mon>),
    Or(Box<Mon>, Box<Mon>),
    Eventually(i64, Box<Mon>),
    Stable(i64, Box<Mon>),
    Once(usize, i64, Box<Mon>),
}

impl Mon {
    /// Wie weit eine Position nach vorn schaut (`property::future`).
    fn future(&self) -> i64 {
        match self {
            Mon::Atom(_) => 0,
            Mon::Not(a) | Mon::Once(_, _, a) => a.future(),
            Mon::And(a, b) | Mon::Or(a, b) => a.future().max(b.future()),
            Mon::Eventually(n, a) | Mon::Stable(n, a) => n + a.future(),
        }
    }

    /// Wie weit sie zurueckschaut.
    fn past(&self) -> i64 {
        match self {
            Mon::Atom(_) => 0,
            Mon::Not(a) | Mon::Eventually(_, a) | Mon::Stable(_, a) => a.past(),
            Mon::And(a, b) | Mon::Or(a, b) => a.past().max(b.past()),
            Mon::Once(_, n, a) => n + a.past(),
        }
    }

    /// Die `once` darin, innerstes zuerst.
    fn onces(&self, out: &mut Vec<(usize, i64)>) {
        match self {
            Mon::Atom(_) => {}
            Mon::Not(a) | Mon::Eventually(_, a) | Mon::Stable(_, a) => a.onces(out),
            Mon::And(a, b) | Mon::Or(a, b) => {
                a.onces(out);
                b.onces(out);
            }
            Mon::Once(id, n, a) => {
                a.onces(out);
                out.push((*id, *n));
            }
        }
    }
}

/// Wie eine Eigenschaft im Zustand entschieden wird.
#[derive(Clone, Debug)]
enum Plan {
    Past(Mon),
    Response { a: Mon, b: Mon, n: i64 },
    Ring { formula: Mon, future: i64, depth: i64 },
}

/// Der Monitor einer Eigenschaft mit Zeitoperatoren.
#[derive(Clone, Debug)]
pub(super) struct Monitor {
    /// Die Eigenschaft.
    name: String,
    atoms: Vec<Expr>,
    plan: Plan,
    /// Die `once`-Zaehler mit ihrem Fenster.
    onces: Vec<(usize, i64)>,
}

impl Monitor {
    /// Die Eigenschaft.
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    fn loc(&self, part: &str) -> String {
        format!("s.mon.{}.{part}", self.name)
    }

    fn ring(&self, atom: usize, back: i64) -> String {
        self.loc(&format!("r{atom}.{back}"))
    }

    fn once(&self, id: usize) -> String {
        self.loc(&format!("once{id}"))
    }

    /// Das laengste Fenster des Monitors in Ticks.
    pub(super) fn window(&self) -> i64 {
        let onces = self.onces.iter().map(|(_, n)| *n).max().unwrap_or(0);
        match &self.plan {
            Plan::Ring { depth, .. } => *depth,
            Plan::Response { n, .. } => (*n).max(onces),
            Plan::Past(_) => onces,
        }
    }

    /// Wie weit die Entscheidung einer Position hinter dem Tick liegt.
    pub(super) fn future(&self) -> i64 {
        match &self.plan {
            Plan::Ring { future, .. } => *future,
            Plan::Past(_) | Plan::Response { .. } => 0,
        }
    }
}

/// Die Formel unter `always` (`never(φ)` ist `always(not φ)`); `None`, wenn
/// die Eigenschaft keinen Zeitoperator darin hat.
fn under_always(f: &TProp) -> Option<(&TProp, bool)> {
    match f {
        TProp::Temporal { op: TemporalOp::Always, inner, .. } => Some((inner, false)),
        TProp::Temporal { op: TemporalOp::Never, inner, .. } => Some((inner, true)),
        _ => None,
    }
}

fn temporal(f: &TProp) -> bool {
    match f {
        TProp::Temporal { .. } => true,
        TProp::Atom(_) => false,
        TProp::Not(a) => temporal(a),
        TProp::And(a, b) | TProp::Or(a, b) | TProp::Implies(a, b) => temporal(a) || temporal(b),
    }
}

impl Enc<'_> {
    /// Die Monitore der Eigenschaften mit Zeitoperatoren unter `always`.
    pub(super) fn monitors(&self) -> R<Vec<Monitor>> {
        let mut out = Vec::new();
        for prop in &self.p.properties {
            let Some((inner, negate)) = under_always(&prop.formula) else { continue };
            if !temporal(inner) {
                continue;
            }
            out.push(self.monitor(prop, inner, negate)?);
        }
        Ok(out)
    }

    fn monitor(&self, prop: &Property, inner: &TProp, negate: bool) -> R<Monitor> {
        let mut atoms = Vec::new();
        let mut next_once = 0;
        let formula = self.mon(inner, &mut atoms, &mut next_once, prop.span)?;
        let formula = if negate { Mon::Not(Box::new(formula)) } else { formula };
        let plan = match formula {
            f if f.future() == 0 => Plan::Past(f),
            Mon::Or(a, b) => match (*a, *b) {
                (Mon::Not(a), Mon::Eventually(n, b)) if a.future() == 0 && b.future() == 0 => {
                    Plan::Response { a: *a, b: *b, n }
                }
                // `a implies stable[d](b)`: verletzt, wenn `b` faellt, solange
                // `a` im Fenster davor galt.
                (Mon::Not(a), Mon::Stable(n, b)) if a.future() == 0 && b.future() == 0 => {
                    let once = Mon::Once(next_once, n, a);
                    Plan::Past(Mon::Or(b, Box::new(Mon::Not(Box::new(once)))))
                }
                (a, b) => self.ring(Mon::Or(Box::new(a), Box::new(b)), prop.span)?,
            },
            f => self.ring(f, prop.span)?,
        };
        let mut onces = Vec::new();
        match &plan {
            Plan::Past(f) => f.onces(&mut onces),
            Plan::Response { a, b, .. } => {
                a.onces(&mut onces);
                b.onces(&mut onces);
            }
            Plan::Ring { .. } => {}
        }
        Ok(Monitor { name: prop.name.clone(), atoms, plan, onces })
    }

    fn ring(&self, formula: Mon, span: Span) -> R<Plan> {
        let (future, past) = (formula.future(), formula.past());
        let depth = future + past;
        if depth > UNROLL_LIMIT {
            return no(format!("ein Fenster von {depth} Ticks ist fuer den Ring des Monitors zu lang"), span);
        }
        Ok(Plan::Ring { formula, future, depth })
    }

    fn mon(&self, f: &TProp, atoms: &mut Vec<Expr>, onces: &mut usize, span: Span) -> R<Mon> {
        let tick = self.p.config.tick.max(1);
        Ok(match f {
            TProp::Atom(e) => {
                atoms.push(e.clone());
                Mon::Atom(atoms.len() - 1)
            }
            TProp::Not(a) => Mon::Not(Box::new(self.mon(a, atoms, onces, span)?)),
            TProp::And(a, b) => {
                Mon::And(Box::new(self.mon(a, atoms, onces, span)?), Box::new(self.mon(b, atoms, onces, span)?))
            }
            TProp::Or(a, b) => {
                Mon::Or(Box::new(self.mon(a, atoms, onces, span)?), Box::new(self.mon(b, atoms, onces, span)?))
            }
            TProp::Implies(a, b) => Mon::Or(
                Box::new(Mon::Not(Box::new(self.mon(a, atoms, onces, span)?))),
                Box::new(self.mon(b, atoms, onces, span)?),
            ),
            TProp::Temporal { op, window, inner } => {
                let n = window.unwrap_or(0) / tick;
                let inner = Box::new(self.mon(inner, atoms, onces, span)?);
                match op {
                    TemporalOp::Eventually => Mon::Eventually(n, inner),
                    TemporalOp::Stable => Mon::Stable(n, inner),
                    TemporalOp::Once => {
                        *onces += 1;
                        Mon::Once(*onces - 1, n, inner)
                    }
                    TemporalOp::Always | TemporalOp::Never => {
                        return no("`always`/`never` unter einem anderen Operator (Pruefung 56)", span);
                    }
                }
            }
        })
    }

    /// Ein Atom auf einem Zustand am Tick-Rand; ein Atom, das faultet, ist
    /// falsch (13.5). Seine Lesestellen sind keine Pruefstellen.
    pub(super) fn atom(&mut self, e: &Expr, state: &Env) -> R<Term> {
        let actives = BTreeMap::new();
        // Der Rand vor diesem Tick steht in den Kopien `.prev`: Der Zustand
        // nach dem Commit hat die Lieferungen schon verbucht.
        let mut before = state.clone();
        for edge in self.edges()?.into_iter().filter(Edge::stateful) {
            if let Some(prev) = state.get(&edge.loc("has.prev")) {
                before.insert(edge.loc("has"), prev.clone());
            }
        }
        let cx = Cx { m: None, leaf: None, mode: Mode::Entry, pre: &before, active: &actives, locals: None };
        let sites = std::mem::take(&mut self.sites);
        let mut flow = Flow::new(Term::bool(true));
        let t = self.expr(e, &cx, state, &mut flow);
        self.sites = sites;
        let faults = Term::or(flow.exits.iter().map(|x| x.cond.clone()).collect());
        Ok(Term::ite(faults, Term::bool(false), t?))
    }

    /// Die Formel im Tick: Atome auf `state`, `once` ueber den Zaehler.
    fn now(&self, f: &Mon, atoms: &[Term], counters: &BTreeMap<usize, Term>) -> Term {
        match f {
            Mon::Atom(i) => atoms[*i].clone(),
            Mon::Not(a) => self.now(a, atoms, counters).not(),
            Mon::And(a, b) => Term::and(vec![self.now(a, atoms, counters), self.now(b, atoms, counters)]),
            Mon::Or(a, b) => Term::or(vec![self.now(a, atoms, counters), self.now(b, atoms, counters)]),
            Mon::Once(id, n, _) => Term::bin(Op::Le, counters[id].clone(), Term::int(*n)),
            // Ein Plan der Vergangenheit hat keine Zukunft.
            Mon::Eventually(..) | Mon::Stable(..) => Term::bool(false),
        }
    }

    /// Die Position `back` Ticks zurueck aus dem Ring.
    fn back_at(m: &Monitor, f: &Mon, back: i64, state: &Env) -> Term {
        match f {
            Mon::Atom(i) => state[&m.ring(*i, back)].clone(),
            Mon::Not(a) => Enc::back_at(m, a, back, state).not(),
            Mon::And(a, b) => Term::and(vec![Enc::back_at(m, a, back, state), Enc::back_at(m, b, back, state)]),
            Mon::Or(a, b) => Term::or(vec![Enc::back_at(m, a, back, state), Enc::back_at(m, b, back, state)]),
            Mon::Eventually(n, a) => Term::or((0..=*n).map(|j| Enc::back_at(m, a, back - j, state)).collect()),
            Mon::Stable(n, a) => Term::and((0..=*n).map(|j| Enc::back_at(m, a, back - j, state)).collect()),
            Mon::Once(_, n, a) => Term::or((0..=*n).map(|j| Enc::back_at(m, a, back + j, state)).collect()),
        }
    }

    /// Der Zustand der Monitore nach einem Tick: `state` ist sein
    /// committeter Zustand, `before` der davor (`None` vor dem ersten).
    pub(super) fn monitors_next(&mut self, monitors: &[Monitor], before: Option<&Env>, state: &mut Env) -> R<()> {
        for m in monitors {
            let mut atoms = Vec::new();
            for e in &m.atoms {
                atoms.push(self.atom(e, state)?);
            }
            // Die Zaehler, innerstes `once` zuerst: frisch, wo es gilt,
            // sonst einer mehr, gesaettigt hinter dem Fenster.
            let mut counters = BTreeMap::new();
            let formula = match &m.plan {
                Plan::Past(f) => Some(f.clone()),
                Plan::Response { a, b, .. } => Some(Mon::And(Box::new(a.clone()), Box::new(b.clone()))),
                Plan::Ring { .. } => None,
            };
            if let Some(f) = &formula {
                for (id, n) in &m.onces {
                    let inner = once_inner(f, *id).expect("once");
                    let holds = self.now(inner, &atoms, &counters);
                    let older = match before {
                        Some(b) => {
                            let c = b[&m.once(*id)].clone();
                            Term::ite(
                                Term::bin(Op::Gt, c.clone(), Term::int(*n)),
                                Term::int(n + 1),
                                Term::bin(Op::Add, c, Term::int(1)),
                            )
                        }
                        None => Term::int(n + 1),
                    };
                    counters.insert(*id, Term::ite(holds, Term::int(0), older));
                }
                for (id, c) in &counters {
                    state.insert(m.once(*id), c.clone());
                }
            }
            match &m.plan {
                Plan::Past(_) => {}
                Plan::Response { a, b, n } => {
                    let (a, b) = (self.now(a, &atoms, &counters), self.now(b, &atoms, &counters));
                    let open = match before {
                        Some(s) => {
                            let w = s[&m.loc("wait")].clone();
                            let older = Term::ite(
                                Term::bin(Op::Ge, w.clone(), Term::int(*n)),
                                Term::int(*n),
                                Term::bin(Op::Add, w.clone(), Term::int(1)),
                            );
                            Term::ite(
                                Term::bin(Op::Ge, w, Term::int(0)),
                                older,
                                Term::ite(a, Term::int(0), Term::int(-1)),
                            )
                        }
                        None => Term::ite(a, Term::int(0), Term::int(-1)),
                    };
                    state.insert(m.loc("wait"), Term::ite(b, Term::int(-1), open));
                }
                Plan::Ring { future, depth, .. } => {
                    for (i, t) in atoms.iter().enumerate() {
                        state.insert(m.ring(i, 0), t.clone());
                        for j in 1..=*depth {
                            let older = before.map_or_else(|| Term::bool(false), |b| b[&m.ring(i, j - 1)].clone());
                            state.insert(m.ring(i, j), older);
                        }
                    }
                    if *future > 0 {
                        let ticks = match before {
                            Some(b) => {
                                let t = b[&m.loc("ticks")].clone();
                                Term::ite(
                                    Term::bin(Op::Ge, t.clone(), Term::int(*future)),
                                    Term::int(*future),
                                    Term::bin(Op::Add, t, Term::int(1)),
                                )
                            }
                            None => Term::int(0),
                        };
                        state.insert(m.loc("ticks"), ticks);
                    }
                }
            }
        }
        Ok(())
    }

    /// Die Eigenschaft im Zustand `state`; vor der ersten entschiedenen
    /// Position gilt sie.
    pub(super) fn monitor_goal(&mut self, m: &Monitor, state: &Env) -> R<Term> {
        let counters: BTreeMap<usize, Term> =
            m.onces.iter().map(|(id, _)| (*id, state[&m.once(*id)].clone())).collect();
        Ok(match &m.plan {
            Plan::Past(f) => {
                let mut atoms = Vec::new();
                for e in &m.atoms {
                    atoms.push(self.atom(e, state)?);
                }
                self.now(f, &atoms, &counters)
            }
            Plan::Response { n, .. } => Term::bin(Op::Lt, state[&m.loc("wait")].clone(), Term::int(*n)),
            Plan::Ring { formula, future, .. } => {
                let decided = Enc::back_at(m, formula, *future, state);
                if *future == 0 {
                    decided
                } else {
                    Term::or(vec![Term::bin(Op::Lt, state[&m.loc("ticks")].clone(), Term::int(*future)), decided])
                }
            }
        })
    }

    /// Die Grenzen der Zaehler eines Monitors (3.4), fuer die Induktion.
    pub(super) fn monitor_invariants(m: &Monitor, state: &Env, out: &mut Vec<Term>) {
        let within = |x: Term, lo: i64, hi: i64| {
            Term::and(vec![Term::bin(Op::Ge, x.clone(), Term::int(lo)), Term::bin(Op::Le, x, Term::int(hi))])
        };
        for (id, n) in &m.onces {
            out.push(within(state[&m.once(*id)].clone(), 0, n + 1));
        }
        match &m.plan {
            Plan::Response { n, .. } => out.push(within(state[&m.loc("wait")].clone(), -1, *n)),
            Plan::Ring { future, .. } if *future > 0 => out.push(within(state[&m.loc("ticks")].clone(), 0, *future)),
            Plan::Ring { .. } | Plan::Past(_) => {}
        }
    }
}

/// Das Innere des `once` mit der Nummer `id`.
fn once_inner(f: &Mon, id: usize) -> Option<&Mon> {
    match f {
        Mon::Atom(_) => None,
        Mon::Not(a) | Mon::Eventually(_, a) | Mon::Stable(_, a) => once_inner(a, id),
        Mon::And(a, b) | Mon::Or(a, b) => once_inner(a, id).or_else(|| once_inner(b, id)),
        Mon::Once(i, _, a) if *i == id => Some(a),
        Mon::Once(_, _, a) => once_inner(a, id),
    }
}
