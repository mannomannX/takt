//! Desugaring der Sequenzen nach Referenz 6.2.
//!
//! Eine Sequenz wird von links nach rechts in Segmente geteilt; jedes Segment
//! ist ein Kindzustand `S_i` des umgebenden Zustands. Segmentgrenzen sind
//! `wait`, `until`, jede Anweisung mit `->` (auch in `if`-Zweigen) und das
//! Ende eines `repeat`-Koerpers. Die Regeln der Tabelle in 6.2 sind je Item
//! als Funktion dieses Moduls umgesetzt; Beispiel 6.3 ist der Test.
//!
//! Voraussetzung aus dem Lowering: `var x = e` in der Sequenz ist bereits
//! eine gehobene Variable des Zustands (`VarScope::Lifted`) mit `Assign`;
//! Captures und `repeat`-Zaehler sind ebenso gehoben.

use takt_diag::{Diagnostic, Span};

use crate::expr::{BinaryOp, Expr, ExprKind, UnaryOp};
use crate::ids::{StateId, TypeId, VarId};
use crate::machine::*;
use crate::program::Program;
use crate::stmt::{Block, CheckKind, Place, Stmt, StmtKind};
use crate::types::{IntWidth, Type, TypeTable};

/// Code der Diagnosen dieses Moduls.
pub const CODE: &str = "MIR";

/// Ersetzt in allen Maschinen die Sequenz-Oberflaeche durch Zustaende.
pub fn desugar(program: &mut Program) -> Result<(), Diagnostic> {
    let Program { types, machines, .. } = program;
    for m in machines.iter_mut() {
        desugar_machine(types, m)?;
    }
    Ok(())
}

/// Desugaring einer Maschine; danach gilt `Machine::is_core`.
pub fn desugar_machine(types: &mut TypeTable, m: &mut Machine) -> Result<(), Diagnostic> {
    let mut i = 0;
    while i < m.states.len() {
        if let Some(seq) = m.states[i].sequence.take() {
            let state = &m.states[i];
            if !state.children.is_empty() || state.initial.is_some() {
                return Err(Diagnostic::error(
                    CODE,
                    seq.span,
                    format!("Zustand `{}` hat eine Sequenz und Kindzustaende", state.name),
                )
                .with_suggestion("die Sequenz in einen eigenen Kindzustand legen (6.2)"));
            }
            Builder::new(types, m, StateId(i as u32), seq.done).run(seq)?;
        }
        i += 1;
    }
    Ok(())
}

/// Ziel eines Uebergangs waehrend des Aufbaus: das naechste Segment oder fest.
#[derive(Clone, Copy)]
enum SegTarget {
    Next,
    Fixed(Target),
}

struct Proto {
    trigger: TransTrigger,
    actions: Vec<Stmt>,
    target: SegTarget,
    span: Span,
}

struct Seg {
    id: StateId,
    enter: Vec<Stmt>,
    loop_stmts: Vec<Stmt>,
    transitions: Vec<Proto>,
}

struct Builder<'a> {
    m: &'a mut Machine,
    parent: StateId,
    done: VarId,
    segs: Vec<Seg>,
    /// `check` ab Position p: in `loop:` aller folgenden Segmente.
    active_checks: Vec<Stmt>,
    pending_name: Option<String>,
    /// Das aktuelle Segment ist abgeschlossen; das naechste Item eroeffnet ein neues.
    closed: bool,
    /// Die Sequenz hat den Zustand mit `->` verlassen; kein Haltesegment noetig.
    left: bool,
    expect_count: u32,
    bool_ty: TypeId,
    int_ty: TypeId,
}

impl<'a> Builder<'a> {
    fn new(types: &'a mut TypeTable, m: &'a mut Machine, parent: StateId, done: VarId) -> Self {
        let bool_ty = types.intern(Type::Bool);
        let int_ty = types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
        Builder {
            m,
            parent,
            done,
            segs: Vec::new(),
            active_checks: Vec::new(),
            pending_name: None,
            closed: true,
            left: false,
            expect_count: 0,
            bool_ty,
            int_ty,
        }
    }

