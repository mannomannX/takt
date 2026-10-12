//! Grosse Sammlungen als Arrays der SMT-Theorie (FB-497, Referenz 13.3).
//!
//! Bytes, Vektoren und Arrays ab [`ARRAY_FROM`] Blaettern fuehrt das Modell
//! nicht Platz fuer Platz, sondern je Blatt eines Platzes als ein Array ueber
//! die Plaetze (`Shape::Seq`, `V::Seq`): Lesen an einem berechneten Index
//! ist ein `select`, Schreiben ein `store`, statt einer Auswahl ueber alle
//! Plaetze, und der Zustand haelt ein Array statt tausender Blaetter. Eine
//! grosse map, deren Slotfolge nichts beobachtet, ist ein Array nach ihrem
//! Schluessel (`map.rs`).
//!
//! Wer die Plaetze einzeln braucht (Text, Byteform), nimmt sie mit
//! [`V::flat`]; ein Ort im Zustand fasst sie beim Schreiben wieder zu
//! Arrays (`Enc::store`). Ein Platz einer Folge haelt hoechstens selbst eine
//! Folge: Das Array ist dann eines aus Arrays (`Sort::Array2`), etwa die
//! Chunks des Flash-Modells je Schluessel.

use std::collections::BTreeSet;

use takt_diag::Span;
use takt_mir::expr::{Accessor, BinaryOp, ExprKind};
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::stmt::{ForVars, Observe, Stmt, StmtKind};
use takt_mir::types::Type;
use takt_mir::{Program, TypeId, visit};

use super::value::{Shape, V};
use super::{Enc, R, no};
use crate::term::{Node, Term};

/// Ab so vielen Blaettern ist eine Sammlung eine Folge von Arrays. Darunter
/// bleibt jeder Platz ein Blatt: Kleine Sammlungen rechnet der Solver ohne
/// die Theorie der Arrays schneller. Ein Chunk des Flash-Modells (256 Byte)
/// liegt darueber: Jeder Zugriff an einem berechneten Index mischte sonst
/// alle Plaetze.
pub(super) const ARRAY_FROM: u64 = 256;

impl V {
    /// Derselbe Wert mit `f` an jedem Blatt.
    pub(super) fn map_leaves(self, f: &dyn Fn(Term) -> Term) -> V {
        match self {
            V::Leaf(t) => V::Leaf(f(t)),
            V::Node(parts) => V::Node(parts.into_iter().map(|p| p.map_leaves(f)).collect()),
            V::Seq(n, t) => V::Seq(n, Box::new(t.map_leaves(f))),
        }
    }

    /// Der Wert ohne Arrays: Eine Folge in einem Knoten steht fuer ihre `n`
    /// Plaetze. `None` fuer eine map nach Schluesseln — sie hat keine Folge
    /// von Plaetzen.
    pub(super) fn flat(self) -> Option<V> {
        match self {
            V::Leaf(t) => Some(V::Leaf(t)),
            V::Seq(..) => None,
            V::Node(parts) => {
                let mut out = Vec::with_capacity(parts.len());
                for p in parts {
                    match p {
                        V::Seq(Some(n), t) => {
                            for k in 0..n {
                                out.push(seq_item(&t, &Term::int(i64::from(k))).flat()?);
                            }
                        }
                        V::Seq(None, _) => return None,
                        other => out.push(other.flat()?),
                    }
                }
                Some(V::Node(out))
            }
        }
    }

    /// Wie [`V::flat`], sonst eine Luecke mit Grund.
    pub(super) fn flat_at(self, span: Span) -> R<V> {
        match self.flat() {
            Some(v) => Ok(v),
            None => no("die Slotfolge einer map, die das Modell nach ihren Schluesseln fuehrt", span),
        }
    }
}

impl Shape {
    /// Steckt in der Gestalt eine Folge?
    pub(super) fn has_seq(&self) -> bool {
        self.depth() > 0
    }

