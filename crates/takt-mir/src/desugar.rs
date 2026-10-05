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
    let tick_ns = program.config.tick;
    let Program { types, machines, .. } = program;
    for m in machines.iter_mut() {
        desugar_machine(types, m, tick_ns)?;
    }
    Ok(())
}

/// Desugaring einer Maschine; danach gilt `Machine::is_core`.
pub fn desugar_machine(types: &mut TypeTable, m: &mut Machine, tick_ns: i64) -> Result<(), Diagnostic> {
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
            m.states[i].sequence_ticks = Some(ticks_of(&seq.items, m.period, tick_ns, types));
            Builder::new(types, m, StateId(i as u32), seq.done).run(seq)?;
        }
        i += 1;
    }
    Ok(())
}

/// Ziel eines Uebergangs waehrend des Aufbaus: das naechste Segment, ein
/// Segment nach seiner Nummer — auch eines, das erst entsteht — oder fest.
#[derive(Clone, Copy)]
enum SegTarget {
    Next,
    Seg(usize),
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
    /// Der Einmal-Block des letzten `expect` in `loop_stmts`: Was im
    /// Segment dahinter steht, laeuft darin nach der Pruefung (6.2).
    after_expect: Option<usize>,
}

/// Eine Anweisung an ihren Platz im Segment: in `enter:`, hinter einem
/// `expect` in dessen Einmal-Block (6.2).
fn push_after_expect(seg: &mut Seg, s: Stmt) {
    match seg.after_expect {
        Some(i) => match &mut seg.loop_stmts[i].kind {
            StmtKind::If { then, .. } => then.stmts.push(s),
            _ => unreachable!("der Einmal-Block eines `expect` ist ein `if`"),
        },
        None => seg.enter.push(s),
    }
}

struct Builder<'a> {
    m: &'a mut Machine,
    types: &'a TypeTable,
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
    /// Wo das naechste Segment beginnt: beim Item, das es eroeffnet, sonst
    /// bei der Sequenz (GEN-043).
    at: Span,
    bool_ty: TypeId,
    int_ty: TypeId,
}

impl<'a> Builder<'a> {
    fn new(types: &'a mut TypeTable, m: &'a mut Machine, parent: StateId, done: VarId) -> Self {
        let bool_ty = types.intern(Type::Bool);
        let int_ty = types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
        Builder {
            m,
            types,
            parent,
            done,
            segs: Vec::new(),
            active_checks: Vec::new(),
            pending_name: None,
            closed: true,
            left: false,
            expect_count: 0,
            at: Span::default(),
            bool_ty,
            int_ty,
        }
    }

