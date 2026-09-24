//! Die Schrittfunktion als Transitionssystem (plan/m6.md 2.8): je Maschine
//! Blatt, `FAULTED`, Abort-Latch, Aktivierungszaehler, Timer je Zustand,
//! Variablen und Signale; je Output der Latch. Ein Tick ist die
//! symbolische Ausfuehrung des Interpreters (`takt-interp/src/machine.rs`):
//! `loop:`-Bloecke der Kette, Uebergaenge outer-first, `resolve_m` mit
//! Fault-Pfad und `switch`, Abort-Phase, Zaehler, Commit. Zweige werden
//! mit `ite` gemischt, jede Zuweisung unter der Bedingung, dass die
//! Ausfuehrung sie noch erreicht (`alive`).
//!
//! **Reichweite ehrlich.** Was die Kodierung nicht abbildet — Stroeme,
//! Handler, Jobs, `every`, `at`, Sammlungen, Zeitoperatoren in Formeln —
//! meldet sie beim Namen statt es zu naehern; die Annahmen, die sie macht
//! (Inputs gueltig, kein i64-Ueberlauf, Tunables konstant), stehen als
//! Notizen im Export.

use std::collections::BTreeMap;
use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{
    BinaryOp, Builtin, CheckedKind, ConvertKind, Expr, ExprKind, Intrinsic, TProp, TemporalOp, UnaryOp,
};
use takt_mir::fns::BlockDef;
use takt_mir::machine::{FaultTarget, Machine, Target, TransTrigger};
use takt_mir::program::{Direction, Program, Property};
use takt_mir::stmt::{ArmPattern, Block, Method, Place, StmtKind};
use takt_mir::types::{Const, FloatWidth, HandleKind, Type};
use takt_mir::{BlockId, ChannelId, CommandId, MachineId, StateId, TypeId, VarId};

use crate::eval;
use crate::term::{Node, Op, Sort, Term};

/// Etwas, das die Kodierung nicht abbildet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    /// Was.
    pub what: String,
    /// Wo.
    pub span: Span,
}

type R<T> = Result<T, Unsupported>;

fn no<T>(what: impl Into<String>, span: Span) -> R<T> {
    Err(Unsupported { what: what.into(), span })
}

/// Eine Zustandsvariable des Systems.
#[derive(Clone, Debug)]
pub struct StateVar {
    /// Name `s.…`.
    pub name: String,
    /// Sorte.
    pub sort: Sort,
    /// Anfangswert ueber den Eingaben des Ticks 0.
    pub init: Term,
    /// Naechster Wert ueber dem Zustand und den Eingaben des naechsten Ticks.
    pub next: Term,
}

/// Eine Eigenschaft als Invariante ueber Zustand und Eingaben eines Ticks.
#[derive(Clone, Debug)]
pub struct Goal {
    /// Name.
    pub name: String,
    /// `assumption` statt `property`.
    pub assumption: bool,
    /// Die Formel.
    pub formula: Term,
}

/// Eine Pruefstelle (`check`/`expect`) als Beweisziel (plan/m6.md 2.8, B3):
/// „sie feuert nie".
#[derive(Clone, Debug)]
pub struct CheckSite {
    /// Anfang der Anweisung im Quelltext; so heisst die Stelle auch in der
    /// Coverage (`check @<start>`).
    pub start: u32,
    /// Position.
    pub span: Span,
    /// Die Maschine.
    pub machine: String,
    /// `check` oder `expect`.
    pub kind: String,
    /// Feuert im Tick 0, ueber den Eingaben des Ticks 0.
    pub init: Term,
    /// Feuert im Uebergang, ueber dem Zustand davor und den Eingaben danach.
    pub fires: Term,
}

/// Der Vertrag eines Blocks als Beweisziel (5.7, B2): aus jedem
/// typkonformen Zustand haelt ein Schritt unter `requires` sein `ensures`.
#[derive(Clone, Debug)]
pub struct ContractGoal {
    /// Der Block.
    pub block: String,
    /// Position des `step`.
    pub span: Span,
    /// Freie Variablen `c.…`: Parameter, Zustand, Schrittparameter.
    pub vars: Vec<(String, Sort)>,
    /// Erfuellbar genau dann, wenn der Vertrag verletzt sein kann.
    pub violation: Term,
}

/// Das Transitionssystem.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Zustand, nach Namen sortiert.
    pub state: Vec<StateVar>,
    /// Eingaben je Tick: `i.<channel>`, `i.cmd.<command>`, `i.now`.
    pub inputs: Vec<(String, Sort)>,
    /// Annahmen je Tick: Kanal-Ranges und `assumption`-Formeln (13.3).
    pub assumptions: Vec<Term>,
    /// Invarianten des Zustands aus den Typen (3.4): Ranges, Enums, Blaetter,
    /// Zaehler — sie gelten in jedem erreichbaren Zustand, weil ein Wert
    /// ausserhalb faultet statt gespeichert zu werden.
    pub invariants: Vec<Term>,
    /// Beweisziele.
    pub properties: Vec<Goal>,
    /// Pruefstellen als Beweisziele (B3).
    pub checks: Vec<CheckSite>,
    /// Block-Vertraege als Beweisziele (B2).
    pub contracts: Vec<ContractGoal>,
    /// Was die Kodierung annimmt oder auslaesst.
    pub notes: Vec<String>,
    /// Je Maschine die Zustandscodes mit Namen.
    pub leaves: BTreeMap<String, Vec<(i64, String)>>,
}

impl Model {
    /// Der Name eines Blattcodes.
    pub fn leaf_name(&self, machine: &str, code: i64) -> Option<&str> {
        self.leaves.get(machine)?.iter().find(|(c, _)| *c == code).map(|(_, n)| n.as_str())
    }

    /// Fuehrt das Modell konkret aus: `input(step, name)` liefert die
    /// Eingaben, fehlende sind null. Ergebnis: der Zustand je Schritt.
    pub fn simulate(&self, steps: u64, input: &dyn Fn(u64, &str) -> Option<eval::Val>) -> Vec<eval::Env> {
        let mut out = Vec::new();
        let mut env = eval::Env::new();
        for (name, sort) in &self.inputs {
            env.insert(name.clone(), input(0, name).unwrap_or(eval::Val::zero(*sort)));
        }
        let mut state: eval::Env = self.state.iter().map(|v| (v.name.clone(), eval::eval(&v.init, &env))).collect();
        out.push(state.clone());
        for k in 1..=steps {
            let mut env = state.clone();
            for (name, sort) in &self.inputs {
                env.insert(name.clone(), input(k, name).unwrap_or(eval::Val::zero(*sort)));
            }
            state = self.state.iter().map(|v| (v.name.clone(), eval::eval(&v.next, &env))).collect();
            out.push(state.clone());
        }
        out
    }
}

/// Werte der Orte.
type Env = BTreeMap<String, Term>;

fn ite_env(c: &Term, a: &Env, b: &Env) -> Env {
    a.iter()
        .map(|(k, va)| {
            let vb = b.get(k).unwrap_or(va);
            (k.clone(), Term::ite(c.clone(), va.clone(), vb.clone()))
        })
        .collect()
}

/// Wie ein Block endet.
#[derive(Clone, Debug)]
enum ExitKind {
    /// `check` verletzt, mit explizitem Ziel oder dem Fault-Ziel des Blatts.
    Fault(Option<Target>),
    /// `abort`.
    Abort,
    /// `-> q`.
    Goto(Target),
}

#[derive(Clone, Debug)]
struct Exit {
    cond: Term,
    kind: ExitKind,
}

/// Kontrollfluss eines Blocks: `alive` heisst, die Ausfuehrung erreicht
/// die naechste Anweisung; die Ausgaenge sind paarweise exklusiv.
#[derive(Clone, Debug)]
struct Flow {
    alive: Term,
    exits: Vec<Exit>,
}

impl Flow {
    fn new(alive: Term) -> Flow {
        Flow { alive, exits: Vec::new() }
    }
}

/// Modus eines Blocks (9.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Run,
    Entry,
}

/// Kontext einer Auswertung.
struct Cx<'a> {
    /// Die Maschine; `None` in einer Eigenschaft.
    m: Option<MachineId>,
    /// Das Blatt, dessen Zweig entsteht.
    leaf: Option<StateId>,
    mode: Mode,
    /// Zustand am Tick-Anfang (Ψ_k, committete Outputs).
    pre: &'a Env,
    /// Ist eine Maschine in diesem Tick aktiv (fuer frische Lesevorgaenge).
    active: &'a BTreeMap<MachineId, Term>,
    /// Lokale einer eingebetteten Funktion.
    locals: Option<BTreeMap<VarId, Term>>,
}

impl Cx<'_> {
    /// Derselbe Kontext im Entry-Modus (9.3).
    fn entry(&self) -> Cx<'_> {
        Cx {
            m: self.m,
            leaf: self.leaf,
            mode: Mode::Entry,
            pre: self.pre,
            active: self.active,
            locals: self.locals.clone(),
        }
    }
}

