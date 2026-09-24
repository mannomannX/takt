//! Der Durchlauf ueber die Anweisungen (Referenz 3.4, 9.2).
//!
//! Straight-Line-Code plus beschraenkte Schleifen, ohne Fixpunkt ueber Ticks
//! hinweg — 3.4: „Die Analyse eines Ticks ist deshalb lokal." Jede
//! `for`-Schleife wird abgerollt, wenn `n · |Koerper|` unter der Schwelle
//! liegt, sonst gewidet.

use takt_diag::Span;

use crate::analysis::domain::{Domain, Interval, Intervals};
use crate::analysis::facts::Facts;
use crate::expr::{Accessor, BinaryOp, CheckedKind, Expr, ExprKind, UnaryOp};
use crate::stmt::{Block, Place, Stmt, StmtKind};
use crate::types::{Const, IntWidth, Range, Type};
use crate::{Program, VarId};

/// Ab dieser Zahl abgerollter Anweisungen wird eine Schleife gewidet statt
/// abgerollt (3.4: „`for`-Schleifen mit n·|Koerper| unterhalb einer
/// Schwelle werden abgerollt analysiert"). Der Wert ist eine Konstante des
/// Compilers und erscheint im Report.
pub const UNROLL_LIMIT: u64 = 256;

/// Warum eine implizite Pruefung noetig war (3.4, Kennzahl nach Ursache).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckCause {
    /// Zuweisung in eine engere Range.
    Declared,
    /// Array-, `vec`- oder Slice-Zugriff.
    Index,
    /// `as`-Konversion.
    Convert,
    /// Division, Overflow, Shift.
    Arith,
}

impl CheckCause {
    /// Name im Report.
    pub fn name(self) -> &'static str {
        match self {
            CheckCause::Declared => "Declared",
            CheckCause::Index => "Index",
            CheckCause::Convert => "Convert",
            CheckCause::Arith => "Arith",
        }
    }
}

/// Eine implizite Pruefung, die stehen bleibt.
#[derive(Clone, Copy, Debug)]
pub struct ImplicitCheck {
    /// Ursache.
    pub cause: CheckCause,
    /// Stelle.
    pub span: Span,
    /// Steht sie in einer `for`-Schleife oder einem Aktionsblock? Nur dann
    /// entsteht eine Warnung im engeren Sinn (3.4, Warnpolitik).
    pub warns: bool,
    /// Ein Oktagon-Kandidat (plan/m6.md 2.12): Der Ausdruck haengt an zwei
    /// Variablen, oder ein dominierender Vergleich hat seine Variable zu
    /// einer anderen in Beziehung gesetzt — nur dort koennte `±x ± y <= c`
    /// beweisen, was ein Intervall nicht kann.
    pub relational: bool,
}

/// Der Zustand eines Durchlaufs.
pub struct Walk<'p> {
    /// Das Programm, fuer Typen und Deklarationen.
    pub program: &'p Program,
    /// Gefundene implizite Pruefungen.
    pub checks: Vec<ImplicitCheck>,
    /// Stellen und Art der Pruefungen, die die Analyse erlassen hat; der
    /// zweite Durchlauf (`prove`) streicht sie.
    pub proven: Vec<(Span, u8)>,
    /// Bewiesenes Intervall je Stelle, fuer die Annotation `Expr::range`.
    pub ranges: Vec<(Span, Range)>,
    /// Tiefe der `for`-Schleifen: entscheidet ueber die Warnung.
    loop_depth: u32,
    /// Steht der Durchlauf in einem Aktionsblock (5.5)?
    in_action: bool,
    /// Deklarierte Range je Variable, fuer die Weitung.
    declared: Vec<Option<Range>>,
}