    fn run(mut self, seq: Sequence) -> Result<(), Diagnostic> {
        self.open();
        self.items(&seq.items)?;
        if self.closed && !self.left {
            self.open();
        }
        self.finish(seq.span);
        Ok(())
    }

    // ------------------------------------------------------------ Segmente

    /// Eroeffnet Segment `S_i` als Kindzustand; `loop:` beginnt mit den
    /// bis hierher aktiven `check`s.
    fn open(&mut self) {
        let i = self.segs.len();
        let parent_name = self.m.states[self.parent.index()].name.clone();
        let mut state = State::new(format!("{parent_name}.S{i}"), Some(self.parent));
        state.step_name = self.pending_name.take();
        let id = self.m.add_state(state);
        self.segs.push(Seg { id, enter: Vec::new(), loop_stmts: self.active_checks.clone(), transitions: Vec::new() });
        self.closed = false;
        self.left = false;
    }

    fn cur(&mut self) -> &mut Seg {
        if self.closed {
            self.open();
        }
        self.segs.last_mut().expect("Segment offen")
    }

    /// Das aktuelle Segment hat noch keinen Inhalt (frisch nach einer Zeitgrenze).
    fn cur_is_empty(&self) -> bool {
        self.segs.last().is_some_and(|s| s.enter.is_empty() && s.transitions.is_empty())
            && self.segs.last().is_some_and(|s| s.loop_stmts.len() == self.active_checks.len())
    }

    fn close(&mut self) {
        self.closed = true;
    }

    /// Setzt die `Next`-Ziele des letzten Segments auf ein festes Ziel
    /// (Verschmelzung eines unbedingten `->` mit `wait`/`until`, 6.2).
    fn fuse(&mut self, target: Target) -> bool {
        let Some(last) = self.segs.last_mut() else { return false };
        let mut fused = false;
        for t in &mut last.transitions {
            if matches!(t.target, SegTarget::Next) {
                t.target = SegTarget::Fixed(target);
                fused = true;
            }
        }
        fused
    }

    // ------------------------------------------------------------ Items

    fn items(&mut self, items: &[SeqItem]) -> Result<(), Diagnostic> {
        for item in items {
            self.item(item)?;
        }
        Ok(())
    }

    fn item(&mut self, item: &SeqItem) -> Result<(), Diagnostic> {
        match item {
            SeqItem::Stmt(s) => self.stmt(s),
            SeqItem::Wait(d) => {
                self.wait(d.clone());
                Ok(())
            }
            SeqItem::Until { guard, timeout, span } => {
                self.until(guard.clone(), timeout.clone(), *span);
                Ok(())
            }
            SeqItem::Expect { cond, message, span } => {
                self.expect(cond.clone(), message.clone(), *span);
                Ok(())
            }
            SeqItem::Repeat { count, counter, body, span } => self.repeat(count.clone(), *counter, body, *span),
            SeqItem::Step { name, body, .. } => {
                self.step(name);
                self.items(body)
            }
        }
    }

    /// Anweisung: `check` wird kontinuierlich, `->` beendet das Segment,
    /// alles andere gehoert in `enter:` des Segments.
    fn stmt(&mut self, s: &Stmt) -> Result<(), Diagnostic> {
        match &s.kind {
            StmtKind::Check { kind: CheckKind::Check, .. } => {
                self.cur().loop_stmts.push(s.clone());
                self.active_checks.push(s.clone());
                Ok(())
            }
            StmtKind::Goto(target) => {
                if self.closed && self.fuse(*target) {
                    self.left = true;
                    return Ok(());
                }
                let always = self.lit_bool(true, s.span);
                self.cur().transitions.push(Proto {
                    trigger: TransTrigger::When(Guard::Expr(always)),
                    actions: Vec::new(),
                    target: SegTarget::Fixed(*target),
                    span: s.span,
                });
                self.close();
                self.left = true;
                Ok(())
            }
            StmtKind::If { .. } if ends_with_goto(s) => {
                let mut protos = Vec::new();
                self.branches(Vec::new(), std::slice::from_ref(s), &mut protos)?;
                let seg = self.cur();
                seg.transitions.extend(protos);
                self.close();
                Ok(())
            }
            _ => {
                check_no_goto(s)?;
                self.cur().enter.push(s.clone());
                Ok(())
            }
        }
    }

