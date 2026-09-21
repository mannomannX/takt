//! Darstellungsverengung (Referenz 3.4, Lemma 3.4).
//!
//! „Der Compiler waehlt die Darstellung eines `int` aus den bewiesenen
//! Intervallen (32 Bit, wo moeglich) — ohne Aenderung der Semantik."
//!
//! Die Verengung ist eine **Annotation** (`Expr::repr`), kein Typwechsel
//! (plan/m3.md 1.7): Der Interpreter liest das Feld nicht, also ist Lemma
//! 3.4 im Interpreter eine Tautologie und die Beweislast liegt dort, wo sie
//! hingehoert — im Codegen, wo der differentielle Test sie prueft.

use crate::Program;
use crate::analysis::domain::Interval;
use crate::expr::{Expr, ExprKind, Repr};
use crate::ids::{BlockId, FnId};
use crate::machine::{Guard, TransTrigger, Transition};
use crate::stmt::{Block, Method, Place, Stmt, StmtKind};
use crate::types::Type;

/// Zaehler des Reports und die Funktionen, die der besuchte Code ruft.
#[derive(Default)]
struct Count {
    narrowed: u32,
    total: u32,
    /// `step` und Methoden je Block, fuer `BlockInit`.
    block_fns: Vec<Vec<FnId>>,
    calls: Vec<FnId>,
}

impl Count {
    fn block(&mut self, block: BlockId) {
        self.calls.extend_from_slice(&self.block_fns[block.index()]);
    }

    /// Aufrufe in einem Ausdruck, der selbst nicht verengt wird
    /// (Initialwerte, Guards).
    fn calls_in(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call { callee, .. } => self.calls.push(*callee),
            ExprKind::BlockInit { block, .. } => self.block(*block),
            _ => {}
        }
        for c in e.children() {
            self.calls_in(c);
        }
    }
}

/// Annotiert jeden Integer-Ausdruck mit seiner Darstellung. Liefert
/// `(verengt, insgesamt)` fuer den Report. Funktionen zaehlen, wenn eine
/// Maschine sie erreicht: Bibliothekscode, den niemand ruft, gehoert nicht
/// zum Programm (Lemma 3.4).
pub fn narrow(program: &mut Program) -> (u32, u32) {
    let types = program.types.list.clone();
    let block_fns =
        program.blocks.iter().map(|b| b.step.into_iter().chain(b.methods.iter().copied()).collect()).collect();
    let mut n = Count { block_fns, ..Count::default() };
    for m in &mut program.machines {
        for init in m.vars.iter().filter_map(|v| v.init.as_ref()) {
            n.calls_in(init);
        }
        narrow_block(&mut m.loop_block, &types, &mut n);
        for h in &mut m.handlers {
            if let Some(g) = &h.guard {
                n.calls_in(g);
            }
            narrow_block(&mut h.body, &types, &mut n);
        }
        for t in &mut m.faulted.transitions {
            narrow_transition(t, &types, &mut n);
        }
        for s in &mut m.states {
            narrow_block(&mut s.enter, &types, &mut n);
            narrow_block(&mut s.exit, &types, &mut n);
            narrow_block(&mut s.loop_block, &types, &mut n);
            for h in &mut s.handlers {
                if let Some(g) = &h.guard {
                    n.calls_in(g);
                }
                narrow_block(&mut h.body, &types, &mut n);
            }
            for t in &mut s.transitions {
                narrow_transition(t, &types, &mut n);
            }
        }
    }
    let mut seen = vec![false; program.fns.len()];
    while let Some(f) = n.calls.pop() {
        if !std::mem::replace(&mut seen[f.index()], true) {
            let f = &mut program.fns[f.index()];
            for init in f.locals.iter().filter_map(|v| v.init.as_ref()) {
                n.calls_in(init);
            }
            narrow_block(&mut f.body, &types, &mut n);
        }
    }
    (n.narrowed, n.total)
}

fn narrow_transition(t: &mut Transition, types: &[Type], n: &mut Count) {
    match &t.trigger {
        TransTrigger::When(Guard::Expr(e)) => n.calls_in(e),
        TransTrigger::When(Guard::Match { subject, .. }) => n.calls_in(subject),
        TransTrigger::After(e) => n.calls_in(e),
        TransTrigger::When(Guard::Next { .. }) => {}
    }
    narrow_block(&mut t.actions, types, n);
}