impl<'p> Walk<'p> {
    /// Ein Durchlauf ueber die Variablen einer Maschine oder Funktion.
    pub fn new(program: &'p Program, declared: Vec<Option<Range>>) -> Walk<'p> {
        Walk {
            program,
            checks: Vec::new(),
            proven: Vec::new(),
            ranges: Vec::new(),
            loop_depth: 0,
            in_action: false,
            declared,
        }
    }

    /// Betritt einen Aktionsblock (`enter`, `exit`, Transitionsaktion,
    /// `at`): dort warnt jede implizite Pruefung (3.4).
    pub fn action<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let was = std::mem::replace(&mut self.in_action, true);
        let out = f(self);
        self.in_action = was;
        out
    }

    /// Ein Block: die Anweisungen der Reihe nach.
    pub fn block(&mut self, b: &Block, f: &mut Facts) {
        for s in &b.stmts {
            if !f.is_reachable() {
                break;
            }
            self.stmt(s, f);
        }
    }

    /// Eine Anweisung (9.2).
    pub fn stmt(&mut self, s: &Stmt, f: &mut Facts) {
        match &s.kind {
            StmtKind::Assign { target, value } => {
                let i = self.expr(value, f);
                if let Place::Var(v) = target {
                    f.assign(*v, i);
                } else {
                    self.place(target, f);
                }
            }
            StmtKind::Check { cond, .. } => {
                self.expr(cond, f);
                // Ein bestandener `check` verfeinert das Folgende (5.6).
                self.refine(cond, true, f);
            }
            StmtKind::If { cond, then, otherwise } => {
                self.expr(cond, f);
                let mut a = f.clone();
                self.refine(cond, true, &mut a);
                self.block(then, &mut a);
                let mut b = f.clone();
                self.refine(cond, false, &mut b);
                self.block(otherwise, &mut b);
                *f = Facts::join::<Intervals>(&a, &b);
            }
            StmtKind::ForRange { var, count, body } => {
                let n = self.expr(count, f);
                self.for_loop(*var, n, body, f);
            }
            StmtKind::ForEach { vars, iter, body } => {
                self.expr(iter, f);
                // Die Laufvariablen sind bei jedem Durchlauf gesetzt, ihr
                // Wert ist aber unbekannt.
                for v in vars.ids() {
                    f.declare(v, Interval::Top);
                }
                self.bounded_body(body, f);
            }
            StmtKind::Match { subject, arms } => {
                self.expr(subject, f);
                let mut out: Option<Facts> = None;
                for a in arms {
                    let mut branch = f.clone();
                    for v in a.pattern.bound() {
                        branch.declare(v, Interval::Top);
                    }
                    self.block(&a.body, &mut branch);
                    out = Some(match out {
                        Some(prev) => Facts::join::<Intervals>(&prev, &branch),
                        None => branch,
                    });
                }
                if let Some(o) = out {
                    *f = o;
                }
            }
            StmtKind::Every { period, body, .. } => {
                self.expr(period, f);
                // Der Koerper laeuft nicht in jeder Aktivierung; danach gilt
                // nur, was auch ohne ihn galt.
                let mut inner = f.clone();
                self.block(body, &mut inner);
                *f = Facts::join::<Intervals>(f, &inner);
            }
            StmtKind::At { time, body } => {
                self.expr(time, f);
                // 5.5: ein `at`-Block ist ein Aktionsblock.
                let mut inner = f.clone();
                self.action(|w| w.block(body, &mut inner));
                *f = Facts::join::<Intervals>(f, &inner);
            }
            StmtKind::Send { value, .. } => {
                self.expr(value, f);
            }
            StmtKind::Return(e) => {
                self.expr(e, f);
                f.cut();
            }
            StmtKind::Abort { .. } | StmtKind::Goto(_) | StmtKind::Break => f.cut(),
            StmtKind::Observe(o) => self.observe(o, f),
            StmtKind::MethodCall { target, args, .. } => {
                for a in args {
                    self.expr(a, f);
                }
                if let Some(Place::Var(v)) = target {
                    f.assign(*v, Interval::Top);
                }
            }
            StmtKind::Job { handle, args, .. } => {
                for a in args {
                    self.expr(a, f);
                }
                f.assign(*handle, Interval::Top);
            }
            StmtKind::Cancel(_) | StmtKind::Skip(_) | StmtKind::Raise(_) | StmtKind::Arm { .. } | StmtKind::Pass => {}
        }
    }

