//! Verifier beim Laden (plan/m1.md 4.7): Kernform, Indizes in ihren Tabellen,
//! Knoten spaeterer Stufen. Danach indiziert die Ausfuehrung ohne Pruefung.

use takt_diag::{Diagnostic, Span, Stage};
use takt_mir::expr::{Expr, ExprKind, MachineRef, StreamRef};
use takt_mir::machine::{Guard, Machine, TransTrigger};
use takt_mir::program::Program;
use takt_mir::stmt::{Block, Place, Stmt, StmtKind};
use takt_mir::types::Type;

/// Code der Diagnosen des Verifiers.
pub const CODE: &str = "MIR";

struct Checker<'p> {
    p: &'p Program,
    machine: Option<&'p Machine>,
    locals: Option<usize>,
}

fn err(span: Span, msg: impl Into<String>) -> Diagnostic {
    Diagnostic::error(CODE, span, msg)
}

fn stage(span: Span, what: &str, stage: Stage) -> Diagnostic {
    Diagnostic::error(CODE, span, format!("{what} wird erst ab {} ausgefuehrt", stage.as_str())).with_stage(stage)
}

impl Checker<'_> {
    fn ty(&self, id: takt_mir::TypeId, span: Span) -> Result<&Type, Diagnostic> {
        self.p.types.list.get(id.index()).ok_or_else(|| err(span, format!("Typ {} fehlt", id.0)))
    }

    fn index<T>(&self, table: &[T], i: usize, what: &str, span: Span) -> Result<(), Diagnostic> {
        if i < table.len() { Ok(()) } else { Err(err(span, format!("{what} {i} ausserhalb der Tabelle"))) }
    }

    fn var(&self, v: takt_mir::VarId, span: Span) -> Result<(), Diagnostic> {
        match (self.locals, self.machine) {
            (Some(n), _) => self.index(&vec![(); n], v.index(), "Variable", span),
            (None, Some(m)) => self.index(&m.vars, v.index(), "Variable", span),
            (None, None) => Err(err(span, "Variable ausserhalb von Funktion und Maschine")),
        }
    }

    fn machine_ref(&self, m: &MachineRef, span: Span) -> Result<(), Diagnostic> {
        self.index(&self.p.machines, m.machine.index(), "Maschine", span)?;
        if let Some(i) = &m.index {
            self.expr(i)?;
        }
        Ok(())
    }

    fn stream(&self, s: StreamRef, span: Span) -> Result<(), Diagnostic> {
        match s {
            StreamRef::Channel(c) => self.index(&self.p.channels, c.index(), "Channel", span),
            StreamRef::Internal(i) => self.index(&self.p.streams, i.index(), "Stream", span),
            StreamRef::Fired(t) => self.index(&self.p.triggers, t.index(), "Trigger", span),
            StreamRef::Var(v) => self.var(v, span),
        }
    }

    fn place(&self, pl: &Place, span: Span) -> Result<(), Diagnostic> {
        match pl {
            Place::Var(v) => self.var(*v, span),
            Place::Output(c) => self.index(&self.p.channels, c.index(), "Channel", span),
            Place::Field(b, _) => self.place(b, span),
            Place::Index(b, i) => {
                self.place(b, span)?;
                self.expr(i)
            }
            Place::Index2(b, i, j) => {
                self.place(b, span)?;
                self.expr(i)?;
                self.expr(j)
            }
        }
    }

    fn exprs(&self, es: &[Expr]) -> Result<(), Diagnostic> {
        es.iter().try_for_each(|e| self.expr(e))
    }

    fn expr(&self, e: &Expr) -> Result<(), Diagnostic> {
        let span = e.span;
        self.ty(e.ty, span)?;
        match &e.kind {
            ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Duration(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Default
            | ExprKind::Builtin(_) => Ok(()),
            ExprKind::Variant { enum_id, variant, fields } => {
                self.index(&self.p.enums, enum_id.index(), "Enum", span)?;
                self.index(&self.p.enums[enum_id.index()].variants, *variant as usize, "Variante", span)?;
                self.exprs(fields)
            }
            ExprKind::Record { record, fields } => {
                self.index(&self.p.records, record.index(), "Record", span)?;
                self.exprs(fields)
            }
            ExprKind::Array(items) => self.exprs(items),
            ExprKind::Tuple(a, b) => {
                self.expr(a)?;
                self.expr(b)
            }
            ExprKind::BlockInit { block, args, .. } => {
                self.index(&self.p.blocks, block.index(), "Block", span)?;
                self.exprs(args)
            }
            ExprKind::Var(v) => self.var(*v, span),
            ExprKind::Param(p) => self.index(&self.p.params, p.index(), "Parameter", span),
            ExprKind::Command(c) => self.index(&self.p.commands, c.index(), "Command", span),
            ExprKind::Input { channel, .. } | ExprKind::Output(channel) => {
                self.index(&self.p.channels, channel.index(), "Channel", span)
            }
            ExprKind::Published { machine, var } => {
                self.machine_ref(machine, span)?;
                self.index(&self.p.machines[machine.machine.index()].vars, var.index(), "Variable", span)
            }
            ExprKind::StateOf(m) => self.machine_ref(m, span),
            ExprKind::Signal { machine, signal } => {
                self.machine_ref(machine, span)?;
                self.index(&self.p.machines[machine.machine.index()].signals, signal.index(), "Signal", span)
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Index { base, index } => {
                self.expr(base)?;
                self.expr(index)
            }
            ExprKind::Index2 { base, row, col } => {
                self.expr(base)?;
                self.expr(row)?;
                self.expr(col)
            }
            ExprKind::Slice { base, from, to } => {
                self.expr(base)?;
                self.expr(from)?;
                self.expr(to)
            }
            ExprKind::Accessor { base, args, .. } => {
                self.expr(base)?;
                self.exprs(args)
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Cast { expr, .. }
            | ExprKind::Lift(expr)
            | ExprKind::Ok(expr)
            | ExprKind::Err(expr) => self.expr(expr),
            ExprKind::Checked { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs)?;
                self.expr(rhs)
            }
            ExprKind::Cond { cond, then, otherwise } => {
                self.expr(cond)?;
                self.expr(then)?;
                self.expr(otherwise)
            }
            ExprKind::Convert { expr, unit, .. } => {
                self.index(&self.p.units, unit.index(), "Einheit", span)?;
                self.expr(expr)
            }
            ExprKind::Matches { .. } => Err(stage(span, "Musterabgleich", Stage::V1_1)),
            ExprKind::Call { callee, args } => {
                self.index(&self.p.fns, callee.index(), "Funktion", span)?;
                self.exprs(args)
            }
            ExprKind::NativeCall { native, args } => {
                self.index(&self.p.natives, native.index(), "native Funktion", span)?;
                self.exprs(args)
            }
            ExprKind::MatOp { .. } => Err(stage(span, "Matrixoperation", Stage::V1_1)),
            ExprKind::Decode { .. } => Err(stage(span, "decode", Stage::V1_1)),
            ExprKind::Intrinsic { args, .. } => self.exprs(args),
        }
    }

    fn block(&self, b: &Block) -> Result<(), Diagnostic> {
        b.stmts.iter().try_for_each(|s| self.stmt(s))
    }

    fn stmt(&self, s: &Stmt) -> Result<(), Diagnostic> {
        let span = s.span;
        match &s.kind {
            StmtKind::Assign { target, value } => {
                self.place(target, span)?;
                self.expr(value)
            }
            StmtKind::Check { cond, message, confirm, .. } => {
                self.expr(cond)?;
                if let Some(m) = message {
                    self.format(m)?;
                }
                if let Some(c) = confirm {
                    self.expr(&c.duration)?;
                    let m = self.machine.ok_or_else(|| err(span, "check ausserhalb einer Maschine"))?;
                    self.index(&m.layout.viol_sites, c.site.index(), "Bestaetigungsstelle", span)?;
                }
                Ok(())
            }
            StmtKind::Goto(_) | StmtKind::Break | StmtKind::Pass => Ok(()),
            StmtKind::Abort { message } => message.as_ref().map_or(Ok(()), |m| self.format(m)),
            StmtKind::If { cond, then, otherwise } => {
                self.expr(cond)?;
                self.block(then)?;
                self.block(otherwise)
            }
            StmtKind::ForRange { var, count, body } => {
                self.var(*var, span)?;
                self.expr(count)?;
                self.block(body)
            }
            StmtKind::ForEach { iter, body, .. } => {
                self.expr(iter)?;
                self.block(body)
            }
            StmtKind::Match { subject, arms } => {
                self.expr(subject)?;
                arms.iter().try_for_each(|a| self.block(&a.body))
            }
            StmtKind::Return(e) => self.expr(e),
            StmtKind::Send { .. } => Err(stage(span, "send", Stage::V1_1)),
            StmtKind::At { .. } => Err(stage(span, "at", Stage::V1_1)),
            StmtKind::Cancel(_) => Err(stage(span, "cancel", Stage::V1_1)),
            StmtKind::Raise(sig) => {
                let m = self.machine.ok_or_else(|| err(span, "raise ausserhalb einer Maschine"))?;
                self.index(&m.signals, sig.index(), "Signal", span)
            }
            StmtKind::Job { .. } => Err(stage(span, "job", Stage::V1_1)),
            StmtKind::Every { period, counter, body } => {
                self.expr(period)?;
                let m = self.machine.ok_or_else(|| err(span, "every ausserhalb einer Maschine"))?;
                self.index(&m.layout.every_counters, counter.index(), "every-Zaehler", span)?;
                self.block(body)
            }
            StmtKind::Observe(o) => {
                use takt_mir::stmt::Observe;
                match o {
                    Observe::Alert { cond, message, confirm } => {
                        self.expr(cond)?;
                        self.format(message)?;
                        if let Some(c) = confirm {
                            self.expr(&c.duration)?;
                        }
                        Ok(())
                    }
                    Observe::Log(f) => self.format(f),
                    Observe::Measure { value, .. } => self.expr(value),
                    Observe::Verify { cond, message, .. } => {
                        self.expr(cond)?;
                        self.format(message)
                    }
                    Observe::Verdict { message, .. } => message.as_ref().map_or(Ok(()), |m| self.format(m)),
                }
            }
            StmtKind::Arm { .. } => Err(stage(span, "arm", Stage::V1_2)),
            StmtKind::MethodCall { target, receiver, args, .. } => {
                if let Some(t) = target {
                    self.place(t, span)?;
                }
                self.place(receiver, span)?;
                self.exprs(args)
            }
        }
    }

    fn format(&self, f: &takt_mir::pattern::Format) -> Result<(), Diagnostic> {
        for piece in &f.pieces {
            if let takt_mir::pattern::FormatPiece::Expr { expr, .. } = piece {
                self.expr(expr)?;
            }
        }
        Ok(())
    }

    fn guard(&self, g: &Guard, span: Span) -> Result<(), Diagnostic> {
        match g {
            Guard::Expr(e) => self.expr(e),
            Guard::Match { .. } => Err(stage(span, "Musterguard", Stage::V1_1)),
            Guard::Next { stream, .. } => {
                self.stream(*stream, span)?;
                Err(stage(span, "Stream-Guard", Stage::V1_1))
            }
        }
    }
}

/// Prueft ein Programm; die erste Verletzung ist das Ergebnis.
pub fn check(p: &Program) -> Result<(), Diagnostic> {
    if !p.is_core() {
        return Err(err(Span::default(), "Sequenz-Oberflaeche nicht entzuckert (desugar fehlt)"));
    }
    // Rahmengroesse je Funktion: Lokale (die Parameter zaehlen mit); bei einer
    // Blockmethode stehen davor die Instanzvariablen (plan/mir.md Abschnitt 7).
    let mut frame: Vec<usize> = p.fns.iter().map(|f| f.locals.len()).collect();
    for b in &p.blocks {
        let instance = b.params.len() + b.state_vars.len();
        for f in b.step.iter().chain(&b.methods) {
            if let Some(n) = frame.get_mut(f.index()) {
                *n += instance;
            }
        }
    }
    for (i, f) in p.fns.iter().enumerate() {
        let c = Checker { p, machine: None, locals: Some(frame[i]) };
        c.block(&f.body)?;
        for v in &f.locals {
            if let Some(init) = &v.init {
                c.expr(init)?;
            }
        }
    }
    for b in &p.blocks {
        let n = b.params.len() + b.state_vars.len();
        let c = Checker { p, machine: None, locals: Some(n) };
        for v in &b.state_vars {
            if let Some(init) = &v.init {
                c.expr(init)?;
            }
        }
        for f in b.step.iter().chain(&b.methods) {
            c.index(&p.fns, f.index(), "Funktion", b.span)?;
        }
    }
    for m in &p.machines {
        let c = Checker { p, machine: Some(m), locals: None };
        if m.states.is_empty() {
            if matches!(m.kind, takt_mir::machine::MachineKind::Template) {
                continue;
            }
            return Err(err(m.span, format!("Maschine `{}` ohne Zustand", m.name)));
        }
        c.index(&m.states, m.initial.index(), "Zustand", m.span)?;
        if m.states[m.initial.index()].parent.is_some() {
            return Err(err(m.span, format!("initial `{}` ist kein Wurzelzustand", m.states[m.initial.index()].name)));
        }
        for v in &m.vars {
            if let Some(init) = &v.init {
                c.expr(init)?;
            }
        }
        if !m.persist.is_empty() {
            return Err(stage(m.span, "persist", Stage::V1_1));
        }
        if !m.follows.is_empty() {
            return Err(stage(m.span, "follows", Stage::V1_1));
        }
        if m.node.is_some() {
            return Err(stage(m.span, "Knotenplatzierung", Stage::V2));
        }
        c.block(&m.loop_block)?;
        if !m.handlers.is_empty() {
            return Err(stage(m.span, "Handler", Stage::V1_1));
        }
        for t in &m.faulted.transitions {
            self_transition(&c, t)?;
        }
        for (i, s) in m.states.iter().enumerate() {
            if let Some(parent) = s.parent {
                c.index(&m.states, parent.index(), "Zustand", s.span)?;
            }
            if let Some(init) = s.initial {
                c.index(&m.states, init.index(), "Zustand", s.span)?;
                if m.states[init.index()].parent != Some(takt_mir::StateId(i as u32)) {
                    return Err(err(
                        s.span,
                        format!("initial `{}` ist kein Kind von `{}`", m.states[init.index()].name, s.name),
                    ));
                }
            } else if !s.children.is_empty() {
                return Err(err(s.span, format!("Zustand `{}` hat Kinder, aber kein initial", s.name)));
            }
            if s.idle {
                return Err(stage(s.span, "idle", Stage::V1_1));
            }
            if s.resume {
                return Err(stage(s.span, "resume", Stage::V1_2));
            }
            if !s.instances.is_empty() {
                return Err(stage(s.span, "gescopte Instanzen", Stage::V1_2));
            }
            if !s.handlers.is_empty() {
                return Err(stage(s.span, "Handler", Stage::V1_1));
            }
            for &v in &s.vars {
                c.index(&m.vars, v.index(), "Variable", s.span)?;
            }
            c.block(&s.enter)?;
            c.block(&s.exit)?;
            c.block(&s.loop_block)?;
            for t in &s.transitions {
                self_transition(&c, t)?;
            }
        }
    }
    if !p.triggers.is_empty() {
        return Err(stage(p.triggers[0].span, "Trigger", Stage::V1_2));
    }
    if !p.properties.is_empty() {
        return Err(stage(p.properties[0].span, "Eigenschaften", Stage::V1_1));
    }
    Ok(())
}

fn self_transition(c: &Checker<'_>, t: &takt_mir::machine::Transition) -> Result<(), Diagnostic> {
    match &t.trigger {
        TransTrigger::When(g) => c.guard(g, t.span)?,
        TransTrigger::After(d) => c.expr(d)?,
    }
    c.block(&t.actions)?;
    if let takt_mir::machine::Target::State(s) = t.target {
        let m = c.machine.expect("Maschine");
        c.index(&m.states, s.index(), "Zustand", t.span)?;
    }
    Ok(())
}