    /// Wie tief Folgen in der Gestalt ineinander stecken.
    fn depth(&self) -> u32 {
        match self {
            Shape::Seq(_, elem) => 1 + elem.depth(),
            Shape::Node(parts) => parts.iter().map(|(_, p)| p.depth()).max().unwrap_or(0),
            _ => 0,
        }
    }
}

/// Ein Index, der beim Bau feststeht.
fn fixed(i: &Term) -> Option<i64> {
    match &*i.0 {
        Node::Int(k) => Some(*k),
        _ => None,
    }
}

/// Der Platz `i` einer Folge: ihre Arrays an diesem Index.
pub(super) fn seq_item(t: &V, i: &Term) -> V {
    t.clone().map_leaves(&|a| Term::select(a, i.clone()))
}

/// Die Folge mit `x` am Platz `i`, wo `cond` gilt. Haelt ein Platz selbst
/// eine Folge, steht ihr Array am Index; stehen ihre Plaetze in `x` einzeln
/// da, werden sie vorher zu Arrays.
pub(super) fn seq_store(t: V, i: &Term, x: V, cond: &Term) -> V {
    match (t, x) {
        (V::Leaf(a), V::Leaf(v)) => V::Leaf(Term::ite(cond.clone(), Term::store(a.clone(), i.clone(), v), a)),
        (V::Seq(n, t), V::Seq(_, x)) => V::Seq(n, Box::new(seq_store(*t, i, *x, cond))),
        (V::Node(ts), V::Node(xs)) => {
            let xs = regroup_like(&ts, xs);
            V::Node(ts.into_iter().zip(xs).map(|(t, x)| seq_store(t, i, x, cond)).collect())
        }
        (t, _) => t,
    }
}

/// Die Teile `xs` in der Form der Teile `ts`: Steht in `ts` eine Folge und
/// in `xs` ihre Plaetze einzeln, werden diese zu ihren Arrays.
fn regroup_like(ts: &[V], mut xs: Vec<V>) -> Vec<V> {
    let Some((s, n)) = ts.iter().enumerate().find_map(|(k, t)| match t {
        V::Seq(Some(n), _) => Some((k, *n as usize)),
        _ => None,
    }) else {
        return xs;
    };
    if xs.len() != ts.len() - 1 + n || matches!(xs.get(s), Some(V::Seq(..))) {
        return xs;
    }
    let V::Seq(_, elem) = &ts[s] else { return xs };
    let tail = xs.split_off(s + n);
    let items = xs.split_off(s).into_iter().map(|x| conform_like(elem, x)).collect();
    xs.push(Enc::seq_of(items));
    xs.extend(tail);
    xs
}

/// `x` in der Form von `t`, Teil fuer Teil ([`regroup_like`]).
fn conform_like(t: &V, x: V) -> V {
    match (t, x) {
        (V::Node(ts), V::Node(xs)) => {
            V::Node(ts.iter().zip(regroup_like(ts, xs)).map(|(t, x)| conform_like(t, x)).collect())
        }
        (_, x) => x,
    }
}

/// Die Plaetze einer Sammlung: je Platz ein Wert oder eine Folge.
#[derive(Clone, Debug)]
pub(super) enum Places {
    List(Vec<V>),
    Arrays(u32, V),
}

impl Places {
    /// Die Plaetze in den Teilen eines Werts: eine Folge oder die Teile selbst.
    pub(super) fn of(parts: Vec<V>) -> Places {
        match <[V; 1]>::try_from(parts) {
            Ok([V::Seq(Some(n), t)]) => Places::Arrays(n, *t),
            Ok([other]) => Places::List(vec![other]),
            Err(parts) => Places::List(parts),
        }
    }

    /// Wie viele Plaetze.
    pub(super) fn count(&self) -> usize {
        match self {
            Places::List(items) => items.len(),
            Places::Arrays(n, _) => *n as usize,
        }
    }

    /// Der Platz `i`, ohne Pruefung: ausserhalb in einer Liste der letzte,
    /// in einer Folge, was dort im Array steht.
    pub(super) fn get(&self, i: &Term) -> V {
        match self {
            Places::List(items) => Enc::select(items.clone(), i),
            Places::Arrays(_, t) => seq_item(t, i),
        }
    }