    /// Beobachtungen lesen nur (5.6, Entscheidung 14).
    fn observe(&mut self, o: &crate::stmt::Observe, f: &mut Facts) {
        use crate::stmt::Observe;
        match o {
            Observe::Alert { cond, .. } => {
                self.expr(cond, f);
            }
            Observe::Measure { value, .. } => {
                self.expr(value, f);
            }
            Observe::Verify { cond, .. } => {
                self.expr(cond, f);
            }
            Observe::Log(_) | Observe::Verdict { .. } => {}
        }
    }

    /// `for i in range(n)`: abrollen oder weiten (3.4).
    fn for_loop(&mut self, var: VarId, count: Interval, body: &Block, f: &mut Facts) {
        let n = match count {
            Interval::Int { hi, .. } if hi >= 0 => hi as u64,
            _ => 0,
        };
        let size = n.saturating_mul(body.stmts.len() as u64);
        if n > 0 && size <= UNROLL_LIMIT {
            // Abgerollt: der Index ist in jedem Durchlauf bekannt.
            for k in 0..n {
                f.declare(var, Interval::point(i128::from(k)));
                self.bounded_body(body, f);
                if !f.is_reachable() {
                    // `break` beendet die Schleife, nicht das Folgende.
                    break;
                }
            }
            let _ = f.is_reachable();
            f.declare(var, Interval::Int { lo: 0, hi: i128::from(n.saturating_sub(1)) });
        } else {
            f.declare(var, if n > 0 { Interval::Int { lo: 0, hi: i128::from(n - 1) } } else { Interval::Top });
            let written = written_vars(body);
            let declared: Vec<Option<Range>> = self.declared.clone();
            f.widen::<Intervals>(&written, |v| {
                declared.get(v.0 as usize).and_then(|r| r.as_ref()).map_or(Interval::Top, Interval::from_range)
            });
            self.bounded_body(body, f);
        }
        // Eine Schleife verlaesst man immer; `break` schneidet nur sie ab.
        f_reachable(f);
    }

    /// Der Koerper einer Schleife: dort warnt jede implizite Pruefung (3.4).
    fn bounded_body(&mut self, body: &Block, f: &mut Facts) {
        self.loop_depth += 1;
        let mut inner = f.clone();
        self.block(body, &mut inner);
        self.loop_depth -= 1;
        // Nach der Schleife gilt, was der Koerper *und* das Ueberspringen
        // gemeinsam tragen.
        *f = Facts::join::<Intervals>(f, &inner);
    }

    /// Verfeinert die Fakten an einer Bedingung (3.4).
    fn refine(&mut self, cond: &Expr, taken: bool, f: &mut Facts) {
        match &cond.kind {
            // De Morgan: `a and b` gilt ganz, `not (a or b)` verneint beide.
            ExprKind::Binary { op: BinaryOp::And, lhs, rhs } if taken => {
                self.refine(lhs, true, f);
                self.refine(rhs, true, f);
            }
            ExprKind::Binary { op: BinaryOp::Or, lhs, rhs } if !taken => {
                self.refine(lhs, false, f);
                self.refine(rhs, false, f);
            }
            ExprKind::Binary { op: BinaryOp::And | BinaryOp::Or, .. } => {}
            ExprKind::Binary { op, lhs, rhs } => {
                let effective = if taken { *op } else { negate(*op) };
                if !taken && effective == *op {
                    return;
                }
                self.refine_cmp(effective, lhs, rhs, f);
            }
            ExprKind::Unary { op: UnaryOp::Not, expr } => self.refine(expr, !taken, f),
            // 3.8: `x.valid` dominiert den Zweig.
            ExprKind::Accessor { base, accessor, .. } if taken && is_validity(*accessor) => {
                if let Some(key) = dom_key(base) {
                    f.dominate(key);
                }
            }
            _ => {}
        }
    }