    fn run(mut self, seq: Sequence) -> Result<(), Diagnostic> {
        self.at = seq.items.first().map_or(seq.span, SeqItem::span);
        self.open();
        self.items(&seq.items)?;
        if self.closed && !self.left {
            self.at = seq.span;
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
        state.span = self.at;
        let id = self.m.add_state(state);
        self.segs.push(Seg {
            id,
            enter: Vec::new(),
            loop_stmts: self.active_checks.clone(),
            transitions: Vec::new(),
            after_expect: None,
        });
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
    /// (Verschmelzung eines unbedingten `->` mit `wait`/`until`, 6.2) — und
    /// mit ihnen jedes Ziel, das auf das Segment zeigt, das nun nicht mehr
    /// entsteht (der Ausgang eines leeren `repeat`).
    fn fuse(&mut self, target: Target) -> bool {
        let coming = self.segs.len();
        let Some(last) = self.segs.last_mut() else { return false };
        let mut fused = false;
        for t in &mut last.transitions {
            if matches!(t.target, SegTarget::Next) {
                t.target = SegTarget::Fixed(target);
                fused = true;
            }
        }
        if fused {
            for t in self.segs.iter_mut().flat_map(|s| &mut s.transitions) {
                if matches!(t.target, SegTarget::Seg(k) if k == coming) {
                    t.target = SegTarget::Fixed(target);
                }
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
        self.at = item.span();
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
            SeqItem::Expect { cond, message, req, span } => {
                self.expect(cond.clone(), message.clone(), req.clone(), *span);
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
    /// alles andere gehoert in `enter:` des Segments — hinter einem `expect`
    /// in dessen Einmal-Block, nach der Pruefung (6.2, FB-283).
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
                push_after_expect(self.cur(), s.clone());
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
    /// genau einmal im Entry-Tick, danach durch ein Flag deaktiviert. Die
    /// Anweisungen dahinter im selben Segment folgen im selben Block: Sie
    /// laufen nur, wenn die Erwartung gilt (6.2).
    fn expect(&mut self, cond: Expr, message: Option<crate::pattern::Format>, req: Option<String>, span: Span) {
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
            StmtKind::Check { cond, message, confirm: None, within: None, target: None, req, kind: CheckKind::Expect },
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
        let seg = self.cur();
        seg.loop_stmts.push(guarded);
        seg.after_expect = Some(seg.loop_stmts.len() - 1);
    }

    /// `repeat n:` → Koerper in eigenen Segmenten; nach dem letzten Segment
    /// `when k + 1 < n: k += 1; -> S_first` sonst `k = 0; -> S_after`.
    ///
    /// Eine Zahl, die null sein kann (ein `param` mit `0` in seiner Range),
    /// laesst den Koerper dann aus (SYN-032, wie `for` ueber `range(0)`): Ein
    /// Eintrittssegment prueft vorher `when n < 1: -> S_after`, sonst
    /// `when true: -> S_first`. Ein Literal und eine Zahl, deren Range bei
    /// eins oder darueber beginnt, behalten ihre Form ([`at_least_one`]).
    fn repeat(&mut self, count: Expr, counter: VarId, body: &[SeqItem], span: Span) -> Result<(), Diagnostic> {
        let skip = if at_least_one(&count, self.types) {
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
            None
        } else {
            // Das offene Segment ist der Eintritt; ist keines offen, ein neues.
            let entry = self.segs.len() - usize::from(!self.closed);
            let one = Expr::new(ExprKind::Int(1), self.int_ty, span);
            let none = self.binary(BinaryOp::Lt, count.clone(), one, self.bool_ty, span);
            let always = self.lit_bool(true, span);
            let seg = self.cur();
            // Das Ziel des Auslassens steht erst nach dem Koerper fest.
            seg.transitions.push(Proto {
                trigger: TransTrigger::When(Guard::Expr(none)),
                actions: Vec::new(),
                target: SegTarget::Next,
                span,
            });
            let skip = seg.transitions.len() - 1;
            seg.transitions.push(Proto {
                trigger: TransTrigger::When(Guard::Expr(always)),
                actions: Vec::new(),
                target: SegTarget::Next,
                span,
            });
            self.close();
            Some((entry, skip))
        };
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
        // Ausgelassen wird in das Segment hinter dem `repeat`, das als
        // naechstes entsteht — oder, verschmilzt ein `->` mit dem Ausgang,
        // dorthin (`fuse`).
        if let Some((entry, skip)) = skip {
            let after = self.segs.len();
            self.segs[entry].transitions[skip].target = SegTarget::Seg(after);
        }
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
        let segs = std::mem::take(&mut self.segs);
        for (i, mut seg) in segs.into_iter().enumerate() {
            if seg.transitions.is_empty() && i + 1 == n {
                // `done` am Ende der Sequenz, wie jede Anweisung dort: hinter
                // einem `expect` erst nach der Pruefung.
                let done = Stmt::new(
                    StmtKind::Assign {
                        target: Place::Var(self.done),
                        value: Expr::new(ExprKind::Bool(true), self.bool_ty, span),
                    },
                    span,
                );
                push_after_expect(&mut seg, done);
            }
            let state = &mut self.m.states[seg.id.index()];
            state.enter.stmts = seg.enter;
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
                        SegTarget::Seg(k) => Target::State(ids[k]),
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

/// Ist die Zahl eines `repeat` sicher mindestens eins? Ein Literal ueber
/// null, oder ein Ausdruck, dessen Range — bewiesen oder aus dem Typ, etwa
/// `param N : int in 1..10` — bei eins oder darueber beginnt. Nur eine Zahl,
/// die null sein kann, braucht den Eintritt, der den Koerper auslaesst.
fn at_least_one(count: &Expr, types: &TypeTable) -> bool {
    let from_one = |r: &Option<crate::types::Range>| matches!(r, Some(r) if matches!(r.lo, crate::types::Const::Int(lo) if lo >= 1));
    match count.kind {
        ExprKind::Int(n) => n > 0,
        _ => {
            from_one(&count.range)
                || matches!(types.list.get(count.ty.index()), Some(Type::Int { range, .. }) if from_one(range))
        }
    }
}

/// Die Dauer einer Sequenz in Basis-Ticks (6.2, FB-129).
///
/// Segmente enden an `wait`, `until`, an einer Anweisung mit `->` und
/// am Ende eines `repeat`-Koerpers; jede Grenze ist ein Uebergang und
/// kostet eine Aktivierung. Ein `->` unmittelbar nach `wait`/`until`
/// verschmilzt mit deren Uebergang. Ein `repeat` mit Literal laeuft
/// `n`-mal, sonst bleibt das Ende offen.
fn ticks_of(items: &[SeqItem], period: u32, tick_ns: i64, types: &TypeTable) -> SequenceTicks {
    let act = u64::from(period.max(1));
    // Eine Dauer, die kein Literal ist (`wait BURN_DURATION`), kennt die
    // Sequenz nicht: mindestens eine Aktivierung, das Ende offen.
    let ticks = |e: &Expr| match e.kind {
        ExprKind::Duration(ns) if ns > 0 => {
            let t = (ns as u64).div_ceil((tick_ns.max(1) as u64).saturating_mul(act)).max(1).saturating_mul(act);
            (t, Some(t))
        }
        ExprKind::Duration(_) => (act, Some(act)),
        _ => (act, None),
    };
    let mut acc = SequenceTicks { min: 0, max: Some(0) };
    // Gesaettigt, und eine obere Schranke ueber u64 ist keine: Ein `repeat`
    // mit grosser Zahl panickte sonst im Debug-Bau.
    let add = |acc: &mut SequenceTicks, lo: u64, hi: Option<u64>| {
        acc.min = acc.min.saturating_add(lo);
        acc.max = acc.max.zip(hi).and_then(|(a, b)| a.checked_add(b));
    };
    let mut after_wait = false;
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                let goes = crate::stmt::Block::new(vec![s.clone()]).has_goto();
                if goes && !after_wait {
                    add(&mut acc, act, Some(act));
                }
                after_wait = false;
            }
            SeqItem::Wait(d) => {
                let (lo, hi) = ticks(d);
                add(&mut acc, lo, hi);
                after_wait = true;
            }
            SeqItem::Until { timeout, .. } => {
                add(&mut acc, act, timeout.as_ref().and_then(|t| ticks(&t.duration).1));
                after_wait = true;
            }
            SeqItem::Expect { .. } => {}
            SeqItem::Repeat { count, body, .. } => {
                let inner = ticks_of(body, period, tick_ns, types);
                let n = match count.kind {
                    ExprKind::Int(n) if n > 0 => Some(n as u64),
                    _ => None,
                };
                // Der Ruecksprung ist eine schwache Transition: je Durchlauf ein Tick.
                let per = inner.min.max(act);
                match n {
                    Some(n) => {
                        add(&mut acc, per, inner.max.and_then(|m| n.checked_mul(m.max(act))));
                        acc.min = acc.min.saturating_add(per.saturating_mul(n - 1));
                    }
                    None if at_least_one(count, types) => add(&mut acc, per, None),
                    // Eine Zahl, die null sein kann, verlangt keinen Durchlauf;
                    // der Eintritt kostet seinen Uebergang (SYN-032).
                    None => add(&mut acc, act, None),
                }
                after_wait = false;
            }
            SeqItem::Step { body, .. } => {
                let inner = ticks_of(body, period, tick_ns, types);
                add(&mut acc, inner.min, inner.max);
                after_wait = false;
            }
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ParamId;

    const MS: i64 = 1_000_000;

    fn dur(ns: i64) -> Expr {
        Expr::new(ExprKind::Duration(ns), TypeId(0), Span::default())
    }

    fn param() -> Expr {
        Expr::new(ExprKind::Param(ParamId(0)), TypeId(0), Span::default())
    }

    fn until(timeout: Option<Expr>) -> SeqItem {
        SeqItem::Until {
            guard: Guard::Expr(Expr::new(ExprKind::Bool(true), TypeId(0), Span::default())),
            timeout: timeout.map(|duration| Timeout { duration, action: TimeoutAction::Fault }),
            span: Span::default(),
        }
    }

    fn repeat(count: Expr, body: Vec<SeqItem>) -> SeqItem {
        SeqItem::Repeat { count, counter: VarId(0), body, span: Span::default() }
    }

    fn int(n: i64) -> Expr {
        Expr::new(ExprKind::Int(n), TypeId(0), Span::default())
    }

    fn of(items: &[SeqItem]) -> (u64, Option<u64>) {
        let t = ticks_of(items, 1, MS, &TypeTable::default());
        (t.min, t.max)
    }

    /// SYN-033: Eine Dauer, die kein Literal ist (`wait BURN_DURATION`, ein
    /// `param`), kennt die Sequenz nicht: Das Ende ist offen, nicht eine
    /// Aktivierung. Ein `repeat` mit grosser Zahl rechnet gesaettigt, statt
    /// im Debug-Bau zu panicken.
    #[test]
    fn an_unknown_duration_leaves_the_end_open() {
        assert_eq!(of(&[SeqItem::Wait(dur(25 * MS))]), (25, Some(25)));
        assert_eq!(of(&[SeqItem::Wait(param())]), (1, None), "wait mit Param");
        assert_eq!(of(&[until(Some(dur(10 * MS)))]), (1, Some(10)));
        assert_eq!(of(&[until(Some(param()))]), (1, None), "until mit Param als Timeout");
        assert_eq!(of(&[until(None)]), (1, None), "until ohne Timeout");
        assert_eq!(of(&[repeat(int(3), vec![SeqItem::Wait(dur(2 * MS))])]), (6, Some(6)));
        assert_eq!(of(&[repeat(param(), vec![SeqItem::Wait(dur(2 * MS))])]), (1, None), "repeat mit Param, auch null");
        let huge = of(&[repeat(int(i64::MAX), vec![SeqItem::Wait(dur(i64::MAX))])]);
        assert_eq!(huge, (u64::MAX, None), "gesaettigt statt uebergelaufen");
        // `->` direkt nach `wait` verschmilzt mit dessen Uebergang.
        let goto = SeqItem::Stmt(Stmt::new(StmtKind::Goto(crate::machine::Target::State(StateId(0))), Span::default()));
        assert_eq!(of(&[SeqItem::Wait(dur(3 * MS)), goto]), (3, Some(3)));
    }

    /// GEN-043: Ein Abschnittszustand traegt die Spanne des Items, das ihn
    /// eroeffnet, das Haltesegment hinter dem letzten Item die der Sequenz.
    /// Ohne Spanne nennt `Nicht erreicht` die Zeile 1 statt der Stelle.
    #[test]
    fn every_segment_starts_at_the_item_that_opens_it() {
        let at = |start: u32| Span { start, end: start + 4, ..Span::default() };
        let expect = |start| SeqItem::Expect {
            cond: Expr::new(ExprKind::Bool(true), TypeId(0), at(start)),
            message: None,
            req: None,
            span: at(start),
        };
        let wait = |start| SeqItem::Wait(Expr::new(ExprKind::Duration(MS), TypeId(0), at(start)));
        let mut m = Machine::new("m");
        let burn = m.add_state(State::new("BURN", None));
        let items = vec![expect(10), wait(20), expect(30), wait(40)];
        m.states[burn.index()].sequence = Some(Sequence { items, done: VarId(0), span: at(2) });
        desugar_machine(&mut TypeTable::default(), &mut m, MS).expect("Sequenz");
        let segs: Vec<(&str, u32)> = m.states[burn.index()]
            .children
            .iter()
            .map(|c| (m.states[c.index()].name.as_str(), m.states[c.index()].span.start))
            .collect();
        assert_eq!(segs, [("BURN.S0", 10), ("BURN.S1", 30), ("BURN.S2", 2)]);
    }
}