    /// `wait d` → `after d: -> S_{i+1}`.
    fn wait(&mut self, d: Expr) {
        let span = d.span;
        self.cur().transitions.push(Proto {
            trigger: TransTrigger::After(d),
            actions: Vec::new(),
            target: SegTarget::Next,
            span,
        });
        self.close();
    }

    /// `until c [timeout d …]` → `when c: -> S_{i+1}` plus Timeout-Uebergang.
    fn until(&mut self, guard: Guard, timeout: Option<Timeout>, span: Span) {
        let seg = self.cur();
        seg.transitions.push(Proto {
            trigger: TransTrigger::When(guard),
            actions: Vec::new(),
            target: SegTarget::Next,
            span,
        });
        if let Some(t) = timeout {
            let (actions, target) = match t.action {
                TimeoutAction::Fault => (Vec::new(), SegTarget::Fixed(Target::Fault(FaultKind::Timeout))),
                TimeoutAction::Goto(x) => (Vec::new(), SegTarget::Fixed(x)),
                TimeoutAction::Else(block) => {
                    let mut stmts = block.stmts;
                    match stmts.last().map(|s| &s.kind) {
                        Some(StmtKind::Goto(x)) => {
                            let x = *x;
                            stmts.pop();
                            (stmts, SegTarget::Fixed(x))
                        }
                        _ => (stmts, SegTarget::Next),
                    }
                }
            };
            seg.transitions.push(Proto { trigger: TransTrigger::After(t.duration), actions, target, span });
        }
        self.close();
    }

    /// `expect e` → `check e` (Fault-Art `Expect`) in `loop:` von `S_i`,
    /// genau einmal im Entry-Tick, danach durch ein Flag deaktiviert.
    fn expect(&mut self, cond: Expr, message: Option<crate::pattern::Format>, span: Span) {
        let seg_id = self.cur().id;
        self.expect_count += 1;
        let flag = self.m.add_var(VarDef {
            name: format!("expect_{}", self.expect_count),
            ty: self.bool_ty,
            init: Some(self.lit_bool(true, span)),
            scope: VarScope::State(seg_id),
            public: false,
            span,
        });
        let check = Stmt::new(
            StmtKind::Check { cond, message, confirm: None, target: None, req: None, kind: CheckKind::Expect },
            span,
        );
        let clear = Stmt::new(StmtKind::Assign { target: Place::Var(flag), value: self.lit_bool(false, span) }, span);
        let guarded = Stmt::new(
            StmtKind::If {
                cond: self.var(flag, self.bool_ty, span),
                then: Block { stmts: vec![check, clear], span },
                otherwise: Block::default(),
            },
            span,
        );
        self.cur().loop_stmts.push(guarded);
    }