    /// `a < b` und Verwandte: beide Seiten verfeinern, wenn eine eine
    /// Variable ist.
    fn refine_cmp(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, f: &mut Facts) {
        if op == BinaryOp::And {
            self.refine(lhs, true, f);
            self.refine(rhs, true, f);
            return;
        }
        if let (ExprKind::Var(a), ExprKind::Var(b)) = (&lhs.kind, &rhs.kind) {
            f.relate(*a, *b);
        }
        let (l, r) = (self.eval_only(lhs, f), self.eval_only(rhs, f));
        if let (ExprKind::Var(v), Interval::Int { lo, hi }) = (&lhs.kind, r) {
            f.refine(*v, bound(op, lo, hi));
        }
        if let (Interval::Int { lo, hi }, ExprKind::Var(v)) = (l, &rhs.kind) {
            f.refine(*v, bound(flip(op), lo, hi));
        }
    }

    /// Wertet einen Ausdruck aus, ohne Pruefungen zu zaehlen.
    fn eval_only(&mut self, e: &Expr, f: &Facts) -> Interval {
        let mut probe = Walk::new(self.program, self.declared.clone());
        let mut copy = f.clone();
        probe.expr(e, &mut copy)
    }

    /// Eine Stelle: die Indizes darin sind Ausdruecke.
    fn place(&mut self, p: &Place, f: &mut Facts) {
        match p {
            Place::Var(_) | Place::Output(_) | Place::Port(_) => {}
            Place::Field(b, _) => self.place(b, f),
            Place::Index(b, i) => {
                self.place(b, f);
                self.expr(i, f);
            }
            Place::Index2(b, r, c) => {
                self.place(b, f);
                self.expr(r, f);
                self.expr(c, f);
            }
        }
    }

    /// Das Intervall eines Ausdrucks; zaehlt dabei die impliziten Pruefungen,
    /// die stehen bleiben.
    pub fn expr(&mut self, e: &Expr, f: &mut Facts) -> Interval {
        // Was der Ausdruck selbst hergibt, geschnitten mit dem, was sein Typ
        // zulaesst (3.2, 3.4): Ein `u16` liegt in `0..65535`, auch wenn die
        // Herleitung nichts Engeres findet.
        let i = self.expr_inner(e, f).meet(self.bounds_of(e.ty));
        if let Some(r) = i.to_range() {
            self.ranges.push((e.span, r));
        }
        i
    }