    /// Der Platz `k`.
    pub(super) fn item(&self, k: usize) -> V {
        match self {
            Places::List(items) => items[k].clone(),
            Places::Arrays(_, t) => seq_item(t, &Term::int(k as i64)),
        }
    }

    /// Alle Plaetze einzeln.
    pub(super) fn items(self) -> Vec<V> {
        match self {
            Places::List(items) => items,
            Places::Arrays(n, t) => (0..n).map(|k| seq_item(&t, &Term::int(i64::from(k)))).collect(),
        }
    }

    /// Die Plaetze mit `f(alt)` am Platz `i`, wo `cond` gilt. Steht `i`
    /// fest, aendert sich in einer Liste nur sein Platz.
    pub(super) fn update(self, i: &Term, cond: &Term, f: &mut dyn FnMut(V) -> R<V>) -> R<Places> {
        Ok(match self {
            Places::List(items) => {
                let fixed = fixed(i);
                let mut out = Vec::with_capacity(items.len());
                for (k, item) in items.into_iter().enumerate() {
                    let k = k as i64;
                    if fixed.is_some_and(|f| f != k) {
                        out.push(item);
                        continue;
                    }
                    let here = Term::and(vec![cond.clone(), Term::eq(i.clone(), Term::int(k))]);
                    let changed = f(item.clone())?;
                    out.push(V::ite(&here, changed, item));
                }
                Places::List(out)
            }
            Places::Arrays(n, t) => {
                let changed = f(seq_item(&t, i))?;
                Places::Arrays(n, seq_store(t, i, changed, cond))
            }
        })
    }

    /// Die Plaetze mit `x` am Platz `i`, wo `cond` gilt.
    pub(super) fn set(self, i: &Term, x: V, cond: &Term) -> Places {
        match self {
            Places::Arrays(n, t) => Places::Arrays(n, seq_store(t, i, x, cond)),
            Places::List(items) => {
                let fixed = fixed(i);
                let set = |(k, item): (usize, V)| {
                    let k = k as i64;
                    if fixed.is_some_and(|f| f != k) {
                        return item;
                    }
                    let here = Term::and(vec![cond.clone(), Term::eq(i.clone(), Term::int(k))]);
                    V::ite(&here, x.clone(), item)
                };
                Places::List(items.into_iter().enumerate().map(set).collect())
            }
        }
    }

    /// Die Teile, mit denen die Plaetze im Wert ihrer Sammlung stehen.
    pub(super) fn into_parts(self) -> Vec<V> {
        match self {
            Places::List(items) => items,
            Places::Arrays(n, t) => vec![V::Seq(Some(n), Box::new(t))],
        }
    }
}