    /// `repeat n:` → Koerper in eigenen Segmenten; nach dem letzten Segment
    /// `when k + 1 < n: k += 1; -> S_first` sonst `k = 0; -> S_after`.
    fn repeat(&mut self, count: Expr, counter: VarId, body: &[SeqItem], span: Span) -> Result<(), Diagnostic> {
        if !self.closed && !self.cur_is_empty() {
            let always = self.lit_bool(true, span);
            self.cur().transitions.push(Proto {
                trigger: TransTrigger::When(Guard::Expr(always)),
                actions: Vec::new(),
                target: SegTarget::Next,
                span,
            });
            self.close();
        }
        let first = self.cur().id;
        self.items(body)?;
        let k = self.var(counter, self.int_ty, span);
        let one = Expr::new(ExprKind::Int(1), self.int_ty, span);
        let zero = Expr::new(ExprKind::Int(0), self.int_ty, span);
        let k_plus_1 = self.binary(BinaryOp::Add, k.clone(), one, self.int_ty, span);
        let more = self.binary(BinaryOp::Lt, k_plus_1.clone(), count, self.bool_ty, span);
        let again = Proto {
            trigger: TransTrigger::When(Guard::Expr(more)),
            actions: vec![Stmt::new(StmtKind::Assign { target: Place::Var(counter), value: k_plus_1 }, span)],
            target: SegTarget::Fixed(Target::State(first)),
            span,
        };
        let leave = Proto {
            trigger: TransTrigger::When(Guard::Expr(self.lit_bool(true, span))),
            actions: vec![Stmt::new(StmtKind::Assign { target: Place::Var(counter), value: zero }, span)],
            target: SegTarget::Next,
            span,
        };
        let seg = self.cur();
        seg.transitions.push(again);
        seg.transitions.push(leave);
        self.close();
        Ok(())
    }

    /// `step "name"` benennt das erste Segment des Koerpers.
    fn step(&mut self, name: &str) {
        if !self.closed && self.cur_is_empty() {
            let id = self.segs.last().expect("Segment offen").id;
            let state = &mut self.m.states[id.index()];
            if state.step_name.is_none() {
                state.step_name = Some(name.to_string());
                return;
            }
        }
        self.pending_name = Some(name.to_string());
    }

    /// Zweige einer `if`-Kette mit `->` (6.2): jeder Zweig wird zu
    /// `when <Bedingung des Zweigs>: <Aktionen>; -> X`, ein Zweig ohne `->`
    /// (oder ein fehlendes `else`) zu `when <Bedingung>: <Aktionen>; -> S_{i+1}`.
    fn branches(&mut self, prefix: Vec<Expr>, stmts: &[Stmt], out: &mut Vec<Proto>) -> Result<(), Diagnostic> {
        let span = stmts.last().map_or(Span::default(), |s| s.span);
        match stmts.last().map(|s| &s.kind) {
            Some(StmtKind::If { cond, then, otherwise }) if ends_with_goto(stmts.last().expect("letzte")) => {
                for s in &stmts[..stmts.len() - 1] {
                    check_no_goto(s)?;
                }
                if stmts.len() > 1 {
                    return Err(Diagnostic::error(CODE, span, "Anweisungen vor einem `if` mit `->` im selben Zweig")
                        .with_suggestion("die Anweisungen vor das `if` in der Sequenz ziehen"));
                }
                let mut then_prefix = prefix.clone();
                then_prefix.push(cond.clone());
                self.branches(then_prefix, &then.stmts, out)?;
                let mut else_prefix = prefix;
                else_prefix.push(self.unary(UnaryOp::Not, cond.clone(), span));
                self.branches(else_prefix, &otherwise.stmts, out)
            }
            Some(StmtKind::Goto(target)) => {
                let actions = stmts[..stmts.len() - 1].to_vec();
                for s in &actions {
                    check_no_goto(s)?;
                }
                let cond = self.conjunction(prefix, span);
                out.push(Proto {
                    trigger: TransTrigger::When(Guard::Expr(cond)),
                    actions,
                    target: SegTarget::Fixed(*target),
                    span,
                });
                Ok(())
            }
            _ => {
                for s in stmts {
                    check_no_goto(s)?;
                }
                let cond = self.conjunction(prefix, span);
                out.push(Proto {
                    trigger: TransTrigger::When(Guard::Expr(cond)),
                    actions: stmts.to_vec(),
                    target: SegTarget::Next,
                    span,
                });
                Ok(())
            }
        }
    }

    // ------------------------------------------------------------ Abschluss