    fn expr_inner(&mut self, e: &Expr, f: &mut Facts) -> Interval {
        match &e.kind {
            ExprKind::Int(v) => Interval::point(i128::from(*v)),
            ExprKind::Duration(v) => Interval::point(i128::from(*v)),
            ExprKind::Bool(_) | ExprKind::Float(_) => Interval::Top,
            ExprKind::Builtin(crate::expr::Builtin::Tick) => Interval::point(i128::from(self.program.config.tick)),
            ExprKind::Var(v) => f.interval(*v),
            ExprKind::Unary { op, expr } => {
                let i = self.expr(expr, f);
                match op {
                    UnaryOp::Neg => -i,
                    _ => Interval::Top,
                }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let (a, b) = (self.expr(lhs, f), self.expr(rhs, f));
                match op {
                    BinaryOp::Add => a + b,
                    BinaryOp::Sub => a - b,
                    BinaryOp::Mul => a * b,
                    BinaryOp::Div => a / b,
                    BinaryOp::Rem => a % b,
                    BinaryOp::Shr => shr(a, b),
                    BinaryOp::BitAnd => bit_and(a, b),
                    _ => Interval::Top,
                }
            }
            ExprKind::Cond { cond, then, otherwise } => {
                self.expr(cond, f);
                let mut a = f.clone();
                self.refine(cond, true, &mut a);
                let x = self.expr(then, &mut a);
                let mut b = f.clone();
                self.refine(cond, false, &mut b);
                let y = self.expr(otherwise, &mut b);
                Intervals::join(&x, &y)
            }
            ExprKind::Checked { expr, kind } => self.checked(e, expr, kind, f),
            // 3.9: Die Laenge liegt zwischen null und der Kapazitaet.
            ExprKind::Accessor { base, accessor: Accessor::Len | Accessor::Count, args } => {
                self.expr(base, f);
                for a in args {
                    self.expr(a, f);
                }
                capacity_of(self.program, base.ty)
            }
            ExprKind::Index { base, index } => {
                self.expr(base, f);
                self.expr(index, f);
                Interval::Top
            }
            ExprKind::Field { base, .. } => {
                self.expr(base, f);
                Interval::Top
            }
            ExprKind::Cast { expr, to } => {
                let i = self.expr(expr, f);
                // Eine Verengung, die nachweislich passt, aendert nichts;
                // sonst gilt danach, was der Zieltyp zulaesst (3.10).
                let b = self.bounds_of(*to);
                // Der Schnitt: eine Erweiterung behaelt das engere Wissen der
                // Quelle, eine Verengung klemmt auf den Zieltyp.
                i.meet(b)
            }
            ExprKind::Intrinsic { op, args } => {
                let vals: Vec<Interval> = args.iter().map(|a| self.expr(a, f)).collect();
                match (op, vals.as_slice()) {
                    (crate::expr::Intrinsic::Abs, [x]) => x.abs(),
                    (crate::expr::Intrinsic::Min, [x, y]) => min_of(*x, *y),
                    (crate::expr::Intrinsic::Max, [x, y]) => max_of(*x, *y),
                    _ => Interval::Top,
                }
            }
            other => {
                walk_children(other, &mut |c| {
                    self.expr(c, f);
                });
                Interval::Top
            }
        }
    }

    /// Eine implizite Pruefung: entfaellt sie, oder bleibt sie stehen?
    /// Liefert das Intervall hinter der bestandenen Pruefung.
    fn checked(&mut self, node: &Expr, inner: &Expr, kind: &CheckedKind, f: &mut Facts) -> Interval {
        let relational = relational(inner, f);
        let contained = |i: Interval, r: Interval| i != Interval::Top && i.meet(r) == i;
        let (cause, proven, result) = match kind {
            // Validitaet und Wrapper entscheidet die Dominanz im Sema (3.5, 3.8).
            CheckedKind::Valid | CheckedKind::Missing => return self.expr(inner, f),
            CheckedKind::Range(r) => {
                let i = self.expr(inner, f);
                (CheckCause::Declared, i.fits(r), Interval::from_range(r))
            }
            // Der Knoten umschliesst den Zugriff oder (an einer Stelle) den Index.
            CheckedKind::Index { len } => {
                let (i, value) = match &inner.kind {
                    ExprKind::Index { index, .. } => {
                        self.expr(inner, f);
                        (self.eval_only(index, f), Interval::Top)
                    }
                    _ => {
                        let i = self.expr(inner, f);
                        (i, i)
                    }
                };
                let r = Interval::Int { lo: 0, hi: i128::from(*len) - 1 };
                (CheckCause::Index, *len > 0 && contained(i, r), if *len > 0 { value.meet(r) } else { value })
            }
            CheckedKind::Convert => match &inner.kind {
                ExprKind::Cast { expr, to } => {
                    let s = self.expr(expr, f);
                    let b = self.width_of(*to).map_or(Interval::Top, width_bounds);
                    (CheckCause::Convert, contained(s, b), s.meet(b))
                }
                _ => (CheckCause::Convert, false, self.expr(inner, f)),
            },
            CheckedKind::Shift => match &inner.kind {
                ExprKind::Binary { op, lhs, rhs } => {
                    let (a, b) = (self.expr(lhs, f), self.expr(rhs, f));
                    let bits = self.width_of(node.ty).map_or(64, |w| i128::from(w.bits()));
                    let value = if *op == BinaryOp::Shr { shr(a, b) } else { Interval::Top };
                    (CheckCause::Arith, contained(b, Interval::Int { lo: 0, hi: bits - 1 }), value)
                }
                _ => (CheckCause::Arith, false, self.expr(inner, f)),
            },
            // Das rohe Ergebnis zaehlt; `expr` klemmte es auf den Typ.
            CheckedKind::Overflow => {
                let i = match &inner.kind {
                    ExprKind::Binary { op, lhs, rhs } => {
                        let (a, b) = (self.expr(lhs, f), self.expr(rhs, f));
                        match op {
                            BinaryOp::Add => a + b,
                            BinaryOp::Sub => a - b,
                            BinaryOp::Mul => a * b,
                            BinaryOp::Div => a / b,
                            BinaryOp::Rem => a % b,
                            _ => Interval::Top,
                        }
                    }
                    _ => self.expr(inner, f),
                };
                let b = self.width_of(node.ty).map_or(Interval::Top, width_bounds);
                (CheckCause::Arith, contained(i, b), i.meet(b))
            }
            CheckedKind::DivZero => {
                let i = self.expr(inner, f);
                (CheckCause::Arith, !i.contains_zero(), i)
            }
            CheckedKind::NonFinite | CheckedKind::Domain => (CheckCause::Arith, false, self.expr(inner, f)),
        };
        if proven {
            // 3.4: „Ist das Intervall des Ausdrucks enthalten → keine
            // Pruefung."
            self.proven.push((node.span, tag(kind)));
        } else {
            // Ein Ueberlauf in 64 Bit warnt nicht (Pruefung 4).
            let wide = matches!(kind, CheckedKind::Overflow) && self.width_of(node.ty).is_none_or(|w| w.bits() == 64);
            let warns = !wide && (self.loop_depth > 0 || self.in_action);
            self.checks.push(ImplicitCheck { cause, span: node.span, warns, relational });
        }
        result
    }