impl Enc<'_> {
    /// Die Plaetze einer Sammlung in ihrer Gestalt: einzeln `[k]`, oder ab
    /// [`ARRAY_FROM`] Blaettern als eine Folge `[*]`, wenn ein Platz selbst
    /// keine haelt.
    pub(super) fn place_shapes(&self, n: u32, elem: Shape) -> Vec<(String, Shape)> {
        if elem.depth() <= 1 && u64::from(n).saturating_mul(Enc::leaf_count(&elem)) >= ARRAY_FROM {
            return vec![("[*]".to_string(), Shape::Seq(Some(n), Box::new(elem)))];
        }
        (0..n).map(|i| (format!("[{i}]"), elem.clone())).collect()
    }

    /// Fuehrt das Modell die map `ty` nach ihren Schluesseln? Sie ist gross,
    /// ihr Schluessel ein Skalar, ein Eintrag haelt hoechstens eine Folge,
    /// und ihre Slotfolge beobachtet nichts im Programm
    /// ([`slot_order_observed`]).
    pub(super) fn keyed(&self, ty: TypeId, key: TypeId, cap: u32, slot: &Shape) -> bool {
        u64::from(cap).saturating_mul(Enc::leaf_count(slot)) >= ARRAY_FROM
            && slot.depth() <= 1
            && self.key_scalar(key)
            && !self.observed_maps.get_or_init(|| slot_order_observed(self.p)).contains(&ty)
    }

    /// Ein Schluessel, der sich ohne Verlust als Index schreibt.
    fn key_scalar(&self, key: TypeId) -> bool {
        match self.p.types.get(key) {
            Type::Int { .. } | Type::Bool | Type::Duration { .. } => true,
            Type::Enum(e) => !self.fielded(*e),
            _ => false,
        }
    }

    /// Der Index eines Schluessels in einer map nach Schluesseln: die Zahl
    /// selbst, ein Wahrheitswert als 0 oder 1.
    pub(super) fn key_index(&self, key: V, span: Span) -> R<Term> {
        let k = key.leaf(span)?;
        Ok(match k.sort() {
            crate::term::Sort::Bool => Term::ite(k, Term::int(1), Term::int(0)),
            _ => k,
        })
    }

    /// Die Teile eines Werts so, wie die Gestalt `parts` sie fuehrt: Stehen
    /// die Plaetze einer Folge einzeln da, werden sie zu ihren Arrays.
    pub(super) fn regroup(parts: &[(String, Shape)], mut values: Vec<V>) -> Vec<V> {
        let Some((s, n, elem)) = parts.iter().enumerate().find_map(|(i, (_, p))| match p {
            Shape::Seq(Some(n), elem) => Some((i, *n as usize, elem)),
            _ => None,
        }) else {
            return values;
        };
        if values.len() != parts.len() - 1 + n {
            return values;
        }
        let tail = values.split_off(s + n);
        let items = values.split_off(s).into_iter().map(|x| Enc::conform(elem, x)).collect();
        values.push(Enc::seq_of(items));
        values.extend(tail);
        values
    }

    /// Eine Folge aus ihren Plaetzen: jedes Array null, dann je Platz ein `store`.
    fn seq_of(items: Vec<V>) -> V {
        let n = items.len() as u32;
        let Some(first) = items.first() else { return V::Seq(Some(0), Box::new(V::Node(Vec::new()))) };
        let zero = first.clone().map_leaves(&|x| Term::const_array(Enc::zero(x.sort())));
        let t = items
            .into_iter()
            .enumerate()
            .fold(zero, |t, (k, x)| seq_store(t, &Term::int(k as i64), x, &Term::bool(true)));
        V::Seq(Some(n), Box::new(t))
    }
}

/// Die map-Typen, deren Slotfolge ein Lauf beobachten kann (3.9): `for (k, v)
/// in m`, `==`, Text und Formate, die Byteform (`persist`, `encode`, `send`,
/// Natives), Channels, `pub var` und Parameter im Trace. Grob nach Typ: Wer
/// irgendwo einen Wert eines Typs so liest, macht ihn fuer das ganze
/// Programm zur Slotform; eine map nach Schluesseln, die doch eine Folge
/// braucht, meldet das Modell als Luecke (`V::flat_at`).
pub(super) fn slot_order_observed(p: &Program) -> BTreeSet<TypeId> {
    let mut out = BTreeSet::new();
    for c in &p.channels {
        maps_in(p, c.ty, &mut out);
    }
    for prm in &p.params {
        maps_in(p, prm.ty, &mut out);
    }
    for m in &p.machines {
        for v in m.vars.iter().filter(|v| v.public) {
            maps_in(p, v.ty, &mut out);
        }
        for pv in &m.persist {
            maps_in(p, m.vars[pv.var.index()].ty, &mut out);
        }
        visit::for_each_stmt(m, &mut |s| stmt_marks(p, s, &mut out));
        visit::for_each_expr_machine(m, &mut |e| expr_marks(p, e, &mut out));
    }
    for f in &p.fns {
        visit::for_each_stmt_block(&f.body, &mut |s| stmt_marks(p, s, &mut out));
        visit::for_each_expr_block(&f.body, &mut |e| expr_marks(p, e, &mut out));
    }
    for prop in &p.properties {
        prop_marks(p, &prop.formula, &mut out);
    }
    out
}