/// Der Kodierer.
struct Enc<'p> {
    p: &'p Program,
    order: Vec<MachineId>,
    inputs: BTreeMap<String, Sort>,
    notes: Vec<String>,
    /// `abort` in diesem Tick, je Stelle.
    aborts: Vec<Term>,
    /// Feuerbedingungen der Pruefstellen der laufenden Phase, je Anfang.
    sites: BTreeMap<u32, Vec<Term>>,
    /// Position, Maschine und Art je Pruefstelle.
    site_info: BTreeMap<u32, (Span, String, String)>,
}

/// Kodiert ein Programm.
pub fn encode(p: &Program) -> R<Model> {
    let order = takt_mir::analysis::schedule::order(p).unwrap_or_else(|_| takt_mir::analysis::schedule::runnable(p));
    let mut enc = Enc {
        p,
        order,
        inputs: BTreeMap::new(),
        notes: Vec::new(),
        aborts: Vec::new(),
        sites: BTreeMap::new(),
        site_info: BTreeMap::new(),
    };
    enc.check_reach()?;
    let init = enc.init()?;
    let init_sites = std::mem::take(&mut enc.sites);
    let pre: Env = init.keys().map(|k| (k.clone(), Term::var(k.clone(), init[k].sort()))).collect();
    let next = enc.tick(&pre)?;
    let tick_sites = std::mem::take(&mut enc.sites);
    let invariants = enc.state_invariants(&pre);
    let contracts = enc.contract_goals()?;
    let checks: Vec<CheckSite> = enc
        .site_info
        .iter()
        .map(|(start, (span, machine, kind))| CheckSite {
            start: *start,
            span: *span,
            machine: machine.clone(),
            kind: kind.clone(),
            init: Term::or(init_sites.get(start).cloned().unwrap_or_default()),
            fires: Term::or(tick_sites.get(start).cloned().unwrap_or_default()),
        })
        .collect();
    let mut properties = Vec::new();
    let mut assumptions = enc.channel_assumptions()?;
    for prop in &p.properties {
        match enc.goal(prop, &pre)? {
            Some(goal) => {
                if goal.assumption {
                    assumptions.push(goal.formula.clone());
                }
                properties.push(goal);
            }
            None => enc
                .notes
                .push(format!("`{}` nicht kodiert: nur `always(…)`/`never(…)` ohne Zeitoperatoren (2.8)", prop.name)),
        }
    }
    let state = init
        .iter()
        .map(|(name, i)| StateVar { name: name.clone(), sort: i.sort(), init: i.clone(), next: next[name].clone() })
        .collect();
    let mut leaves = BTreeMap::new();
    for &id in &enc.order {
        let m = &p.machines[id.index()];
        let mut codes: Vec<(i64, String)> =
            m.states.iter().enumerate().map(|(i, s)| (enc.code(id, StateId(i as u32)), s.name.clone())).collect();
        if let Some(c) = enc.faulted_code(id) {
            codes.push((c, "FAULTED".to_string()));
        }
        leaves.insert(m.name.clone(), codes);
    }
    let mut notes = enc.notes;
    notes.sort();
    notes.dedup();
    Ok(Model {
        state,
        inputs: enc.inputs.into_iter().collect(),
        assumptions,
        invariants,
        properties,
        checks,
        contracts,
        notes,
        leaves,
    })
}