    /// Die Breite eines Ganzzahl- oder Dauertyps.
    fn width_of(&self, ty: crate::TypeId) -> Option<IntWidth> {
        match self.program.types.list.get(ty.index()) {
            Some(Type::Int { width, .. }) => Some(*width),
            Some(Type::Duration { .. }) => Some(IntWidth::I64),
            _ => None,
        }
    }

    /// Was ein Typ ueber seine Werte sagt: die deklarierte Range, sonst die
    /// Schranken seiner Breite (3.2). Ein `u16` ohne Range ist `0..65535`,
    /// und das reicht der Analyse fuer den haeufigsten Fall — die Konversion
    /// eines schmalen Registerwerts in eine passende Range.
    fn bounds_of(&self, ty: crate::TypeId) -> Interval {
        match self.program.types.list.get(ty.index()) {
            Some(Type::Int { range: Some(r), .. } | Type::Duration { range: Some(r) }) => Interval::from_range(r),
            // `int` ist i64: die Schranke ist wahr, aber nutzlos eng zu
            // nennen — sie verhindert nur die Verengung auf i32.
            Some(Type::Int { width, .. }) if width.bits() < 64 => width_bounds(*width),
            _ => Interval::Top,
        }
    }
}

/// Die Art einer Pruefung als Schluessel; zwei Knoten an derselben Stelle
/// (Divisor und sein Ueberlauf) bleiben so unterscheidbar.
pub fn tag(kind: &CheckedKind) -> u8 {
    match kind {
        CheckedKind::DivZero => 0,
        CheckedKind::Overflow => 1,
        CheckedKind::NonFinite => 2,
        CheckedKind::Domain => 3,
        CheckedKind::Index { .. } => 4,
        CheckedKind::Range(_) => 5,
        CheckedKind::Convert => 6,
        CheckedKind::Shift => 7,
        CheckedKind::Valid => 8,
        CheckedKind::Missing => 9,
    }
}

/// Was `len` und `count` einer Sammlung hoechstens sind (3.9).
fn capacity_of(p: &Program, ty: crate::TypeId) -> Interval {
    match p.types.list.get(ty.index()) {
        Some(Type::Array { len, .. } | Type::Samples { len, .. }) => Interval::point(i128::from(*len)),
        Some(
            Type::Bytes { cap }
            | Type::Vec { cap, .. }
            | Type::Str { cap }
            | Type::Line { cap }
            | Type::Map { cap, .. },
        ) => Interval::Int { lo: 0, hi: i128::from(*cap) },
        _ => Interval::Top,
    }
}