fn narrow_block(b: &mut Block, types: &[Type], n: &mut Count) {
    for s in &mut b.stmts {
        narrow_stmt(s, types, n);
    }
}

fn narrow_stmt(s: &mut Stmt, types: &[Type], n: &mut Count) {
    match &mut s.kind {
        StmtKind::Assign { target, value } => {
            narrow_place(target, types, n);
            narrow_expr(value, types, n);
        }
        StmtKind::Check { cond, message, confirm, .. } => {
            narrow_expr(cond, types, n);
            let _ = message;
            if let Some(c) = confirm {
                narrow_expr(&mut c.duration, types, n);
            }
        }
        StmtKind::If { cond, then, otherwise } => {
            narrow_expr(cond, types, n);
            narrow_block(then, types, n);
            narrow_block(otherwise, types, n);
        }
        StmtKind::ForRange { count, body, .. } => {
            narrow_expr(count, types, n);
            narrow_block(body, types, n);
        }
        StmtKind::ForEach { iter, body, .. } => {
            narrow_expr(iter, types, n);
            narrow_block(body, types, n);
        }
        StmtKind::Match { subject, arms } => {
            narrow_expr(subject, types, n);
            for a in arms {
                narrow_block(&mut a.body, types, n);
            }
        }
        StmtKind::Every { period, body, .. } => {
            narrow_expr(period, types, n);
            narrow_block(body, types, n);
        }
        StmtKind::At { time, body } => {
            narrow_expr(time, types, n);
            narrow_block(body, types, n);
        }
        StmtKind::Send { value, .. } | StmtKind::Return(value) => narrow_expr(value, types, n),
        StmtKind::MethodCall { target, method, args, .. } => {
            if let Method::Block(f) = method {
                n.calls.push(*f);
            }
            if let Some(t) = target {
                narrow_place(t, types, n);
            }
            for a in args {
                narrow_expr(a, types, n);
            }
        }
        StmtKind::Job { args, .. } => {
            for a in args {
                narrow_expr(a, types, n);
            }
        }
        StmtKind::Observe(o) => narrow_observe(o, types, n),
        _ => {}
    }
}

fn narrow_observe(o: &mut crate::stmt::Observe, types: &[Type], n: &mut Count) {
    use crate::stmt::Observe;
    match o {
        Observe::Alert { cond, confirm, .. } => {
            narrow_expr(cond, types, n);
            if let Some(c) = confirm {
                narrow_expr(&mut c.duration, types, n);
            }
        }
        Observe::Measure { value, .. } => narrow_expr(value, types, n),
        Observe::Verify { cond, .. } => narrow_expr(cond, types, n),
        _ => {}
    }
}

fn narrow_place(p: &mut Place, types: &[Type], n: &mut Count) {
    match p {
        Place::Var(_) | Place::Output(_) | Place::Port(_) => {}
        Place::Field(b, _) => narrow_place(b, types, n),
        Place::Index(b, i) => {
            narrow_place(b, types, n);
            narrow_expr(i, types, n);
        }
        Place::Index2(b, r, c) => {
            narrow_place(b, types, n);
            narrow_expr(r, types, n);
            narrow_expr(c, types, n);
        }
    }
}

/// Ein Ausdruck: erst die Kinder, dann er selbst.
fn narrow_expr(e: &mut Expr, types: &[Type], n: &mut Count) {
    match &e.kind {
        ExprKind::Call { callee, .. } => n.calls.push(*callee),
        ExprKind::BlockInit { block, .. } => n.block(*block),
        _ => {}
    }
    for c in e.children_mut() {
        narrow_expr(c, types, n);
    }
    let Some(Type::Int { width, .. }) = types.get(e.ty.index()) else { return };
    // Nur `int`/`i64` ist zu verengen; schmale Typen sind schon schmal.
    if width.bits() < 64 {
        return;
    }
    n.total += 1;
    let fits = match e.range {
        Some(r) => Interval::from_range(&r).fits_i32(),
        None => false,
    };
    if fits {
        e.repr = Some(Repr::I32);
        n.narrowed += 1;
    } else {
        e.repr = Some(Repr::I64);
    }
}