impl Enc<'_> {
    fn machine(&self, m: MachineId) -> &Machine {
        &self.p.machines[m.index()]
    }

    fn note(&mut self, text: &str) {
        if !self.notes.iter().any(|n| n == text) {
            self.notes.push(text.to_string());
        }
    }

    /// Was das Modell grundsaetzlich nicht abbildet.
    fn check_reach(&self) -> R<()> {
        for &id in &self.order {
            let m = self.machine(id);
            if !m.handlers.is_empty() || m.states.iter().any(|s| !s.handlers.is_empty()) {
                return no("Handler auf Stroemen", m.span);
            }
            if !m.layout.job_slots.is_empty() {
                return no("Jobs", m.span);
            }
            if !m.faulted.transitions.is_empty() {
                return no("Uebergaenge aus FAULTED", m.span);
            }
            if m.states.iter().any(|s| !s.instances.is_empty()) {
                return no("gescopte Instanzen", m.span);
            }
        }
        if !self.p.streams.is_empty() {
            return no("interne Stroeme", Span::default());
        }
        Ok(())
    }

    fn sort_of(&self, ty: TypeId, span: Span) -> R<Sort> {
        match self.p.types.get(ty) {
            Type::Bool => Ok(Sort::Bool),
            Type::Int { .. } | Type::Duration { .. } | Type::Enum(_) => Ok(Sort::Int),
            Type::Float { width: FloatWidth::F32, .. } => Ok(Sort::F32),
            Type::Float { width: FloatWidth::F64, .. } => Ok(Sort::F64),
            other => no(format!("Typ {other:?}"), span),
        }
    }

    fn zero(sort: Sort) -> Term {
        match sort {
            Sort::Bool => Term::bool(false),
            Sort::Int => Term::int(0),
            s => Term::float(0.0, s),
        }
    }

    // ------------------------------------------------------------ Orte

    fn loc_leaf(&self, m: MachineId) -> String {
        format!("s.{}.leaf", self.machine(m).name)
    }
    fn loc_faulted(&self, m: MachineId) -> String {
        format!("s.{}.faulted", self.machine(m).name)
    }
    fn loc_latched(&self, m: MachineId) -> String {
        format!("s.{}.latched", self.machine(m).name)
    }
    fn loc_countdown(&self, m: MachineId) -> String {
        format!("s.{}.countdown", self.machine(m).name)
    }
    fn loc_timer(&self, m: MachineId, s: StateId) -> String {
        format!("s.{}.t.{}", self.machine(m).name, self.machine(m).states[s.index()].name)
    }
    fn loc_var(&self, m: MachineId, v: VarId) -> String {
        format!("s.{}.v.{}", self.machine(m).name, self.machine(m).vars[v.index()].name)
    }
    fn loc_sig(&self, m: MachineId, s: usize) -> String {
        format!("s.{}.sig.{}", self.machine(m).name, self.machine(m).signals[s].name)
    }
    fn loc_out(&self, c: ChannelId) -> String {
        format!("s.out.{}", self.p.channels[c.index()].name)
    }

    fn input(&mut self, name: String, sort: Sort) -> Term {
        self.inputs.insert(name.clone(), sort);
        Term::var(name, sort)
    }

    /// Der Code eines Zustands: seine Variante in `<m>.State`, sonst sein Index.
    fn code(&self, m: MachineId, s: StateId) -> i64 {
        let machine = self.machine(m);
        let name = &machine.states[s.index()].name;
        self.p
            .enums
            .iter()
            .find(|e| e.name == format!("{}.State", machine.name))
            .and_then(|e| e.variants.iter().position(|v| v.name == *name))
            .map_or(s.index() as i64, |i| i as i64)
    }

    fn faulted_code(&self, m: MachineId) -> Option<i64> {
        let machine = self.machine(m);
        let e = self.p.enums.iter().find(|e| e.name == format!("{}.State", machine.name))?;
        e.variants.iter().position(|v| v.name == "FAULTED").map(|i| i as i64)
    }

    fn chain_to(&self, m: MachineId, s: StateId) -> Vec<StateId> {
        let machine = self.machine(m);
        let mut out = Vec::new();
        let mut cur = Some(s);
        while let Some(id) = cur {
            out.push(id);
            cur = machine.states[id.index()].parent;
        }
        out.reverse();
        out
    }

    /// Die Kette bis zum Anfangsblatt unter `s`.
    fn descend(&self, m: MachineId, s: StateId) -> Vec<StateId> {
        let machine = self.machine(m);
        let mut out = self.chain_to(m, s);
        let mut cur = s;
        while let Some(i) = machine.states[cur.index()].initial {
            out.push(i);
            cur = i;
        }
        out
    }

    fn leaves(&self, m: MachineId) -> Vec<StateId> {
        self.machine(m)
            .states
            .iter()
            .enumerate()
            .filter(|(_, s)| s.children.is_empty())
            .map(|(i, _)| StateId(i as u32))
            .collect()
    }

    fn fault_target(&self, m: MachineId, leaf: StateId) -> Target {
        match self.machine(m).fault_target_of(leaf) {
            FaultTarget::State(s) => Target::State(s),
            FaultTarget::Faulted => Target::Faulted,
        }
    }

    // ------------------------------------------------------------ Ausdruecke

    /// Ein konstanter Ausdruck (Parameter-Defaults, `safe`, `after`).
    fn const_expr(&mut self, e: &Expr) -> R<Term> {
        let pre = Env::new();
        let active = BTreeMap::new();
        let cx = Cx { m: None, leaf: None, mode: Mode::Entry, pre: &pre, active: &active, locals: None };
        let mut flow = Flow::new(Term::bool(true));
        let t = self.expr(e, &cx, &pre, &mut flow)?;
        if !flow.exits.is_empty() || has_var(&t) {
            return no("kein konstanter Ausdruck", e.span);
        }
        Ok(t)
    }

    fn const_int(&mut self, e: &Expr) -> R<i64> {
        let t = self.const_expr(e)?;
        match eval::eval(&t, &eval::Env::new()) {
            eval::Val::Int(i) => Ok(i),
            _ => no("Ganzzahl erwartet", e.span),
        }
    }

    fn expr(&mut self, e: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Term> {
        let span = e.span;
        let sort = self.sort_of(e.ty, span);
        Ok(match &e.kind {
            ExprKind::Bool(b) => Term::bool(*b),
            ExprKind::Int(i) | ExprKind::Duration(i) => Term::int(*i),
            ExprKind::Float(f) => Term::float(*f, sort?),
            ExprKind::Default => Enc::zero(sort?),
            ExprKind::Variant { variant, fields, .. } => {
                if !fields.is_empty() {
                    return no("Variante mit Feldern", span);
                }
                Term::int(i64::from(*variant))
            }
            ExprKind::Var(v) => {
                if let Some(locals) = &cx.locals {
                    if let Some(t) = locals.get(v) {
                        return Ok(t.clone());
                    }
                }
                let Some(m) = cx.m else { return no("Variable ausserhalb einer Maschine", span) };
                env.get(&self.loc_var(m, *v)).cloned().ok_or_else(|| Unsupported { what: "Variable".into(), span })?
            }
            ExprKind::Param(id) => {
                let param = &self.p.params[id.index()];
                if param.tunable {
                    self.note("Tunables gelten als Konstanten (ihr Default)");
                }
                let default = param.default.clone();
                self.const_expr(&default)?
            }
            ExprKind::Command(c) => self.command(*c),
            ExprKind::Input { channel, .. } => {
                let ch = &self.p.channels[channel.index()];
                let sort = self.sort_of(ch.ty, span)?;
                self.input(format!("i.{}", ch.name), sort)
            }
            ExprKind::Output(c) => {
                let own = cx.m.is_some() && self.p.channels[c.index()].owner == cx.m;
                let loc = self.loc_out(*c);
                let src = if own { env } else { cx.pre };
                src.get(&loc).cloned().ok_or_else(|| Unsupported { what: "Output".into(), span })?
            }
            ExprKind::Published { machine, var } => {
                if machine.index.is_some() {
                    return no("Instanz-Array", span);
                }
                let loc = self.loc_var(machine.machine, *var);
                self.psi(cx, env, machine.machine, &loc, span)?
            }
            ExprKind::StateOf(machine) => {
                if machine.index.is_some() {
                    return no("Instanz-Array", span);
                }
                let loc = self.loc_leaf(machine.machine);
                self.psi(cx, env, machine.machine, &loc, span)?
            }
            ExprKind::Signal { machine, signal } => {
                if machine.index.is_some() {
                    return no("Instanz-Array", span);
                }
                let loc = self.loc_sig(machine.machine, signal.index());
                self.psi(cx, env, machine.machine, &loc, span)?
            }
            ExprKind::Builtin(b) => match b {
                Builtin::Tick => Term::int(self.p.config.tick),
                Builtin::Now => {
                    self.note("`now` ist eine freie Eingabe je Tick");
                    self.input("i.now".to_string(), Sort::Int)
                }
                Builtin::TimeInState => {
                    let (Some(m), Some(leaf)) = (cx.m, cx.leaf) else { return no("`time_in_state`", span) };
                    let period = i64::from(self.machine(m).period.max(1)).saturating_mul(self.p.config.tick);
                    let timer = env.get(&self.loc_timer(m, leaf)).cloned().expect("Timer");
                    Term::bin(Op::Mul, timer, Term::int(period))
                }
                other => return no(format!("`{other:?}`"), span),
            },
            ExprKind::Unary { op, expr } => {
                let x = self.expr(expr, cx, env, flow)?;
                match (op, x.sort()) {
                    (UnaryOp::Not, _) => x.not(),
                    (UnaryOp::Neg, Sort::Int) => Term::app(Op::Neg, vec![x]),
                    (UnaryOp::Neg, _) => Term::app(Op::FNeg, vec![x]),
                    (UnaryOp::BitNot, _) => return no("`~`", span),
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.expr(lhs, cx, env, flow)?;
                let b = self.expr(rhs, cx, env, flow)?;
                self.binary(*op, a, b, span)?
            }
            ExprKind::Cond { cond, then, otherwise } => {
                let c = self.expr(cond, cx, env, flow)?;
                let a = self.expr(then, cx, env, flow)?;
                let b = self.expr(otherwise, cx, env, flow)?;
                Term::ite(c, a, b)
            }
            ExprKind::Checked { expr, kind } => {
                let x = self.expr(expr, cx, env, flow)?;
                self.checked(kind, x, span, cx, flow)?
            }
            ExprKind::Convert { expr, kind, .. } => {
                let x = self.expr(expr, cx, env, flow)?;
                match kind {
                    ConvertKind::ToFloat => Term::app(if sort? == Sort::F32 { Op::ToF32 } else { Op::ToF64 }, vec![x]),
                    ConvertKind::As => x,
                    ConvertKind::To => return no("Einheitenumrechnung `.to`", span),
                }
            }
            ExprKind::Cast { expr, .. } => {
                let x = self.expr(expr, cx, env, flow)?;
                match (x.sort(), sort?) {
                    (Sort::Int, Sort::Int) => x,
                    (Sort::Int, Sort::F32) => Term::app(Op::ToF32, vec![x]),
                    (Sort::Int, Sort::F64) => Term::app(Op::ToF64, vec![x]),
                    (a, b) if a == b => x,
                    _ => return no("Konversion", span),
                }
            }
            ExprKind::Intrinsic { op, args } => {
                let mut xs = Vec::new();
                for a in args {
                    xs.push(self.expr(a, cx, env, flow)?);
                }
                self.intrinsic(*op, xs, span)?
            }
            ExprKind::Call { callee, args } => {
                let mut xs = Vec::new();
                for a in args {
                    xs.push(self.expr(a, cx, env, flow)?);
                }
                self.call(*callee, xs, cx, env, flow, span)?
            }
            other => return no(format!("Ausdruck {}", node_name(other)), span),
        })
    }

    fn command(&mut self, c: CommandId) -> Term {
        let name = format!("i.cmd.{}", self.p.commands[c.index()].name);
        self.input(name, Sort::Bool)
    }

    /// Ψ eines anderen Maschinenwerts: Follower lesen frisch, was in diesem
    /// Tick schon lief; sonst gilt Ψ_k (7.2).
    fn psi(&mut self, cx: &Cx<'_>, env: &Env, target: MachineId, loc: &str, span: Span) -> R<Term> {
        let old = cx.pre.get(loc).cloned().ok_or_else(|| Unsupported { what: "Ψ".into(), span })?;
        let fresh = cx.m.is_some_and(|m| self.machine(m).follows.contains(&target));
        if !fresh {
            return Ok(old);
        }
        let new = env.get(loc).cloned().unwrap_or_else(|| old.clone());
        let active = cx.active.get(&target).cloned().unwrap_or_else(|| Term::bool(false));
        Ok(Term::ite(active, new, old))
    }

    fn binary(&mut self, op: BinaryOp, a: Term, b: Term, span: Span) -> R<Term> {
        let float = matches!(a.sort(), Sort::F32 | Sort::F64);
        Ok(match op {
            BinaryOp::And => Term::and(vec![a, b]),
            BinaryOp::Or => Term::or(vec![a, b]),
            BinaryOp::Eq if float => Term::bin(Op::FEq, a, b),
            BinaryOp::Eq => Term::eq(a, b),
            BinaryOp::Ne if float => Term::bin(Op::FEq, a, b).not(),
            BinaryOp::Ne => Term::eq(a, b).not(),
            BinaryOp::Lt => Term::bin(if float { Op::FLt } else { Op::Lt }, a, b),
            BinaryOp::Le => Term::bin(if float { Op::FLe } else { Op::Le }, a, b),
            BinaryOp::Gt => Term::bin(if float { Op::FGt } else { Op::Gt }, a, b),
            BinaryOp::Ge => Term::bin(if float { Op::FGe } else { Op::Ge }, a, b),
            BinaryOp::Add => Term::bin(if float { Op::FAdd } else { Op::Add }, a, b),
            BinaryOp::Sub => Term::bin(if float { Op::FSub } else { Op::Sub }, a, b),
            BinaryOp::Mul => Term::bin(if float { Op::FMul } else { Op::Mul }, a, b),
            BinaryOp::Div => Term::bin(if float { Op::FDiv } else { Op::Div }, a, b),
            BinaryOp::Rem if !float => Term::bin(Op::Rem, a, b),
            BinaryOp::BitAnd => Term::bin(Op::BitAnd, a, b),
            BinaryOp::BitOr => Term::bin(Op::BitOr, a, b),
            BinaryOp::BitXor => Term::bin(Op::BitXor, a, b),
            BinaryOp::Shl => Term::bin(Op::Shl, a, b),
            BinaryOp::Shr => Term::bin(Op::Shr, a, b),
            other => return no(format!("Operator `{other:?}`"), span),
        })
    }

    fn intrinsic(&mut self, op: Intrinsic, xs: Vec<Term>, span: Span) -> R<Term> {
        let float = xs.first().is_some_and(|x| matches!(x.sort(), Sort::F32 | Sort::F64));
        Ok(match (op, xs.as_slice()) {
            (Intrinsic::Abs, [x]) if float => Term::app(Op::FAbs, vec![x.clone()]),
            (Intrinsic::Abs, [x]) => {
                let neg = Term::bin(Op::Lt, x.clone(), Term::int(0));
                Term::ite(neg, Term::app(Op::Neg, vec![x.clone()]), x.clone())
            }
            (Intrinsic::Min | Intrinsic::Max, [a, b]) => {
                let lt = Term::bin(if float { Op::FLt } else { Op::Lt }, a.clone(), b.clone());
                if op == Intrinsic::Min {
                    Term::ite(lt, a.clone(), b.clone())
                } else {
                    Term::ite(lt, b.clone(), a.clone())
                }
            }
            (Intrinsic::Sqrt, [x]) if float => Term::app(Op::FSqrt, vec![x.clone()]),
            (Intrinsic::Fma, [a, b, c]) if float => Term::app(Op::FFma, vec![a.clone(), b.clone(), c.clone()]),
            _ => return no(format!("Primitive `{op:?}`"), span),
        })
    }

    /// Eine implizite Pruefung (4.1): Range und Division sind Fault-Zweige,
    /// Nichtendlichkeit ebenso; Ueberlauf und Gueltigkeit sind Annahmen.
    fn checked(&mut self, kind: &CheckedKind, x: Term, span: Span, cx: &Cx<'_>, flow: &mut Flow) -> R<Term> {
        let fail = match kind {
            // Die Intervallanalyse hat sie bewiesen (3.4).
            CheckedKind::Range(r) if r.origin == takt_mir::types::RangeOrigin::Proven => return Ok(x),
            CheckedKind::Range(r) => {
                let (lo, hi) = (self.bound(&r.lo, x.sort()), self.bound(&r.hi, x.sort()));
                let (ge, le) = if x.sort() == Sort::Int { (Op::Ge, Op::Le) } else { (Op::FGe, Op::FLe) };
                Term::and(vec![Term::bin(ge, x.clone(), lo), Term::bin(le, x.clone(), hi)]).not()
            }
            CheckedKind::DivZero => Term::eq(x.clone(), Term::int(0)),
            CheckedKind::NonFinite => Term::app(Op::IsFinite, vec![x.clone()]).not(),
            CheckedKind::Overflow | CheckedKind::Shift | CheckedKind::Convert => {
                self.note("Ganzzahlueberlauf (i64) und Schiebebetraege nicht modelliert");
                return Ok(x);
            }
            CheckedKind::Domain => {
                self.note("Definitionsbereich von `sqrt`/`log` nicht modelliert");
                return Ok(x);
            }
            CheckedKind::Valid => {
                self.note("Inputs gelten als gueltig (3.5)");
                return Ok(x);
            }
            CheckedKind::Missing | CheckedKind::Index { .. } => return no("Wrapper oder Index", span),
        };
        let cond = Term::and(vec![flow.alive.clone(), fail.clone()]);
        // Eine Pruefstelle (11.3): `takt prove` zeigt, ob sie je faultet.
        self.sites.entry(span.start).or_default().push(cond.clone());
        if let Some(m) = cx.m {
            let name = takt_mir::analysis::walk::name(kind).to_string();
            self.site_info.insert(span.start, (span, self.machine(m).name.clone(), name));
        }
        flow.exits.push(Exit { cond, kind: ExitKind::Fault(None) });
        flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        Ok(x)
    }

    fn bound(&self, c: &Const, sort: Sort) -> Term {
        match (c, sort) {
            (Const::Int(i) | Const::Duration(i), Sort::Int) => Term::int(*i),
            (Const::Int(i) | Const::Duration(i), s) => Term::float(*i as f64, s),
            (Const::Float(f), Sort::Int) => Term::int(*f as i64),
            (Const::Float(f), s) => Term::float(*f, s),
            (Const::Bool(b), _) => Term::bool(*b),
        }
    }

    /// Ein Aufruf einer reinen Funktion (4.4), eingebettet.
    fn call(
        &mut self,
        callee: takt_mir::FnId,
        args: Vec<Term>,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<Term> {
        let f = self.p.fns[callee.index()].clone();
        let Some(ret) = f.ret else { return no("Funktion ohne Rueckgabe", span) };
        if f.params.iter().any(|p| p.inout) {
            return no("`inout`-Parameter", span);
        }
        let mut locals: BTreeMap<VarId, Term> = BTreeMap::new();
        for (i, local) in f.locals.iter().enumerate() {
            let value = match args.get(i) {
                Some(a) => a.clone(),
                None => Enc::zero(self.sort_of(local.ty, local.span)?),
            };
            locals.insert(VarId(i as u32), value);
        }
        let inner =
            Cx { m: cx.m, leaf: cx.leaf, mode: Mode::Entry, pre: cx.pre, active: cx.active, locals: Some(locals) };
        let mut sub = Flow::new(flow.alive.clone());
        let mut ret_val = Enc::zero(self.sort_of(ret, span)?);
        let mut env = env.clone();
        let mut inner = inner;
        self.fn_block(&f.body, &mut inner, &mut env, &mut sub, &mut ret_val)?;
        // Die Funktion kehrt zurueck; nur ihre Faults beenden den Aufrufer.
        let faults = Term::or(sub.exits.iter().map(|x| x.cond.clone()).collect());
        flow.exits.extend(sub.exits);
        flow.alive = Term::and(vec![flow.alive.clone(), !faults]);
        Ok(ret_val)
    }

    /// Der Rumpf einer Funktion: Zuweisungen an Lokale, `if`, `return`.
    fn fn_block(&mut self, b: &Block, cx: &mut Cx<'_>, env: &mut Env, flow: &mut Flow, ret: &mut Term) -> R<()> {
        for s in &b.stmts {
            if flow.alive.is_bool(false) {
                break;
            }
            match &s.kind {
                StmtKind::Return(e) => {
                    let v = self.expr(e, cx, env, flow)?;
                    *ret = Term::ite(flow.alive.clone(), v, ret.clone());
                    flow.alive = Term::bool(false);
                }
                StmtKind::Assign { target: Place::Var(v), value } => {
                    let val = self.expr(value, cx, env, flow)?;
                    let locals = cx.locals.as_mut().expect("Lokale");
                    let old = locals.get(v).cloned().unwrap_or_else(|| Enc::zero(val.sort()));
                    locals.insert(*v, Term::ite(flow.alive.clone(), val, old));
                }
                StmtKind::If { cond, then, otherwise } => {
                    let c = self.expr(cond, cx, env, flow)?;
                    let saved = cx.locals.clone();
                    let mut ft = Flow::new(Term::and(vec![flow.alive.clone(), c.clone()]));
                    let mut rt = ret.clone();
                    self.fn_block(then, cx, env, &mut ft, &mut rt)?;
                    let then_locals = cx.locals.replace(saved.clone().expect("Lokale"));
                    let mut fe = Flow::new(Term::and(vec![flow.alive.clone(), c.clone().not()]));
                    let mut re = ret.clone();
                    self.fn_block(otherwise, cx, env, &mut fe, &mut re)?;
                    let else_locals = cx.locals.take().expect("Lokale");
                    let then_locals = then_locals.expect("Lokale");
                    let merged: BTreeMap<VarId, Term> = then_locals
                        .iter()
                        .map(|(k, a)| {
                            (
                                *k,
                                Term::ite(
                                    c.clone(),
                                    a.clone(),
                                    else_locals.get(k).cloned().unwrap_or_else(|| a.clone()),
                                ),
                            )
                        })
                        .collect();
                    cx.locals = Some(merged);
                    *ret = Term::ite(c, rt, re);
                    flow.exits.extend(ft.exits);
                    flow.exits.extend(fe.exits);
                    flow.alive = Term::or(vec![ft.alive, fe.alive]);
                }
                StmtKind::Pass | StmtKind::Observe(_) => {}
                other => return no(format!("Anweisung {} in einer Funktion", stmt_name(other)), s.span),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ Anweisungen

    fn block(&mut self, b: &Block, cx: &Cx<'_>, env: &mut Env, flow: &mut Flow) -> R<()> {
        let m = cx.m.expect("Maschine");
        for s in &b.stmts {
            if flow.alive.is_bool(false) {
                break;
            }
            let span = s.span;
            match &s.kind {
                StmtKind::Assign { target, value } => {
                    let v = self.expr(value, cx, env, flow)?;
                    let loc = match target {
                        Place::Var(id) => self.loc_var(m, *id),
                        Place::Output(c) => self.loc_out(*c),
                        _ => return no("Zuweisung an Feld oder Element", span),
                    };
                    let old = env.get(&loc).cloned().ok_or_else(|| Unsupported { what: "Ort".into(), span })?;
                    env.insert(loc, Term::ite(flow.alive.clone(), v, old));
                }
                StmtKind::Check { cond, confirm, within, target, kind, .. } => {
                    if confirm.is_some() || within.is_some() {
                        return no("`check … for` oder `within`", span);
                    }
                    let c = self.expr(cond, cx, env, flow)?;
                    let fail = Term::and(vec![flow.alive.clone(), c.clone().not()]);
                    let word = if *kind == takt_mir::stmt::CheckKind::Check { "check" } else { "expect" };
                    self.sites.entry(span.start).or_default().push(fail.clone());
                    self.site_info.insert(span.start, (span, self.machine(m).name.clone(), word.to_string()));
                    flow.exits.push(Exit { cond: fail, kind: ExitKind::Fault(*target) });
                    flow.alive = Term::and(vec![flow.alive.clone(), c]);
                }
                StmtKind::Goto(t) => {
                    if cx.mode == Mode::Run {
                        flow.exits.push(Exit { cond: flow.alive.clone(), kind: ExitKind::Goto(*t) });
                        flow.alive = Term::bool(false);
                    }
                }
                StmtKind::Abort { .. } => {
                    self.aborts.push(flow.alive.clone());
                    flow.exits.push(Exit { cond: flow.alive.clone(), kind: ExitKind::Abort });
                    flow.alive = Term::bool(false);
                }
                StmtKind::If { cond, then, otherwise } => {
                    let c = self.expr(cond, cx, env, flow)?;
                    let mut env_t = env.clone();
                    let mut ft = Flow::new(Term::and(vec![flow.alive.clone(), c.clone()]));
                    self.block(then, cx, &mut env_t, &mut ft)?;
                    let mut env_e = env.clone();
                    let mut fe = Flow::new(Term::and(vec![flow.alive.clone(), c.clone().not()]));
                    self.block(otherwise, cx, &mut env_e, &mut fe)?;
                    *env = ite_env(&c, &env_t, &env_e);
                    flow.exits.extend(ft.exits);
                    flow.exits.extend(fe.exits);
                    flow.alive = Term::or(vec![ft.alive, fe.alive]);
                }
                StmtKind::Match { subject, arms } => {
                    let subj = self.expr(subject, cx, env, flow)?;
                    let mut remaining = flow.alive.clone();
                    let mut merged = env.clone();
                    let mut alive_out = Term::bool(false);
                    for arm in arms {
                        let cond = match &arm.pattern {
                            ArmPattern::Variant { variant, fields } if fields.is_empty() => {
                                Term::eq(subj.clone(), Term::int(i64::from(*variant)))
                            }
                            ArmPattern::Wild => Term::bool(true),
                            _ => return no("`case` mit Werten oder Feldern", arm.span),
                        };
                        let take = Term::and(vec![remaining.clone(), cond.clone()]);
                        let mut env_a = env.clone();
                        let mut fa = Flow::new(take.clone());
                        self.block(&arm.body, cx, &mut env_a, &mut fa)?;
                        merged = ite_env(&take, &env_a, &merged);
                        alive_out = Term::or(vec![alive_out, fa.alive]);
                        flow.exits.extend(fa.exits);
                        remaining = Term::and(vec![remaining, cond.not()]);
                    }
                    *env = merged;
                    flow.alive = Term::or(vec![alive_out, remaining]);
                }
                StmtKind::ForRange { var, count, body } => {
                    let n = self.const_int(count)?;
                    let loc = self.loc_var(m, *var);
                    for i in 0..n.max(0) {
                        env.insert(loc.clone(), Term::int(i));
                        self.block(body, cx, env, flow)?;
                    }
                }
                StmtKind::Raise(sig) => {
                    let loc = self.loc_sig(m, sig.index());
                    let old = env.get(&loc).cloned().unwrap_or_else(|| Term::bool(false));
                    env.insert(loc, Term::ite(flow.alive.clone(), Term::bool(true), old));
                }
                StmtKind::MethodCall { target, receiver, method, args } => {
                    self.method_call(target.as_ref(), receiver, *method, args, cx, env, flow, span)?;
                }
                StmtKind::Observe(_) | StmtKind::Pass => {}
                other => return no(format!("Anweisung {}", stmt_name(other)), span),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ Bloecke (5.7)

    /// Block und Felder (`s.<m>.v.<instanz>.<feld>`: Parameter, dann
    /// Zustand) einer Instanz; keine bei Instanz-Arrays.
    fn block_fields(&self, m: MachineId, inst: VarId) -> Option<(BlockId, Vec<(String, TypeId)>)> {
        let machine = self.machine(m);
        let bi = machine.layout.block_instances.iter().find(|b| b.var == inst)?;
        if bi.count > 1 {
            return None;
        }
        let def = &self.p.blocks[bi.block.index()];
        let base = format!("s.{}.v.{}", machine.name, machine.vars[inst.index()].name);
        let fields = def
            .params
            .iter()
            .map(|p| (format!("{base}.{}", p.name), p.ty))
            .chain(def.state_vars.iter().map(|v| (format!("{base}.{}", v.name), v.ty)))
            .collect();
        Some((bi.block, fields))
    }

    /// Setzt den Zustand einer Instanz aus ihren Parametern (`reset`, 5.7).
    fn block_reset(
        &mut self,
        def: &BlockDef,
        fields: &[(String, TypeId)],
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
    ) -> R<()> {
        let mut locals: BTreeMap<VarId, Term> = BTreeMap::new();
        for i in 0..def.params.len() {
            locals.insert(VarId(i as u32), env[&fields[i].0].clone());
        }
        for (j, sv) in def.state_vars.iter().enumerate() {
            let icx = Cx {
                m: cx.m,
                leaf: cx.leaf,
                mode: cx.mode,
                pre: cx.pre,
                active: cx.active,
                locals: Some(locals.clone()),
            };
            let v = match &sv.init {
                Some(e) => self.expr(e, &icx, env, flow)?,
                None => Enc::zero(self.sort_of(sv.ty, sv.span)?),
            };
            let id = VarId((def.params.len() + j) as u32);
            locals.insert(id, v.clone());
            let loc = fields[def.params.len() + j].0.clone();
            let old = env[&loc].clone();
            env.insert(loc, Term::ite(flow.alive.clone(), v, old));
        }
        Ok(())
    }

    /// `b.step(args)`, `b.reset()`, `b.<methode>(args)`: der Rumpf eingebettet,
    /// der Zustand der Instanz in den Feldern; `requires` des `step` ist eine
    /// Pruefstelle ohne Laufzeitpruefung (5.7).
    #[allow(clippy::too_many_arguments)]
    fn method_call(
        &mut self,
        target: Option<&Place>,
        receiver: &Place,
        method: Method,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let m = cx.m.expect("Maschine");
        let Place::Var(inst) = receiver else { return no("Methodenaufruf auf diesem Ziel", span) };
        let Some((bid, fields)) = self.block_fields(m, *inst) else { return no("Methodenaufruf", span) };
        let def = self.p.blocks[bid.index()].clone();
        let mut arg_terms = Vec::new();
        for a in args {
            arg_terms.push(self.expr(a, cx, env, flow)?);
        }
        let fid = match method {
            Method::Reset => return self.block_reset(&def, &fields, cx, env, flow),
            Method::Step => def.step.ok_or_else(|| Unsupported { what: "`step`".into(), span })?,
            Method::Block(f) => f,
            _ => return no("Sammlungsmethode", span),
        };
        let f = self.p.fns[fid.index()].clone();
        let base = fields.len() as u32;
        let mut locals: BTreeMap<VarId, Term> = BTreeMap::new();
        for (i, (loc, _)) in fields.iter().enumerate() {
            locals.insert(VarId(i as u32), env[loc].clone());
        }
        for (i, local) in f.locals.iter().enumerate() {
            let value = match arg_terms.get(i) {
                Some(a) => a.clone(),
                None => Enc::zero(self.sort_of(local.ty, local.span)?),
            };
            locals.insert(VarId(base + i as u32), value);
        }
        let call_alive = flow.alive.clone();
        if method == Method::Step {
            let rcx = Cx {
                m: cx.m,
                leaf: cx.leaf,
                mode: cx.mode,
                pre: cx.pre,
                active: cx.active,
                locals: Some(locals.clone()),
            };
            for r in &def.requires {
                let mut rflow = Flow::new(call_alive.clone());
                let c = self.expr(r, &rcx, env, &mut rflow)?;
                let fail = Term::and(vec![call_alive.clone(), !c]);
                self.sites.entry(span.start).or_default().push(fail);
                self.site_info.insert(span.start, (span, self.machine(m).name.clone(), "requires".to_string()));
            }
        }
        let mut icx =
            Cx { m: cx.m, leaf: cx.leaf, mode: Mode::Entry, pre: cx.pre, active: cx.active, locals: Some(locals) };
        let mut sub = Flow::new(call_alive.clone());
        let mut ret = match f.ret {
            Some(t) => Enc::zero(self.sort_of(t, span)?),
            None => Term::bool(false),
        };
        self.fn_block(&f.body, &mut icx, env, &mut sub, &mut ret)?;
        let faults = Term::or(sub.exits.iter().map(|x| x.cond.clone()).collect());
        flow.exits.extend(sub.exits);
        // Der Zustand bleibt, was der Rumpf bis zu einem Fault schrieb.
        let locals = icx.locals.expect("Lokale");
        for (i, (loc, _)) in fields.iter().enumerate().skip(def.params.len()) {
            let v = locals[&VarId(i as u32)].clone();
            let old = env[loc].clone();
            env.insert(loc.clone(), Term::ite(call_alive.clone(), v, old));
        }
        flow.alive = Term::and(vec![call_alive, !faults]);
        if let Some(t) = target {
            let loc = match t {
                Place::Var(id) => self.loc_var(m, *id),
                Place::Output(c) => self.loc_out(*c),
                _ => return no("Ziel eines Methodenaufrufs", span),
            };
            let old = env[&loc].clone();
            env.insert(loc, Term::ite(flow.alive.clone(), ret, old));
        }
        Ok(())
    }

    /// Die Vertraege aller Bloecke als Ziele (5.7, B2).
    fn contract_goals(&mut self) -> R<Vec<ContractGoal>> {
        let mut out = Vec::new();
        for def in self.p.blocks.clone() {
            let (Some(fid), false) = (def.step, def.ensures.is_empty()) else { continue };
            let f = self.p.fns[fid.index()].clone();
            let mut vars: Vec<(String, Sort)> = Vec::new();
            let mut locals: BTreeMap<VarId, Term> = BTreeMap::new();
            let mut assume = Vec::new();
            let mut typed = |this: &mut Self, name: String, ty: TypeId, id: VarId| -> R<()> {
                let sort = this.sort_of(ty, def.span)?;
                let t = Term::var(name.clone(), sort);
                if let Some(inv) = this.type_invariant(t.clone(), ty) {
                    assume.push(inv);
                }
                vars.push((name, sort));
                locals.insert(id, t);
                Ok(())
            };
            let mut ok = true;
            for (i, p) in def.params.iter().enumerate() {
                ok &= typed(self, format!("c.{}.{}", def.name, p.name), p.ty, VarId(i as u32)).is_ok();
            }
            for (j, v) in def.state_vars.iter().enumerate() {
                ok &= typed(self, format!("c.{}.{}", def.name, v.name), v.ty, VarId((def.params.len() + j) as u32))
                    .is_ok();
            }
            let base = (def.params.len() + def.state_vars.len()) as u32;
            for (i, p) in f.params.iter().enumerate() {
                ok &= typed(self, format!("c.{}.step.{}", def.name, p.name), p.ty, VarId(base + i as u32)).is_ok();
            }
            if !ok {
                self.note(&format!("Vertrag von `{}` nicht kodiert: ein Typ ausserhalb der Reichweite", def.name));
                continue;
            }
            for (i, local) in f.locals.iter().enumerate().skip(f.params.len()) {
                let Ok(sort) = self.sort_of(local.ty, local.span) else { continue };
                locals.insert(VarId(base + i as u32), Enc::zero(sort));
            }
            let pre = Env::new();
            let actives = BTreeMap::new();
            let mut cx =
                Cx { m: None, leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: Some(locals) };
            let mut flow = Flow::new(Term::bool(true));
            let mut env = Env::new();
            for r in &def.requires {
                let t = self.expr(r, &cx, &env, &mut flow)?;
                assume.push(t);
            }
            let Some(ret_ty) = f.ret else { continue };
            let mut ret = Enc::zero(self.sort_of(ret_ty, f.span)?);
            self.fn_block(&f.body, &mut cx, &mut env, &mut flow, &mut ret)?;
            let faults = Term::or(flow.exits.iter().map(|x| x.cond.clone()).collect());
            // `result` ist die Lokale hinter den Schrittparametern.
            cx.locals.as_mut().expect("Lokale").insert(VarId(base + f.params.len() as u32), ret);
            let mut ensures = Vec::new();
            for e in &def.ensures {
                let mut eflow = Flow::new(Term::bool(true));
                ensures.push(self.expr(e, &cx, &env, &mut eflow)?);
            }
            let violation = Term::and(vec![Term::and(assume), !faults, !Term::and(ensures)]);
            out.push(ContractGoal { block: def.name.clone(), span: def.span, vars, violation });
        }
        Ok(out)
    }

    // ------------------------------------------------------------ Zustandswechsel

    /// `switch` (9.3): Austritte innen nach aussen, Eintritte aussen nach
    /// innen, Timer und zustandslokale Variablen frisch, Entry-Schleifen.
    /// Ein Fault in einem dieser Bloecke wird vom neuen Blatt aus
    /// aufgeloest, wie `resolve_m` es tut (Lemma 9.3.1 begrenzt die Tiefe).
    /// `under`: die Bedingung, unter der der Wechsel stattfindet — sie
    /// gehoert in jede Feuerbedingung, die seine Bloecke aufzeichnen.
    fn switch(
        &mut self,
        cx: &Cx<'_>,
        from: Option<StateId>,
        target: Target,
        env: &mut Env,
        depth: u32,
        under: &Term,
    ) -> R<()> {
        let m = cx.m.expect("Maschine");
        let machine = self.machine(m).clone();
        if depth > machine.states.len() as u32 + 2 {
            return no("Fault-Pfade tiefer als der Fault-Wald (Lemma 9.3.1)", machine.span);
        }
        let old = from.map(|s| self.chain_to(m, s)).unwrap_or_default();
        let new = match target {
            Target::Faulted => Vec::new(),
            Target::State(s) => self.descend(m, s),
            Target::Fault(_) => return no("Timeout-Fault einer Sequenz", machine.span),
        };
        let mut common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        if let Target::State(s) = target {
            common = common.min(self.chain_to(m, s).len() - 1);
        }
        // (1) Die neue Konfiguration steht, bevor ein Block laeuft.
        let new_leaf = new.last().copied();
        env.insert(self.loc_faulted(m), Term::bool(target == Target::Faulted));
        match new_leaf {
            Some(leaf) => {
                env.insert(self.loc_leaf(m), Term::int(self.code(m, leaf)));
            }
            None => {
                if let Some(code) = self.faulted_code(m) {
                    env.insert(self.loc_leaf(m), Term::int(code));
                }
            }
        }
        let entry = cx.entry();
        let mut flow = Flow::new(under.clone());
        for s in old[common..].iter().rev() {
            self.block(&machine.states[s.index()].exit, &entry, env, &mut flow)?;
        }
        if target == Target::Faulted {
            for (i, c) in self.p.channels.iter().enumerate() {
                if c.owner != Some(m) || c.dir != Direction::Output {
                    continue;
                }
                if let Some(safe) = c.attrs.safe.clone() {
                    let v = self.const_expr(&safe)?;
                    let loc = self.loc_out(ChannelId(i as u32));
                    let old = env[&loc].clone();
                    env.insert(loc, Term::ite(flow.alive.clone(), v, old));
                }
            }
        } else {
            let entered: Vec<StateId> = new[common..].to_vec();
            for s in &entered {
                let loc = self.loc_timer(m, *s);
                let old = env[&loc].clone();
                env.insert(loc, Term::ite(flow.alive.clone(), Term::int(0), old));
                for v in machine.states[s.index()].vars.clone() {
                    let def = machine.vars[v.index()].clone();
                    let value = match &def.init {
                        Some(e) => self.expr(e, &entry, env, &mut flow)?,
                        None => Enc::zero(self.sort_of(def.ty, def.span)?),
                    };
                    let loc = self.loc_var(m, v);
                    let old = env[&loc].clone();
                    env.insert(loc, Term::ite(flow.alive.clone(), value, old));
                }
            }
            for s in &entered {
                self.block(&machine.states[s.index()].enter, &entry, env, &mut flow)?;
            }
            if from.is_none() {
                self.block(&machine.loop_block, &entry, env, &mut flow)?;
            }
            for s in &entered {
                self.block(&machine.states[s.index()].loop_block, &entry, env, &mut flow)?;
            }
        }
        if flow.exits.is_empty() {
            return Ok(());
        }
        let base = env.clone();
        let mut merged = base.clone();
        for exit in &flow.exits {
            if exit.cond.is_bool(false) {
                continue;
            }
            let applied = self.resolve(cx, new_leaf, exit, &base, depth + 1)?;
            merged = ite_env(&exit.cond, &applied, &merged);
        }
        *env = merged;
        Ok(())
    }

    /// Wendet einen Ausgang an (`resolve_m`); `leaf` ist das Blatt, von dem
    /// aus gewechselt wird — keines in `FAULTED`.
    fn resolve(&mut self, cx: &Cx<'_>, leaf: Option<StateId>, exit: &Exit, env: &Env, depth: u32) -> R<Env> {
        let m = cx.m.expect("Maschine");
        let fault_target = match leaf {
            Some(l) => self.fault_target(m, l),
            None => match self.machine(m).fault_target {
                FaultTarget::State(s) => Target::State(s),
                FaultTarget::Faulted => Target::Faulted,
            },
        };
        let mut out = env.clone();
        match &exit.kind {
            ExitKind::Goto(t) => {
                out.insert(self.loc_latched(m), Term::bool(false));
                // Der Timeout einer Sequenz nimmt den Fault-Pfad des Zustands (6.2).
                let t = if matches!(t, Target::Fault(_)) { fault_target } else { *t };
                self.switch(cx, leaf, t, &mut out, depth, &exit.cond)?;
            }
            ExitKind::Fault(explicit) => {
                let t = explicit.unwrap_or(fault_target);
                self.switch(cx, leaf, t, &mut out, depth, &exit.cond)?;
            }
            ExitKind::Abort => {
                let latched = env[&self.loc_latched(m)].clone();
                let mut sw = env.clone();
                sw.insert(self.loc_latched(m), Term::bool(true));
                let under = Term::and(vec![exit.cond.clone(), !latched.clone()]);
                self.switch(cx, leaf, fault_target, &mut sw, depth, &under)?;
                out = ite_env(&latched, env, &sw);
            }
        }
        Ok(out)
    }

    /// Ein Tick einer Maschine (`step_m`), je Blatt ein Zweig.
    fn step_machine(
        &mut self,
        m: MachineId,
        active: &Term,
        pre: &Env,
        actives: &BTreeMap<MachineId, Term>,
        cur: &mut Env,
    ) -> R<()> {
        let machine = self.machine(m).clone();
        let base = cur.clone();
        let faulted = base.get(&self.loc_faulted(m)).cloned().expect("faulted");
        let leaf_now = base.get(&self.loc_leaf(m)).cloned().expect("leaf");
        let alive0 = Term::and(vec![active.clone(), faulted.not()]);
        let mut merged = base.clone();
        let period = i64::from(machine.period.max(1)).saturating_mul(self.p.config.tick);
        for leaf in self.leaves(m) {
            let is = Term::and(vec![alive0.clone(), Term::eq(leaf_now.clone(), Term::int(self.code(m, leaf)))]);
            let cx = Cx { m: Some(m), leaf: Some(leaf), mode: Mode::Run, pre, active: actives, locals: None };
            let mut env = base.clone();
            let mut flow = Flow::new(is.clone());
            self.block(&machine.loop_block, &cx, &mut env, &mut flow)?;
            let chain = self.chain_to(m, leaf);
            for s in &chain {
                self.block(&machine.states[s.index()].loop_block, &cx, &mut env, &mut flow)?;
            }
            for s in &chain {
                for t in &machine.states[s.index()].transitions {
                    let fired = match &t.trigger {
                        TransTrigger::When(takt_mir::machine::Guard::Expr(e)) => self.expr(e, &cx, &env, &mut flow)?,
                        TransTrigger::When(_) => return no("Guard mit Muster", t.span),
                        TransTrigger::After(d) => {
                            let ns = self.const_int(d)?;
                            let needed = ((ns + period - 1) / period).max(1);
                            let timer = env.get(&self.loc_timer(m, *s)).cloned().expect("Timer");
                            Term::bin(Op::Ge, timer, Term::int(needed))
                        }
                    };
                    let take = Term::and(vec![flow.alive.clone(), fired]);
                    let entry = cx.entry();
                    let mut sub = Flow::new(take.clone());
                    self.block(&t.actions, &entry, &mut env, &mut sub)?;
                    flow.exits.extend(sub.exits);
                    flow.exits.push(Exit { cond: sub.alive, kind: ExitKind::Goto(t.target) });
                    flow.alive = Term::and(vec![flow.alive.clone(), take.not()]);
                }
            }
            let mut out = env.clone();
            for exit in &flow.exits {
                if exit.cond.is_bool(false) {
                    continue;
                }
                let applied = self.resolve(&cx, Some(leaf), exit, &env, 0)?;
                out = ite_env(&exit.cond, &applied, &out);
            }
            merged = ite_env(&is, &out, &merged);
        }
        *cur = merged;
        Ok(())
    }

    /// Die Abort-Phase (5.4): jede nicht gelatchte Maschine nimmt ihren
    /// Fault-Pfad, wenn in diesem Tick ein `abort` lief.
    fn abort_phase(&mut self, raised: &Term, pre: &Env, actives: &BTreeMap<MachineId, Term>, cur: &mut Env) -> R<()> {
        for &m in &self.order.clone() {
            let faulted = cur.get(&self.loc_faulted(m)).cloned().expect("faulted");
            let latched = cur.get(&self.loc_latched(m)).cloned().expect("latched");
            let deliver = Term::and(vec![raised.clone(), faulted.not(), latched.not()]);
            if deliver.is_bool(false) {
                continue;
            }
            let leaf_now = cur.get(&self.loc_leaf(m)).cloned().expect("leaf");
            let base = cur.clone();
            let mut merged = base.clone();
            for leaf in self.leaves(m) {
                let is = Term::and(vec![deliver.clone(), Term::eq(leaf_now.clone(), Term::int(self.code(m, leaf)))]);
                let cx = Cx { m: Some(m), leaf: Some(leaf), mode: Mode::Entry, pre, active: actives, locals: None };
                let mut env = base.clone();
                env.insert(self.loc_latched(m), Term::bool(true));
                let t = self.fault_target(m, leaf);
                self.switch(&cx, Some(leaf), t, &mut env, 0, &is)?;
                merged = ite_env(&is, &env, &merged);
            }
            *cur = merged;
        }
        Ok(())
    }

    /// `advance_counters` (7.2): Timer entlang der neuen Kette, Zaehler.
    fn advance(&mut self, actives: &BTreeMap<MachineId, Term>, cur: &mut Env) {
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let active = actives[&m].clone();
            if machine.period > 1 {
                let loc = self.loc_countdown(m);
                let c = cur[&loc].clone();
                let next = Term::ite(
                    active.clone(),
                    Term::int(i64::from(machine.period) - 1),
                    Term::bin(Op::Sub, c.clone(), Term::int(1)),
                );
                cur.insert(loc, next);
            }
            let faulted = cur[&self.loc_faulted(m)].clone();
            let leaf_now = cur[&self.loc_leaf(m)].clone();
            for (i, _) in machine.states.iter().enumerate() {
                let s = StateId(i as u32);
                let in_chain: Vec<Term> = self
                    .leaves(m)
                    .into_iter()
                    .filter(|l| self.chain_to(m, *l).contains(&s))
                    .map(|l| Term::eq(leaf_now.clone(), Term::int(self.code(m, l))))
                    .collect();
                let cond = Term::and(vec![active.clone(), faulted.clone().not(), Term::or(in_chain)]);
                let loc = self.loc_timer(m, s);
                let t = cur[&loc].clone();
                cur.insert(loc, Term::ite(cond, Term::bin(Op::Add, t.clone(), Term::int(1)), t));
            }
        }
    }

    fn actives(&self, pre: &Env) -> BTreeMap<MachineId, Term> {
        self.order
            .iter()
            .map(|&m| {
                let active = if self.machine(m).period > 1 {
                    Term::eq(pre[&self.loc_countdown(m)].clone(), Term::int(0))
                } else {
                    Term::bool(true)
                };
                (m, active)
            })
            .collect()
    }

    /// Ein Tick des Systems ueber `pre`.
    fn tick(&mut self, pre: &Env) -> R<Env> {
        let mut cur = pre.clone();
        for &m in &self.order.clone() {
            for i in 0..self.machine(m).signals.len() {
                cur.insert(self.loc_sig(m, i), Term::bool(false));
            }
        }
        let actives = self.actives(pre);
        self.aborts.clear();
        for &m in &self.order.clone() {
            let active = actives[&m].clone();
            self.step_machine(m, &active, pre, &actives, &mut cur)?;
        }
        let raised = Term::or(std::mem::take(&mut self.aborts));
        if !raised.is_bool(false) {
            self.abort_phase(&raised, pre, &actives, &mut cur)?;
        }
        self.advance(&actives, &mut cur);
        Ok(cur)
    }

    /// Der Anfangszustand: Defaults, `safe`-Outputs, `init_vars`, dann der
    /// erste Eintritt jeder Maschine in Schrittordnung (`Sim::init`).
    fn init(&mut self) -> R<Env> {
        let mut env = Env::new();
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            env.insert(self.loc_leaf(m), Term::int(self.code(m, machine.initial)));
            env.insert(self.loc_faulted(m), Term::bool(false));
            env.insert(self.loc_latched(m), Term::bool(false));
            if machine.period > 1 {
                env.insert(self.loc_countdown(m), Term::int(0));
            }
            for i in 0..machine.states.len() {
                env.insert(self.loc_timer(m, StateId(i as u32)), Term::int(0));
            }
            for (i, v) in machine.vars.iter().enumerate() {
                if matches!(self.p.types.get(v.ty), Type::Handle(HandleKind::Block(_))) {
                    let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) else {
                        return no("Instanz-Array eines Blocks", v.span);
                    };
                    for (loc, ty) in fields {
                        env.insert(loc, Enc::zero(self.sort_of(ty, v.span)?));
                    }
                    continue;
                }
                env.insert(self.loc_var(m, VarId(i as u32)), Enc::zero(self.sort_of(v.ty, v.span)?));
            }
            for i in 0..machine.signals.len() {
                env.insert(self.loc_sig(m, i), Term::bool(false));
            }
        }
        for (i, c) in self.p.channels.iter().enumerate() {
            if c.dir != Direction::Output {
                continue;
            }
            let value = match c.attrs.safe.clone() {
                Some(e) => self.const_expr(&e)?,
                None => Enc::zero(self.sort_of(c.ty, c.span)?),
            };
            env.insert(self.loc_out(ChannelId(i as u32)), value);
        }
        // `init_vars` liest Ψ mit den Anfangswerten; die Eintritte lesen
        // frisch (7.2), also gilt jede Maschine als aktiv.
        let actives: BTreeMap<MachineId, Term> = self.order.iter().map(|&m| (m, Term::bool(true))).collect();
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let pre = env.clone();
            let cx = Cx { m: Some(m), leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: None };
            for (i, v) in machine.vars.iter().enumerate() {
                if let Some(Expr { kind: ExprKind::BlockInit { block, args, .. }, .. }) = &v.init {
                    // Eine Instanz (5.7): Parameter aus den Argumenten, Zustand aus
                    // seinen Initialwerten — wie `instantiate_block`.
                    let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) else { continue };
                    let def = self.p.blocks[block.index()].clone();
                    let mut flow = Flow::new(Term::bool(true));
                    for (k, a) in args.iter().enumerate() {
                        let t = self.expr(a, &cx, &env, &mut flow)?;
                        env.insert(fields[k].0.clone(), t);
                    }
                    self.block_reset(&def, &fields, &cx, &mut env, &mut flow)?;
                    continue;
                }
                if let Some(e) = &v.init {
                    let mut flow = Flow::new(Term::bool(true));
                    let t = self.expr(e, &cx, &env, &mut flow)?;
                    env.insert(self.loc_var(m, VarId(i as u32)), t);
                }
            }
        }
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let pre = env.clone();
            let cx = Cx { m: Some(m), leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: None };
            self.switch(&cx, None, Target::State(machine.initial), &mut env, 0, &Term::bool(true))?;
        }
        self.advance(&actives, &mut env);
        Ok(env)
    }

    // ------------------------------------------------------------ Eigenschaften

    /// Die Invariante eines typisierten Orts (3.4): Range, Endlichkeit, Enum.
    fn type_invariant(&self, x: Term, ty: TypeId) -> Option<Term> {
        Some(match self.p.types.get(ty) {
            Type::Int { range: Some(r), .. } | Type::Duration { range: Some(r) } => {
                let (lo, hi) = (self.bound(&r.lo, Sort::Int), self.bound(&r.hi, Sort::Int));
                Term::and(vec![Term::bin(Op::Ge, x.clone(), lo), Term::bin(Op::Le, x, hi)])
            }
            Type::Float { range: Some(r), .. } => {
                let s = x.sort();
                let (lo, hi) = (self.bound(&r.lo, s), self.bound(&r.hi, s));
                Term::and(vec![Term::bin(Op::FGe, x.clone(), lo), Term::bin(Op::FLe, x, hi)])
            }
            Type::Float { .. } => Term::app(Op::IsFinite, vec![x]),
            Type::Enum(e) => {
                let n = self.p.enums[e.index()].variants.len() as i64;
                Term::and(vec![Term::bin(Op::Ge, x.clone(), Term::int(0)), Term::bin(Op::Lt, x, Term::int(n))])
            }
            _ => return None,
        })
    }

    /// Invarianten des Zustands aus den Typen (3.4): Blaetter, Zaehler,
    /// Variablen und Outputs in ihren Ranges.
    fn state_invariants(&mut self, pre: &Env) -> Vec<Term> {
        let mut out = Vec::new();
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let leaf = pre[&self.loc_leaf(m)].clone();
            let mut codes: Vec<Term> =
                self.leaves(m).into_iter().map(|l| Term::eq(leaf.clone(), Term::int(self.code(m, l)))).collect();
            if let Some(c) = self.faulted_code(m) {
                codes.push(Term::eq(leaf.clone(), Term::int(c)));
            }
            out.push(Term::or(codes));
            for i in 0..machine.states.len() {
                let t = pre[&self.loc_timer(m, StateId(i as u32))].clone();
                out.push(Term::bin(Op::Ge, t, Term::int(0)));
            }
            if machine.period > 1 {
                let c = pre[&self.loc_countdown(m)].clone();
                out.push(Term::and(vec![
                    Term::bin(Op::Ge, c.clone(), Term::int(0)),
                    Term::bin(Op::Lt, c, Term::int(i64::from(machine.period))),
                ]));
            }
            for (i, v) in machine.vars.iter().enumerate() {
                if let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) {
                    for (loc, ty) in fields {
                        if let Some(t) = pre.get(&loc).cloned().and_then(|x| self.type_invariant(x, ty)) {
                            out.push(t);
                        }
                    }
                    continue;
                }
                let loc = self.loc_var(m, VarId(i as u32));
                if let Some(t) = pre.get(&loc).cloned().and_then(|x| self.type_invariant(x, v.ty)) {
                    out.push(t);
                }
            }
        }
        for (i, c) in self.p.channels.iter().enumerate() {
            if c.dir != Direction::Output {
                continue;
            }
            let loc = self.loc_out(ChannelId(i as u32));
            if let Some(t) = pre.get(&loc).cloned().and_then(|x| self.type_invariant(x, c.ty)) {
                out.push(t);
            }
        }
        out
    }

    /// Kanal-Ranges als Annahmen ueber die Eingaben (13.3).
    fn channel_assumptions(&mut self) -> R<Vec<Term>> {
        let mut out = Vec::new();
        for c in &self.p.channels {
            if c.dir != Direction::Input {
                continue;
            }
            let range = match self.p.types.get(c.ty) {
                Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range } => *range,
                _ => None,
            };
            let sort = self.sort_of(c.ty, c.span)?;
            let x = self.input(format!("i.{}", c.name), sort);
            // 4.1: NaN und Unendlich gibt es in der Sprache nicht; der Rand
            // liefert sie nicht.
            if matches!(sort, Sort::F32 | Sort::F64) {
                out.push(Term::app(Op::IsFinite, vec![x.clone()]));
            }
            let Some(r) = range else { continue };
            let (lo, hi) = (self.bound(&r.lo, sort), self.bound(&r.hi, sort));
            let (ge, le) = if sort == Sort::Int { (Op::Ge, Op::Le) } else { (Op::FGe, Op::FLe) };
            out.push(Term::and(vec![Term::bin(ge, x.clone(), lo), Term::bin(le, x, hi)]));
        }
        Ok(out)
    }

    /// `always(φ)`/`never(φ)` ohne Zeitoperatoren als Invariante ueber den
    /// Zustand nach dem Commit und die Eingaben des Ticks.
    fn goal(&mut self, prop: &Property, state: &Env) -> R<Option<Goal>> {
        let (inner, negate) = match &prop.formula {
            TProp::Temporal { op: TemporalOp::Always, inner, .. } => (inner.as_ref(), false),
            TProp::Temporal { op: TemporalOp::Never, inner, .. } => (inner.as_ref(), true),
            _ => return Ok(None),
        };
        let actives = BTreeMap::new();
        let cx = Cx { m: None, leaf: None, mode: Mode::Entry, pre: state, active: &actives, locals: None };
        let Some(t) = self.tprop(inner, &cx, state)? else { return Ok(None) };
        Ok(Some(Goal {
            name: prop.name.clone(),
            assumption: prop.assumption,
            formula: if negate { t.not() } else { t },
        }))
    }

    fn tprop(&mut self, f: &TProp, cx: &Cx<'_>, env: &Env) -> R<Option<Term>> {
        Ok(Some(match f {
            TProp::Atom(e) => {
                let mut flow = Flow::new(Term::bool(true));
                let t = self.expr(e, cx, env, &mut flow)?;
                // Ein Atom, das faultet, ist falsch (13.5).
                let faults = Term::or(flow.exits.iter().map(|x| x.cond.clone()).collect());
                Term::ite(faults, Term::bool(false), t)
            }
            TProp::Not(a) => match self.tprop(a, cx, env)? {
                Some(t) => t.not(),
                None => return Ok(None),
            },
            TProp::And(a, b) | TProp::Or(a, b) | TProp::Implies(a, b) => {
                let (Some(x), Some(y)) = (self.tprop(a, cx, env)?, self.tprop(b, cx, env)?) else { return Ok(None) };
                match f {
                    TProp::And(..) => Term::and(vec![x, y]),
                    TProp::Or(..) => Term::or(vec![x, y]),
                    _ => Term::or(vec![x.not(), y]),
                }
            }
            TProp::Temporal { .. } => return Ok(None),
        }))
    }
}