/// Nach einer Schleife ist die Stelle wieder erreichbar: `break` beendet
/// die Schleife, nicht die Kette (T13).
fn f_reachable(f: &mut Facts) {
    if !f.is_reachable() {
        *f = Facts::entry();
    }
}

/// Die Grenze, die ein Vergleich einer Variablen auferlegt.
fn bound(op: BinaryOp, lo: i128, hi: i128) -> Interval {
    match op {
        BinaryOp::Lt => Interval::below(hi),
        BinaryOp::Le => Interval::at_most(hi),
        BinaryOp::Gt => Interval::above(lo),
        BinaryOp::Ge => Interval::at_least(lo),
        BinaryOp::Eq => Interval::Int { lo, hi },
        _ => Interval::Top,
    }
}

/// `a op b` wird zu `b flip(op) a`.
fn flip(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Gt,
        BinaryOp::Le => BinaryOp::Ge,
        BinaryOp::Gt => BinaryOp::Lt,
        BinaryOp::Ge => BinaryOp::Le,
        other => other,
    }
}

/// Die Bedingung des `else`-Zweigs.
fn negate(op: BinaryOp) -> BinaryOp {
    match op {
        BinaryOp::Lt => BinaryOp::Ge,
        BinaryOp::Le => BinaryOp::Gt,
        BinaryOp::Gt => BinaryOp::Le,
        BinaryOp::Ge => BinaryOp::Lt,
        other => other,
    }
}

/// `a >> b` fuer nichtnegatives `a`.
fn shr(a: Interval, b: Interval) -> Interval {
    match (a, b) {
        (Interval::Int { lo, hi }, Interval::Int { lo: bl, hi: bh }) if lo >= 0 && bl >= 0 && bh < 127 => {
            Interval::Int { lo: lo >> bh, hi: hi >> bl }
        }
        _ => Interval::Top,
    }
}

/// `a & b`: mit einer nichtnegativen Seite hoechstens deren Obergrenze.
fn bit_and(a: Interval, b: Interval) -> Interval {
    let cap = |i: Interval| match i {
        Interval::Int { lo, hi } if lo >= 0 => Some(hi),
        _ => None,
    };
    match (cap(a), cap(b)) {
        (Some(x), Some(y)) => Interval::Int { lo: 0, hi: x.min(y) },
        (Some(x), None) | (None, Some(x)) => Interval::Int { lo: 0, hi: x },
        _ => Interval::Top,
    }
}

fn min_of(a: Interval, b: Interval) -> Interval {
    match (a, b) {
        (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh }) => {
            Interval::Int { lo: al.min(bl), hi: ah.min(bh) }
        }
        _ => Interval::Top,
    }
}

fn max_of(a: Interval, b: Interval) -> Interval {
    match (a, b) {
        (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh }) => {
            Interval::Int { lo: al.max(bl), hi: ah.max(bh) }
        }
        _ => Interval::Top,
    }
}

/// Ist der Zugriff eine Gueltigkeitsfrage (3.8)?
fn is_validity(a: crate::expr::Accessor) -> bool {
    use crate::expr::Accessor;
    matches!(a, Accessor::Valid | Accessor::Ok)
}

/// Schluessel eines dominierten Wrappers: der Ausdruck in Textform reicht,
/// weil die Dominanz nur syntaktisch gilt (3.8).
fn dom_key(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Var(v) => Some(format!("v{}", v.0)),
        ExprKind::Input { channel, .. } => Some(format!("c{}", channel.0)),
        ExprKind::Field { base, field } => Some(format!("{}.{field}", dom_key(base)?)),
        _ => None,
    }
}

