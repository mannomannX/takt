//! Anwenden dessen, was der Durchlauf bewiesen hat (Referenz 3.4).
//!
//! Der Durchlauf (`walk`) liest die MIR und sammelt zwei Listen: Stellen, an
//! denen eine implizite Pruefung entfallen darf, und bewiesene Intervalle.
//! Dieser Durchlauf schreibt sie zurueck — getrennt, weil die Analyse die
//! MIR nur lesen soll und ein `&mut` waehrend der Auswertung die
//! Verzweigungslogik verkompliziert haette.

use std::collections::{BTreeMap, BTreeSet};

use takt_diag::Span;

use crate::Program;
use crate::expr::{Expr, ExprKind};
use crate::stmt::{Block, Place, Stmt, StmtKind};
use crate::types::Range;

/// Was der Durchlauf bewiesen hat.
#[derive(Default)]
pub struct Proofs {
    /// Stellen mit erlassener Pruefung.
    pub dropped: BTreeSet<(u32, u32)>,
    /// Bewiesenes Intervall je Stelle.
    pub ranges: BTreeMap<(u32, u32), Range>,
}

impl Proofs {
    /// Sammelt die Ergebnisse eines Durchlaufs.
    pub fn add(&mut self, proven: &[Span], ranges: &[(Span, Range)]) {
        for s in proven {
            self.dropped.insert(key(*s));
        }
        for (s, r) in ranges {
            self.ranges.insert(key(*s), *r);
        }
    }

    /// Ist die Pruefung an dieser Stelle bewiesen?
    fn is_dropped(&self, s: Span) -> bool {
        self.dropped.contains(&key(s))
    }

    /// Das bewiesene Intervall an dieser Stelle.
    fn range_at(&self, s: Span) -> Option<Range> {
        self.ranges.get(&key(s)).copied()
    }
}

/// Eine Stelle als Schluessel; die Datei-Id gehoert dazu, weil das Prelude
/// dieselben Offsets belegen kann.
fn key(s: Span) -> (u32, u32) {
    (s.file.0, s.start)
}

/// Schreibt die Beweise in die MIR: erlassene Pruefungen verschwinden,
/// bewiesene Intervalle stehen in `Expr::range`.
pub fn apply(program: &mut Program, p: &Proofs) {
    for m in &mut program.machines {
        block(&mut m.loop_block, p);
        for h in &mut m.handlers {
            block(&mut h.body, p);
        }
        for t in &mut m.faulted.transitions {
            block(&mut t.actions, p);
        }
        for s in &mut m.states {
            block(&mut s.enter, p);
            block(&mut s.exit, p);
            block(&mut s.loop_block, p);
            for h in &mut s.handlers {
                block(&mut h.body, p);
            }
            for t in &mut s.transitions {
                block(&mut t.actions, p);
            }
        }
    }
    for f in &mut program.fns {
        block(&mut f.body, p);
    }
}

fn block(b: &mut Block, p: &Proofs) {
    for s in &mut b.stmts {
        stmt(s, p);
    }
}

fn stmt(s: &mut Stmt, p: &Proofs) {
    match &mut s.kind {
        StmtKind::Assign { target, value } => {
            place(target, p);
            expr(value, p);
        }
        StmtKind::Check { cond, confirm, .. } => {
            expr(cond, p);
            if let Some(c) = confirm {
                expr(&mut c.duration, p);
            }
        }
        StmtKind::If { cond, then, otherwise } => {
            expr(cond, p);
            block(then, p);
            block(otherwise, p);
        }
        StmtKind::ForRange { count, body, .. } => {
            expr(count, p);
            block(body, p);
        }
        StmtKind::ForEach { iter, body, .. } => {
            expr(iter, p);
            block(body, p);
        }
        StmtKind::Match { subject, arms } => {
            expr(subject, p);
            for a in arms {
                block(&mut a.body, p);
            }
        }
        StmtKind::Every { period, body, .. } => {
            expr(period, p);
            block(body, p);
        }
        StmtKind::At { time, body } => {
            expr(time, p);
            block(body, p);
        }
        StmtKind::Send { value, .. } | StmtKind::Return(value) => expr(value, p),
        StmtKind::MethodCall { target, args, .. } => {
            if let Some(t) = target {
                place(t, p);
            }
            for a in args {
                expr(a, p);
            }
        }
        StmtKind::Job { args, .. } => {
            for a in args {
                expr(a, p);
            }
        }
        StmtKind::Observe(o) => observe(o, p),
        _ => {}
    }
}

fn observe(o: &mut crate::stmt::Observe, p: &Proofs) {
    use crate::stmt::Observe;
    match o {
        Observe::Alert { cond, confirm, .. } => {
            expr(cond, p);
            if let Some(c) = confirm {
                expr(&mut c.duration, p);
            }
        }
        Observe::Measure { value, .. } => expr(value, p),
        Observe::Verify { cond, .. } => expr(cond, p),
        _ => {}
    }
}

fn place(pl: &mut Place, p: &Proofs) {
    match pl {
        Place::Var(_) | Place::Output(_) => {}
        Place::Field(b, _) => place(b, p),
        Place::Index(b, i) => {
            place(b, p);
            expr(i, p);
        }
        Place::Index2(b, r, c) => {
            place(b, p);
            expr(r, p);
            expr(c, p);
        }
    }
}

/// Ein Ausdruck: erst die Kinder, dann die eigene Pruefung und Annotation.
fn expr(e: &mut Expr, p: &Proofs) {
    for c in e.children_mut() {
        expr(c, p);
    }
    // Eine bewiesene Pruefung faellt weg; der Ausdruck darunter tritt an
    // ihre Stelle (3.4).
    if matches!(e.kind, ExprKind::Checked { .. }) && p.is_dropped(e.span) {
        let ExprKind::Checked { expr: inner, .. } = std::mem::replace(&mut e.kind, ExprKind::Bool(false)) else {
            unreachable!("gerade geprueft")
        };
        let ty = e.ty;
        *e = *inner;
        // Der Typ des Ganzen bleibt der der Pruefung: sie hat verengt.
        e.ty = ty;
    }
    if e.range.is_none() {
        e.range = p.range_at(e.span);
    }
}