fn has_var(t: &Term) -> bool {
    match &*t.0 {
        Node::Var(..) => true,
        Node::App(_, args) => args.iter().any(has_var),
        _ => false,
    }
}

fn node_name(e: &ExprKind) -> &'static str {
    match e {
        ExprKind::Str(_) | ExprKind::Format(_) => "Text",
        ExprKind::Record { .. } | ExprKind::Field { .. } => "Record",
        ExprKind::Array(_) | ExprKind::Index { .. } | ExprKind::Slice { .. } => "Sammlung",
        ExprKind::Accessor { .. } => "Zugriff",
        ExprKind::Matches { .. } => "`matches`",
        ExprKind::NativeCall { .. } => "native Funktion",
        ExprKind::MatOp { .. } | ExprKind::Index2 { .. } => "Matrix",
        ExprKind::Decode { .. } => "`decode`",
        ExprKind::JobState { .. } => "Job",
        ExprKind::Stream(_) => "Strom",
        ExprKind::Lift(_) | ExprKind::Ok(_) | ExprKind::Err(_) | ExprKind::None => "Wrapper",
        _ => "dieser Art",
    }
}

fn stmt_name(s: &StmtKind) -> &'static str {
    match s {
        StmtKind::ForEach { .. } => "`for … in`",
        StmtKind::Send { .. } => "`send`",
        StmtKind::At { .. } => "`at`",
        StmtKind::Every { .. } => "`every`",
        StmtKind::Job { .. } => "`job`",
        StmtKind::MethodCall { .. } => "Methodenaufruf",
        StmtKind::Break => "`break`",
        StmtKind::Cancel(_) | StmtKind::Skip(_) | StmtKind::Arm { .. } => "Strom oder Trigger",
        StmtKind::Return(_) => "`return`",
        _ => "dieser Art",
    }
}