    /// Schreibt die Segmente in die Zustaende; das letzte Segment ohne
    /// Uebergaenge haelt und setzt `done`.
    fn finish(mut self, span: Span) {
        let n = self.segs.len();
        let ids: Vec<StateId> = self.segs.iter().map(|s| s.id).collect();
        for (i, seg) in self.segs.drain(..).enumerate() {
            let holds = seg.transitions.is_empty();
            let state = &mut self.m.states[seg.id.index()];
            state.enter.stmts = seg.enter;
            if holds && i + 1 == n {
                state.enter.stmts.push(Stmt::new(
                    StmtKind::Assign {
                        target: Place::Var(self.done),
                        value: Expr::new(ExprKind::Bool(true), self.bool_ty, span),
                    },
                    span,
                ));
            }
            state.loop_block.stmts = seg.loop_stmts;
            state.transitions = seg
                .transitions
                .into_iter()
                .map(|p| Transition {
                    trigger: p.trigger,
                    actions: Block { stmts: p.actions, span: p.span },
                    target: match p.target {
                        SegTarget::Fixed(t) => t,
                        SegTarget::Next => Target::State(ids[i + 1]),
                    },
                    kind: TransKind::Weak,
                    span: p.span,
                })
                .collect();
        }
        let parent = &mut self.m.states[self.parent.index()];
        parent.initial = Some(ids[0]);
    }

    // ------------------------------------------------------------ Ausdruecke

    fn lit_bool(&self, b: bool, span: Span) -> Expr {
        Expr::new(ExprKind::Bool(b), self.bool_ty, span)
    }

    fn var(&self, v: VarId, ty: TypeId, span: Span) -> Expr {
        Expr::new(ExprKind::Var(v), ty, span)
    }

    fn unary(&self, op: UnaryOp, e: Expr, span: Span) -> Expr {
        Expr::new(ExprKind::Unary { op, expr: Box::new(e) }, self.bool_ty, span)
    }

    fn binary(&self, op: BinaryOp, l: Expr, r: Expr, ty: TypeId, span: Span) -> Expr {
        Expr::new(ExprKind::Binary { op, lhs: Box::new(l), rhs: Box::new(r) }, ty, span)
    }

    /// `a and b and …`; leer ist `true`.
    fn conjunction(&self, conds: Vec<Expr>, span: Span) -> Expr {
        let mut it = conds.into_iter();
        let Some(first) = it.next() else { return self.lit_bool(true, span) };
        it.fold(first, |acc, c| self.binary(BinaryOp::And, acc, c, self.bool_ty, span))
    }
}

/// Endet die Anweisung mit `->`: ein `Goto` oder ein `if`, in dem ein Zweig
/// mit `->` endet?
fn ends_with_goto(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Goto(_) => true,
        StmtKind::If { then, otherwise, .. } => {
            then.stmts.last().is_some_and(ends_with_goto) || otherwise.stmts.last().is_some_and(ends_with_goto)
        }
        _ => false,
    }
}

/// `->` darf in einer Sequenz nur am Ende eines Zweigs stehen (6.2).
fn check_no_goto(s: &Stmt) -> Result<(), Diagnostic> {
    let err = |span| {
        Diagnostic::error(CODE, span, "`->` in einer Sequenz nur als letzte Anweisung eines Zweigs")
            .with_suggestion("den Uebergang ans Ende des `if`-Zweigs stellen oder eine `when`-Transition schreiben")
    };
    match &s.kind {
        StmtKind::Goto(_) => Err(err(s.span)),
        StmtKind::If { then, otherwise, .. } => then.stmts.iter().chain(&otherwise.stmts).try_for_each(check_no_goto),
        StmtKind::ForRange { body, .. }
        | StmtKind::ForEach { body, .. }
        | StmtKind::Every { body, .. }
        | StmtKind::At { body, .. } => body.stmts.iter().try_for_each(check_no_goto),
        StmtKind::Match { arms, .. } => arms.iter().flat_map(|a| &a.body.stmts).try_for_each(check_no_goto),
        _ => Ok(()),
    }
}