/// Variablen, die ein Block schreibt (fuer die Weitung).
fn written_vars(b: &Block) -> Vec<VarId> {
    let mut out = Vec::new();
    collect_written(b, &mut out);
    out.sort_by_key(|v| v.0);
    out.dedup();
    out
}

fn collect_written(b: &Block, out: &mut Vec<VarId>) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Assign { target: Place::Var(v), .. } => out.push(*v),
            StmtKind::MethodCall { target: Some(Place::Var(v)), .. } => out.push(*v),
            StmtKind::If { then, otherwise, .. } => {
                collect_written(then, out);
                collect_written(otherwise, out);
            }
            StmtKind::ForRange { body, .. }
            | StmtKind::ForEach { body, .. }
            | StmtKind::Every { body, .. }
            | StmtKind::At { body, .. } => collect_written(body, out),
            StmtKind::Match { arms, .. } => {
                for a in arms {
                    collect_written(&a.body, out);
                }
            }
            _ => {}
        }
    }
}

/// Teilausdruecke, die kein eigener Fall behandelt.
fn walk_children(k: &ExprKind, f: &mut impl FnMut(&Expr)) {
    match k {
        ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter().for_each(f),
        ExprKind::Array(items) => items.iter().for_each(f),
        ExprKind::Tuple(a, b) => {
            f(a);
            f(b);
        }
        ExprKind::BlockInit { args, .. }
        | ExprKind::Call { args, .. }
        | ExprKind::NativeCall { args, .. }
        | ExprKind::MatOp { args, .. } => args.iter().for_each(f),
        ExprKind::Slice { base, from, to } => {
            f(base);
            f(from);
            f(to);
        }
        ExprKind::Index2 { base, row, col } => {
            f(base);
            f(row);
            f(col);
        }
        ExprKind::Accessor { base, args, .. } => {
            f(base);
            args.iter().for_each(f);
        }
        ExprKind::Convert { expr, .. } | ExprKind::Lift(expr) | ExprKind::Ok(expr) | ExprKind::Err(expr) => f(expr),
        ExprKind::Matches { subject, .. } => f(subject),
        ExprKind::Decode { bytes, .. } => f(bytes),
        _ => {}
    }
}

/// Die Kennzahl aus plan/m6.md 2.12: Koennte eine Relation zweier Variablen
/// diese Pruefung beweisen? Ja, wenn der Ausdruck zwei Variablen oder
/// Inputs nennt oder seine Variable in einer dominierenden Relation steht.
fn relational(e: &Expr, f: &Facts) -> bool {
    let mut names = std::collections::BTreeSet::new();
    let mut stack = vec![e];
    while let Some(x) = stack.pop() {
        match &x.kind {
            ExprKind::Var(v) => {
                names.insert((0u8, v.0));
            }
            ExprKind::Input { channel, .. } => {
                names.insert((1u8, channel.0));
            }
            _ => {}
        }
        stack.extend(x.children());
    }
    names.len() >= 2 || names.iter().any(|(kind, v)| *kind == 0 && f.related(VarId(*v)))
}

/// Die Konstante eines Ausdrucks, wenn er eine ist.
pub fn const_of(e: &Expr) -> Option<Const> {
    match &e.kind {
        ExprKind::Int(v) => Some(Const::Int(*v)),
        ExprKind::Duration(v) => Some(Const::Duration(*v)),
        ExprKind::Bool(v) => Some(Const::Bool(*v)),
        ExprKind::Float(v) => Some(Const::Float(*v)),
        _ => None,
    }
}

/// Was eine Integer-Breite zulaesst (3.2).
fn width_bounds(w: crate::types::IntWidth) -> Interval {
    use crate::types::IntWidth::*;
    let (lo, hi): (i128, i128) = match w {
        I8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
        U8 => (0, i128::from(u8::MAX)),
        I16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
        U16 => (0, i128::from(u16::MAX)),
        I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
        U32 => (0, i128::from(u32::MAX)),
        I64 => (i128::from(i64::MIN), i128::from(i64::MAX)),
        U64 => (0, i128::from(u64::MAX)),
    };
    Interval::Int { lo, hi }
}