fn prop_marks(p: &Program, f: &takt_mir::expr::TProp, out: &mut BTreeSet<TypeId>) {
    use takt_mir::expr::TProp;
    match f {
        TProp::Temporal { inner, .. } | TProp::Not(inner) => prop_marks(p, inner, out),
        TProp::Implies(a, b) | TProp::And(a, b) | TProp::Or(a, b) => {
            prop_marks(p, a, out);
            prop_marks(p, b, out);
        }
        TProp::Atom(e) => visit::walk_expr(e, &mut |e| expr_marks(p, e, out)),
    }
}

/// Was eine Anweisung von einem Wert als Folge liest: Iteration, `send`,
/// die Platzhalter ihrer Meldungen.
fn stmt_marks(p: &Program, s: &Stmt, out: &mut BTreeSet<TypeId>) {
    let format = |f: &Format, out: &mut BTreeSet<TypeId>| {
        for piece in &f.pieces {
            if let FormatPiece::Expr { expr, .. } = piece {
                maps_in(p, expr.ty, out);
            }
        }
    };
    match &s.kind {
        StmtKind::ForEach { vars: ForVars::Pair(..), iter, .. } => maps_in(p, iter.ty, out),
        StmtKind::Send { value, .. } => maps_in(p, value.ty, out),
        StmtKind::Check { message: Some(m), .. } | StmtKind::Abort { message: Some(m) } => format(m, out),
        StmtKind::Observe(Observe::Alert { message, .. } | Observe::Verify { message, .. })
        | StmtKind::Observe(Observe::Log(message))
        | StmtKind::Observe(Observe::Verdict { message: Some(message), .. }) => format(message, out),
        _ => {}
    }
}

/// Was ein Ausdruck von einem Wert als Folge liest: Gleichheit, Formate,
/// Byteformen, Natives, Musterabgleich.
fn expr_marks(p: &Program, e: &takt_mir::expr::Expr, out: &mut BTreeSet<TypeId>) {
    match &e.kind {
        ExprKind::Binary { op: BinaryOp::Eq | BinaryOp::Ne, lhs, rhs } => {
            maps_in(p, lhs.ty, out);
            maps_in(p, rhs.ty, out);
        }
        ExprKind::Format(f) => {
            for piece in &f.pieces {
                if let FormatPiece::Expr { expr, .. } = piece {
                    maps_in(p, expr.ty, out);
                }
            }
        }
        ExprKind::Accessor { base, accessor: Accessor::Encode, .. } => maps_in(p, base.ty, out),
        ExprKind::Decode { .. } => maps_in(p, e.ty, out),
        ExprKind::NativeCall { args, .. } => args.iter().for_each(|a| maps_in(p, a.ty, out)),
        ExprKind::Matches { subject, .. } => maps_in(p, subject.ty, out),
        _ => {}
    }
}

/// Jeder map-Typ in `ty`, auch in Feldern, Elementen und Varianten.
fn maps_in(p: &Program, ty: TypeId, out: &mut BTreeSet<TypeId>) {
    let mut stack = vec![ty];
    let mut seen = BTreeSet::new();
    while let Some(t) = stack.pop() {
        if !seen.insert(t) {
            continue;
        }
        match p.types.get(t) {
            Type::Map { key, value, .. } => {
                out.insert(t);
                stack.extend([*key, *value]);
            }
            Type::Record(r) => stack.extend(p.records[r.index()].fields.iter().map(|f| f.ty)),
            Type::Enum(e) => {
                stack.extend(p.enums[e.index()].variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)))
            }
            Type::Array { elem, .. }
            | Type::Samples { elem, .. }
            | Type::Vec { elem, .. }
            | Type::Optional(elem)
            | Type::Stream(elem)
            | Type::Capture { elem, .. } => stack.push(*elem),
            Type::Result { ok, err } => {
                stack.push(*ok);
                if let Some(e) = p.types.list.iter().position(|x| *x == Type::Enum(*err)) {
                    stack.push(TypeId(e as u32));
                }
            }
            _ => {}
        }
    }
}
