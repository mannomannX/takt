//! Zusammengesetzte Werte im Modell (M11 Schritt 27a). Ein Record, ein
//! Array, ein Optional und ein Enum mit Feldern sind ein Baum aus skalaren
//! Termen; ein Ort im Zustand haelt je Blatt eine Variable (`s.m.r.x`,
//! `s.m.a[3]`, `s.m.o.has`, `s.m.c.tag`). Bytes und Vektoren sind ihre
//! Laenge und ihre Plaetze bis zur Kapazitaet (`s.m.b.len`, `s.m.b[0]`).
//! Ein fehlender Wert, die Felder einer anderen Variante und die Plaetze
//! hinter der Laenge sind null wie im Standardwert des Interpreters
//! (`Value::default_for`); darum gilt Gleichheit Blatt fuer Blatt. Ein
//! Ergebnis `T!E` ist `.is_err`, der Wert und der Fehler — der
//! Standardwert ist `OK(…)`, das Flag also falsch.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Accessor, Builtin, CheckedKind, Expr, ExprKind};
use takt_mir::machine::FaultKind;
use takt_mir::stmt::{ArmPattern, Block, Method, Place};
use takt_mir::types::Type;
use takt_mir::{EnumId, MachineId, TypeId};

use super::{Cx, Enc, Env, Exit, ExitKind, Flow, R, UNROLL_LIMIT, ite_env, no};
use crate::term::{Op, Sort, Term};

/// Ein Wert im Modell: ein skalarer Term oder seine Teile in der Ordnung
/// seiner [`Shape`].
#[derive(Clone, Debug)]
pub(super) enum V {
    Leaf(Term),
    Node(Vec<V>),
}

impl V {
    /// Der skalare Term.
    pub(super) fn leaf(self, span: Span) -> R<Term> {
        match self {
            V::Leaf(t) => Ok(t),
            V::Node(_) => no("zusammengesetzter Wert an dieser Stelle", span),
        }
    }

    /// Der Teil `i`.
    pub(super) fn part(self, i: usize, span: Span) -> R<V> {
        match self {
            V::Node(mut parts) if i < parts.len() => Ok(parts.swap_remove(i)),
            _ => no("Teil eines Werts", span),
        }
    }

    /// Je Blatt `if c then a else b`; beide haben dieselbe Gestalt.
    pub(super) fn ite(c: &Term, a: V, b: V) -> V {
        match (a, b) {
            (V::Node(x), V::Node(y)) => V::Node(x.into_iter().zip(y).map(|(a, b)| V::ite(c, a, b)).collect()),
            (V::Leaf(x), V::Leaf(y)) => V::Leaf(Term::ite(c.clone(), x, y)),
            (a, _) => a,
        }
    }

    pub(super) fn leaves<'a>(&'a self, out: &mut Vec<&'a Term>) {
        match self {
            V::Leaf(t) => out.push(t),
            V::Node(parts) => parts.iter().for_each(|p| p.leaves(out)),
        }
    }
}

/// Die Gestalt eines Typs im Modell.
#[derive(Clone, Debug)]
pub(super) enum Shape {
    /// Ein skalarer Wert seines Typs.
    Leaf(TypeId),
    /// Ob ein Optional einen Wert hat.
    Flag,
    /// Die Variante eines Enums mit Feldern, als Index.
    Tag(EnumId),
    /// Die Laenge einer Sammlung bis zu ihrer Kapazitaet.
    Count(u32),
    /// Ein Byte.
    Byte,
    /// Teile mit ihrem Pfad: `.feld`, `[i]`.
    Node(Vec<(String, Shape)>),
}

/// Die obere Grenze eines Index einer Zuweisungsstelle, geprueft, wenn alle
/// Indizes ausgewertet sind; `depth` ist ihr Schritt von der Wurzel her.
#[derive(Clone, Debug)]
pub(super) struct Pending {
    kind: CheckedKind,
    span: Span,
    index: Term,
    depth: usize,
}

/// Wurzel einer Zuweisungsstelle, ihre Indizes von der Wurzel her (`None`
/// fuer ein Feld) und die noch offenen oberen Grenzen.
type PlacePath<'a> = (&'a Place, Vec<Option<Term>>, Vec<Pending>);

/// Ein Schritt auf dem Weg zu einem Teil. Ein Index zaehlt ab dem Teil
/// `offset`: null im Array, eins hinter der Laenge einer Sammlung.
#[derive(Clone, Debug)]
pub(super) enum Step {
    Field(usize),
    Index { index: Term, offset: usize },
}

/// Die Grenze eines Index: die Laenge des Arrays oder die der Sammlung, die
/// erst im Wert steht.
#[derive(Clone, Copy, Debug)]
enum Bound {
    Fixed(u32),
    Dynamic,
}

impl Enc<'_> {
    /// Ein Typ, dessen Wert im Modell mehr als ein Blatt hat?
    pub(super) fn composite(&self, ty: TypeId) -> bool {
        match self.p.types.get(ty) {
            Type::Record(_)
            | Type::Array { .. }
            | Type::Optional(_)
            | Type::Bytes { .. }
            | Type::Vec { .. }
            | Type::Str { .. }
            | Type::Line { .. }
            | Type::Result { .. } => true,
            Type::Enum(e) => self.fielded(*e),
            _ => false,
        }
    }

    /// Eine Sammlung mit Laenge: Bytes und Vektoren, mit ihrer Kapazitaet.
    fn collection(&self, ty: TypeId) -> Option<u32> {
        match self.p.types.get(ty) {
            Type::Bytes { cap } | Type::Vec { cap, .. } => Some(*cap),
            _ => None,
        }
    }

    /// Das Element eines Arrays oder einer Sammlung, mit der Grenze des Index.
    fn element_of(&self, ty: TypeId) -> Option<(Option<TypeId>, Bound)> {
        match self.p.types.get(ty) {
            Type::Array { elem, len } => Some((Some(*elem), Bound::Fixed(*len))),
            Type::Vec { elem, .. } => Some((Some(*elem), Bound::Dynamic)),
            Type::Bytes { .. } => Some((None, Bound::Dynamic)),
            _ => None,
        }
    }

    /// Der Typ eines Enums.
    fn enum_type(&self, e: EnumId) -> Option<TypeId> {
        self.p.types.list.iter().position(|t| *t == Type::Enum(e)).map(|i| TypeId(i as u32))
    }

    pub(super) fn fielded(&self, e: EnumId) -> bool {
        self.p.enums[e.index()].variants.iter().any(|v| !v.fields.is_empty())
    }

    /// Die Gestalt eines Typs.
    pub(super) fn shape(&self, ty: TypeId, span: Span) -> R<Shape> {
        Ok(match self.p.types.get(ty) {
            Type::Record(r) => {
                let fields = self.p.records[r.index()].fields.clone();
                let mut parts = Vec::new();
                for f in &fields {
                    parts.push((format!(".{}", f.name), self.shape(f.ty, span)?));
                }
                Shape::Node(parts)
            }
            Type::Array { elem, len } => {
                let s = self.shape(*elem, span)?;
                Shape::Node((0..*len).map(|i| (format!("[{i}]"), s.clone())).collect())
            }
            Type::Bytes { cap } => Shape::Node(
                std::iter::once((".len".to_string(), Shape::Count(*cap)))
                    .chain((0..*cap).map(|i| (format!("[{i}]"), Shape::Byte)))
                    .collect(),
            ),
            Type::Vec { elem, cap } => {
                let s = self.shape(*elem, span)?;
                Shape::Node(
                    std::iter::once((".len".to_string(), Shape::Count(*cap)))
                        .chain((0..*cap).map(|i| (format!("[{i}]"), s.clone())))
                        .collect(),
                )
            }
            Type::Optional(t) => {
                Shape::Node(vec![(".has".into(), Shape::Flag), (".value".into(), self.shape(*t, span)?)])
            }
            Type::Result { ok, err } => {
                let Some(e) = self.enum_type(*err) else { return no("Fehlertyp eines Ergebnisses", span) };
                Shape::Node(vec![
                    (".is_err".into(), Shape::Flag),
                    (".value".into(), self.shape(*ok, span)?),
                    (".error".into(), self.shape(e, span)?),
                ])
            }
            Type::Str { cap } => super::text::text_shape(*cap, false),
            Type::Line { cap } => super::text::text_shape(*cap, true),
            Type::Enum(e) if self.fielded(*e) => {
                let def = self.p.enums[e.index()].clone();
                let mut parts = vec![(".tag".to_string(), Shape::Tag(*e))];
                for v in &def.variants {
                    for f in &v.fields {
                        parts.push((format!(".{}.{}", v.name, f.name), self.shape(f.ty, span)?));
                    }
                }
                Shape::Node(parts)
            }
            _ => {
                self.sort_of(ty, span)?;
                Shape::Leaf(ty)
            }
        })
    }

    /// Die Teile der Felder jeder Variante in der Gestalt ihres Enums;
    /// Teil 0 ist die Variante.
    fn variant_parts(&self, e: EnumId) -> Vec<Vec<usize>> {
        let mut next = 1;
        self.p.enums[e.index()]
            .variants
            .iter()
            .map(|v| {
                let parts = (next..next + v.fields.len()).collect();
                next += v.fields.len();
                parts
            })
            .collect()
    }

    pub(super) fn leaf_sort(&self, s: &Shape, span: Span) -> R<Sort> {
        match s {
            Shape::Leaf(ty) => self.sort_of(*ty, span),
            Shape::Flag => Ok(Sort::Bool),
            Shape::Tag(_) | Shape::Count(_) | Shape::Byte => Ok(Sort::Int),
            Shape::Node(_) => no("zusammengesetzter Wert", span),
        }
    }

    /// Der Standardwert eines Typs.
    pub(super) fn zero_of(&self, ty: TypeId, span: Span) -> R<V> {
        self.zero_value(&self.shape(ty, span)?, span)
    }

    /// Der Standardwert (`Value::default_for`): null in jedem Blatt.
    pub(super) fn zero_value(&self, s: &Shape, span: Span) -> R<V> {
        Ok(match s {
            Shape::Node(parts) => V::Node(parts.iter().map(|(_, p)| self.zero_value(p, span)).collect::<R<Vec<_>>>()?),
            leaf => V::Leaf(Enc::zero(self.leaf_sort(leaf, span)?)),
        })
    }

    /// Ein Wert aus seinen Blaettern; `get` liefert den Term eines Orts.
    pub(super) fn gather(
        &mut self,
        base: &str,
        s: &Shape,
        get: &mut dyn FnMut(&mut Self, &str, &Shape) -> R<Term>,
    ) -> R<V> {
        Ok(match s {
            Shape::Node(parts) => {
                let mut out = Vec::new();
                for (path, p) in parts {
                    out.push(self.gather(&format!("{base}{path}"), p, get)?);
                }
                V::Node(out)
            }
            leaf => V::Leaf(get(self, base, leaf)?),
        })
    }

    /// Der Wert eines Orts im Zustand.
    pub(super) fn load(&mut self, env: &Env, base: &str, s: &Shape, span: Span) -> R<V> {
        self.gather(base, s, &mut |_, loc, _| {
            env.get(loc).cloned().ok_or_else(|| super::Unsupported { what: format!("Ort `{loc}`"), span })
        })
    }

    /// Schreibt einen Wert an seinen Ort, Blatt fuer Blatt.
    pub(super) fn store(env: &mut Env, base: &str, s: &Shape, v: V) {
        match (s, v) {
            (Shape::Node(parts), V::Node(values)) => {
                for ((path, p), v) in parts.iter().zip(values) {
                    Enc::store(env, &format!("{base}{path}"), p, v);
                }
            }
            (_, V::Leaf(t)) => {
                env.insert(base.to_string(), t);
            }
            (_, V::Node(_)) => {}
        }
    }

    /// Setzt den Ort `base` des Typs `ty` auf `v`.
    pub(super) fn init_loc(&self, env: &mut Env, base: &str, ty: TypeId, v: V, span: Span) -> R<()> {
        Enc::store(env, base, &self.shape(ty, span)?, v);
        Ok(())
    }

    /// Schreibt `v` an den Ort `base` des Typs `ty`, wo `alive` gilt.
    pub(super) fn put(&mut self, env: &mut Env, base: &str, ty: TypeId, v: V, alive: &Term, span: Span) -> R<()> {
        let shape = self.shape(ty, span)?;
        let old = self.load(env, base, &shape, span)?;
        Enc::store(env, base, &shape, V::ite(alive, v, old));
        Ok(())
    }

    /// Die Invarianten der Blaetter eines Orts (3.4); ein Ort ohne Gestalt
    /// im Modell hat keine.
    pub(super) fn typed_loc(&self, env: &Env, base: &str, ty: TypeId, out: &mut Vec<Term>) {
        let Ok(shape) = self.shape(ty, Span::default()) else { return };
        let mut locs = Vec::new();
        Enc::leaf_locs(base, &shape, &mut locs);
        for (loc, s) in locs {
            if let Some(t) = env.get(&loc).cloned().and_then(|x| self.leaf_invariant(x, &s)) {
                out.push(t);
            }
        }
    }

    /// Die Orte der Blaetter mit ihrer Gestalt.
    pub(super) fn leaf_locs(base: &str, s: &Shape, out: &mut Vec<(String, Shape)>) {
        match s {
            Shape::Node(parts) => {
                for (path, p) in parts {
                    Enc::leaf_locs(&format!("{base}{path}"), p, out);
                }
            }
            leaf => out.push((base.to_string(), leaf.clone())),
        }
    }

    /// Die Invariante eines Blatts (3.4): sein Typ, eine Variante in ihrem Enum.
    pub(super) fn leaf_invariant(&self, x: Term, s: &Shape) -> Option<Term> {
        match s {
            Shape::Leaf(ty) => self.type_invariant(x, *ty),
            Shape::Tag(e) => {
                let n = self.p.enums[e.index()].variants.len() as i64;
                Some(Term::and(vec![Term::bin(Op::Ge, x.clone(), Term::int(0)), Term::bin(Op::Lt, x, Term::int(n))]))
            }
            Shape::Count(cap) => Some(Term::and(vec![
                Term::bin(Op::Ge, x.clone(), Term::int(0)),
                Term::bin(Op::Le, x, Term::int(i64::from(*cap))),
            ])),
            Shape::Byte => {
                Some(Term::and(vec![Term::bin(Op::Ge, x.clone(), Term::int(0)), Term::bin(Op::Le, x, Term::int(255))]))
            }
            Shape::Flag | Shape::Node(_) => None,
        }
    }

    /// Sind zwei Werte gleich? Blatt fuer Blatt, Fliesskomma nach IEEE.
    pub(super) fn equal(a: &V, b: &V) -> Term {
        let (mut x, mut y) = (Vec::new(), Vec::new());
        a.leaves(&mut x);
        b.leaves(&mut y);
        Term::and(
            x.into_iter()
                .zip(y)
                .map(|(a, b)| match a.sort() {
                    Sort::F32 | Sort::F64 => Term::bin(Op::FEq, a.clone(), b.clone()),
                    _ => Term::eq(a.clone(), b.clone()),
                })
                .collect(),
        )
    }

    /// Die Plaetze und, bei einer Sammlung, ihre Laenge.
    pub(super) fn places(&self, ty: TypeId, v: V, span: Span) -> R<(Vec<V>, Option<Term>)> {
        let V::Node(mut parts) = v else { return no("Index", span) };
        if self.collection(ty).is_none() {
            return Ok((parts, None));
        }
        let rest = parts.split_off(1);
        let len = parts.pop().expect("Laenge").leaf(span)?;
        Ok((rest, Some(len)))
    }

    /// Das Element `index` (ohne Pruefung: ausserhalb ist es das letzte).
    fn select(elems: Vec<V>, index: &Term) -> V {
        let mut it = elems.into_iter().enumerate().rev();
        let Some((_, mut acc)) = it.next() else { return V::Node(Vec::new()) };
        for (k, e) in it {
            acc = V::ite(&Term::eq(index.clone(), Term::int(k as i64)), e, acc);
        }
        acc
    }

    /// Der Teil an der Stelle `steps`.
    fn at(v: V, steps: &[Step], span: Span) -> R<V> {
        let Some((step, rest)) = steps.split_first() else { return Ok(v) };
        let part = match (step, v) {
            (Step::Field(i), v) => v.part(*i, span)?,
            (Step::Index { index, offset }, V::Node(parts)) => {
                Enc::select(parts.into_iter().skip(*offset).collect(), index)
            }
            _ => return no("Teil eines Werts", span),
        };
        Enc::at(part, rest, span)
    }

    /// Der Wert mit `new` an der Stelle `steps` (der Wurzel naechster Schritt zuerst).
    fn update(v: V, steps: &[Step], new: V, span: Span) -> R<V> {
        let Some((step, rest)) = steps.split_first() else { return Ok(new) };
        let V::Node(mut parts) = v else { return no("Teil eines Werts", span) };
        match step {
            Step::Field(i) => {
                let Some(old) = parts.get(*i).cloned() else { return no("Feld", span) };
                parts[*i] = Enc::update(old, rest, new, span)?;
            }
            Step::Index { index, offset } => {
                for (k, part) in parts.iter_mut().enumerate().skip(*offset) {
                    let changed = Enc::update(part.clone(), rest, new.clone(), span)?;
                    let here = Term::eq(index.clone(), Term::int((k - offset) as i64));
                    *part = V::ite(&here, changed, part.clone());
                }
            }
        }
        Ok(V::Node(parts))
    }

    /// Ein Fault-Zweig an einer Pruefstelle, wie in `checked`.
    fn fault(&mut self, kind: &CheckedKind, span: Span, fail: Term, cx: &Cx<'_>, flow: &mut Flow) {
        let cond = Term::and(vec![flow.alive.clone(), fail.clone()]);
        self.site(kind, span, cond.clone(), cx);
        flow.exits.push(Exit { cond, kind: ExitKind::Fault(None, self.cause(kind.fault(), span)) });
        flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
    }

    /// Der Wert eines Ausdrucks beliebiger Gestalt; skalare wertet `expr` aus.
    #[deny(clippy::wildcard_enum_match_arm)]
    pub(super) fn value(&mut self, e: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<V> {
        if !self.composite(e.ty) {
            return Ok(V::Leaf(self.expr(e, cx, env, flow)?));
        }
        let span = e.span;
        let shape = self.shape(e.ty, span)?;
        Ok(match &e.kind {
            ExprKind::Default | ExprKind::None => self.zero_value(&shape, span)?,
            ExprKind::Str(s) => self.text_value(&super::text::Text::literal(s), e.ty, span)?,
            ExprKind::Format(f) => self.format(f, e.ty, cx, env, flow, span)?,
            ExprKind::Var(v) => {
                if let Some(local) = cx.locals.as_ref().and_then(|l| l.get(v)) {
                    return Ok(local.clone());
                }
                let Some(m) = cx.m else { return no("Variable ausserhalb einer Maschine", span) };
                let at = self.loc_var(m, *v);
                let stored = self.load(env, &at, &shape, span)?;
                self.bound_value(&at, stored)
            }
            ExprKind::Output(c) => {
                let ch = &self.p.channels[c.index()];
                if ch.owner.is_some_and(|o| !self.order.contains(&o)) {
                    return no("zusammengesetzter Output einer anderen Maschine", span);
                }
                let own = cx.m.is_some() && ch.owner == cx.m;
                let src = if own { env } else { cx.pre };
                self.load(src, &self.loc_out(*c), &shape, span)?
            }
            ExprKind::Published { machine, var } => {
                if machine.index.is_some() || !self.order.contains(&machine.machine) {
                    return no("zusammengesetzter Wert einer anderen Maschine", span);
                }
                let target: MachineId = machine.machine;
                let base = self.loc_var(target, *var);
                self.gather(&base, &shape, &mut |enc, loc, _| enc.psi(cx, env, target, loc, span))?
            }
            ExprKind::Record { fields, .. } => {
                let mut parts = Vec::new();
                for f in fields {
                    parts.push(self.value(f, cx, env, flow)?);
                }
                V::Node(parts)
            }
            ExprKind::Array(items) => {
                let mut parts = Vec::new();
                for x in items {
                    parts.push(self.value(x, cx, env, flow)?);
                }
                if self.collection(e.ty).is_none() {
                    return Ok(V::Node(parts));
                }
                // Eine Sammlung: die Laenge, die Elemente, dahinter null.
                let V::Node(zero) = self.zero_value(&shape, span)? else { return no("Sammlung", span) };
                let len = V::Leaf(Term::int(parts.len() as i64));
                V::Node(std::iter::once(len).chain(parts).chain(zero.into_iter().skip(1 + items.len())).collect())
            }
            ExprKind::Variant { variant, fields, .. } => {
                let mut values = Vec::new();
                for f in fields {
                    values.push(self.value(f, cx, env, flow)?);
                }
                self.variant(e.ty, *variant, values, span)?
            }
            ExprKind::Lift(x) => V::Node(vec![V::Leaf(Term::bool(true)), self.value(x, cx, env, flow)?]),
            ExprKind::Builtin(Builtin::LastFault) => {
                let Some(m) = cx.m else { return no("`last_fault` ausserhalb einer Maschine", span) };
                let at = self.loc_last_fault(m);
                self.load(env, &at, &shape, span)?
            }
            // `OK(x)` und `ERR(e)`: der andere Teil null.
            ExprKind::Ok(x) | ExprKind::Err(x) => {
                let V::Node(mut parts) = self.zero_value(&shape, span)? else { return no("Ergebnis", span) };
                let err = matches!(e.kind, ExprKind::Err(_));
                parts[0] = V::Leaf(Term::bool(err));
                parts[if err { 2 } else { 1 }] = self.value(x, cx, env, flow)?;
                V::Node(parts)
            }
            // `r.err`: der Fehler als Optional.
            ExprKind::Accessor { base, accessor: Accessor::Err, .. }
                if matches!(self.p.types.get(base.ty), Type::Result { .. }) =>
            {
                let r = self.value(base, cx, env, flow)?;
                V::Node(vec![r.clone().part(0, span)?, r.part(2, span)?])
            }
            ExprKind::Field { base, field } => self.field(base, *field as usize, cx, env, flow)?,
            ExprKind::Index { base, index } => self.element(base, index, cx, env, flow, span)?,
            ExprKind::Checked { expr: inner, kind } => self.checked_value(kind, inner, e, cx, env, flow)?,
            ExprKind::Cond { cond, then, otherwise } => {
                let c = self.expr(cond, cx, env, flow)?;
                let a = self.guarded(&c, flow, |enc, flow| enc.value(then, cx, env, flow))?;
                let b = self.guarded(&c.clone().not(), flow, |enc, flow| enc.value(otherwise, cx, env, flow))?;
                V::ite(&c, a, b)
            }
            ExprKind::Accessor { base, accessor: Accessor::Peek, .. }
                if matches!(self.p.types.get(base.ty), Type::Stream(_)) =>
            {
                self.peek(base, cx, flow, span)?
            }
            ExprKind::Accessor { base, accessor: Accessor::Sent, .. } => match self.tx_channel(base) {
                Some(c) => self.tx_accessor(c, Accessor::Sent, e.ty, cx, env, span)?,
                None => return no("`.sent` ohne Ausgabestrom", span),
            },
            ExprKind::Accessor { base, accessor: Accessor::Or, args } => {
                let [default] = args.as_slice() else { return no("`.or` ohne Ersatz", span) };
                self.or_value(base, default, cx, env, flow)?
            }
            ExprKind::Accessor { base, accessor: Accessor::Encode, .. } => {
                let Type::Record(r) = self.p.types.get(base.ty) else { return no("`encode` ohne Record", span) };
                let r = *r;
                let v = self.value(base, cx, env, flow)?;
                self.wire_encode(r, v, span)?
            }
            // `decode` faultet nie (3.7); ohne Wert ist der Record null.
            ExprKind::Decode { record, bytes } => {
                let b = self.value(bytes, cx, env, flow)?;
                let (ok, r) = self.wire_decode(*record, b, span)?;
                let zero = self.zero_value(&shape, span)?.part(1, span)?;
                V::Node(vec![V::Leaf(ok.clone()), V::ite(&ok, r, zero)])
            }
            ExprKind::Slice { base, from, to } => self.slice(base, from, to, cx, env, flow, span)?,
            ExprKind::Call { callee, args } => {
                let mut xs = Vec::new();
                for a in args {
                    xs.push(self.value(a, cx, env, flow)?);
                }
                self.call(*callee, xs, cx, env, flow, span)?
            }
            other @ (ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Duration(_)
            | ExprKind::Tuple(..)
            | ExprKind::BlockInit { .. }
            | ExprKind::Param(_)
            | ExprKind::Command(_)
            | ExprKind::Input { .. }
            | ExprKind::StateOf(_)
            | ExprKind::Signal { .. }
            | ExprKind::Builtin(_)
            | ExprKind::Armed(_)
            | ExprKind::PortRead(_)
            | ExprKind::Index2 { .. }
            | ExprKind::Accessor { .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Cast { .. }
            | ExprKind::Convert { .. }
            | ExprKind::JobState { .. }
            | ExprKind::Stream(_)
            | ExprKind::Matches { .. }
            | ExprKind::NativeCall { .. }
            | ExprKind::MatOp { .. }
            | ExprKind::Intrinsic { .. }) => {
                return no(format!("zusammengesetzter Ausdruck {}", super::node_name(other)), span);
            }
        })
    }

    /// Eine Variante eines Enums mit Feldern: die Variante in Teil 0, ihre
    /// Felder an ihren Stellen, alle anderen null.
    pub(super) fn variant(&self, ty: TypeId, variant: u32, fields: Vec<V>, span: Span) -> R<V> {
        let Type::Enum(e) = self.p.types.get(ty) else { return no("Variante", span) };
        let e = *e;
        let V::Node(mut parts) = self.zero_value(&self.shape(ty, span)?, span)? else { return no("Variante", span) };
        parts[0] = V::Leaf(Term::int(i64::from(variant)));
        let slots = self.variant_parts(e);
        let Some(slots) = slots.get(variant as usize) else { return no("Variante", span) };
        for (slot, v) in slots.iter().zip(fields) {
            parts[*slot] = v;
        }
        Ok(V::Node(parts))
    }

    /// Ein Feld eines Records.
    pub(super) fn field(&mut self, base: &Expr, field: usize, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<V> {
        if !matches!(self.p.types.get(base.ty), Type::Record(_)) {
            return no("Feld einer Variante", base.span);
        }
        self.value(base, cx, env, flow)?.part(field, base.span)
    }

    /// Ein Element ohne Pruefung: Die Analyse hat den Index bewiesen (3.4).
    pub(super) fn element(
        &mut self,
        base: &Expr,
        index: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        if !matches!(self.p.types.get(base.ty), Type::Array { .. } | Type::Bytes { .. } | Type::Vec { .. }) {
            return no("Index auf diesem Wert", span);
        }
        let v = self.value(base, cx, env, flow)?;
        let (elems, _) = self.places(base.ty, v, span)?;
        let i = self.expr(index, cx, env, flow)?;
        Ok(Enc::select(elems, &i))
    }

    /// `b[from..to]` auf Bytes oder einem Vektor (`eval.rs`): ausserhalb
    /// `0 <= from <= to <= len` ein `RangeFault`; der Ausschnitt beginnt bei
    /// null, dahinter null.
    #[allow(clippy::too_many_arguments)]
    fn slice(
        &mut self,
        base: &Expr,
        from: &Expr,
        to: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        let Some(elem) = (match self.p.types.get(base.ty) {
            Type::Bytes { .. } => Some(None),
            Type::Vec { elem, .. } => Some(Some(*elem)),
            _ => None,
        }) else {
            return no("Ausschnitt eines Arrays", span);
        };
        let v = self.value(base, cx, env, flow)?;
        let (elems, len) = self.places(base.ty, v, span)?;
        let len = len.expect("Sammlung");
        let a = self.expr(from, cx, env, flow)?;
        let b = self.expr(to, cx, env, flow)?;
        let fail = Term::or(vec![
            Term::bin(Op::Lt, a.clone(), Term::int(0)),
            Term::bin(Op::Lt, b.clone(), a.clone()),
            Term::bin(Op::Gt, b.clone(), len),
        ]);
        flow.exits.push(Exit {
            cond: Term::and(vec![flow.alive.clone(), fail.clone()]),
            kind: ExitKind::Fault(None, self.cause(FaultKind::Range, span)),
        });
        flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        let n = Term::bin(Op::Sub, b, a.clone());
        let zero = match elem {
            Some(t) => self.zero_of(t, span)?,
            None => V::Leaf(Term::int(0)),
        };
        let mut parts = vec![V::Leaf(n.clone())];
        for j in 0..elems.len() as i64 {
            let item = Enc::select(elems.clone(), &Term::bin(Op::Add, a.clone(), Term::int(j)));
            parts.push(V::ite(&Term::bin(Op::Lt, Term::int(j), n.clone()), item, zero.clone()));
        }
        Ok(V::Node(parts))
    }

    /// Ein skalarer Zugriff auf einen zusammengesetzten Wert: ob ein
    /// Optional einen Wert hat, `.or` mit skalarem Ergebnis, Laenge und Zahl
    /// eines Arrays.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn accessor(
        &mut self,
        base: &Expr,
        accessor: Accessor,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<Term> {
        match (accessor, self.p.types.get(base.ty)) {
            (Accessor::Valid, Type::Optional(_)) => self.value(base, cx, env, flow)?.part(0, span)?.leaf(span),
            (Accessor::Ok, Type::Result { .. }) => {
                Ok(self.value(base, cx, env, flow)?.part(0, span)?.leaf(span)?.not())
            }
            (Accessor::Or, Type::Optional(_) | Type::Result { .. }) => {
                let [default] = args else { return no("`.or` ohne Ersatz", span) };
                self.or_value(base, default, cx, env, flow)?.leaf(span)
            }
            (Accessor::Len | Accessor::Count, Type::Array { len, .. }) => {
                let len = i64::from(*len);
                self.value(base, cx, env, flow)?;
                Ok(Term::int(len))
            }
            (Accessor::Len, Type::Bytes { .. } | Type::Vec { .. } | Type::Str { .. } | Type::Line { .. }) => {
                self.value(base, cx, env, flow)?.part(0, span)?.leaf(span)
            }
            (Accessor::Truncated, Type::Line { cap }) => {
                let cap = *cap as usize;
                self.value(base, cx, env, flow)?.part(cap + 1, span)?.leaf(span)
            }
            (Accessor::StartsWith | Accessor::Contains, Type::Str { .. } | Type::Line { .. }) => {
                let [arg] = args else { return no("Zugriff ohne Argument", span) };
                let s = super::text::Text::of(self.value(base, cx, env, flow)?, span)?;
                let t = super::text::Text::of(self.value(arg, cx, env, flow)?, span)?;
                Ok(self.text_test(&s, &t, accessor == Accessor::Contains))
            }
            // Bits einer Ganzzahl (3.7): Eine Stelle ausserhalb der Breite
            // faultet im Interpreter selbst, ohne Pruefknoten.
            (Accessor::Bit | Accessor::Bits | Accessor::WithBit, Type::Int { width, .. }) => {
                let bits = i64::from(width.bits());
                let x = self.expr(base, cx, env, flow)?;
                let mut terms = Vec::new();
                for a in args {
                    terms.push(self.expr(a, cx, env, flow)?);
                }
                let outside = |i: &Term| {
                    Term::or(vec![
                        Term::bin(Op::Lt, i.clone(), Term::int(0)),
                        Term::bin(Op::Ge, i.clone(), Term::int(bits)),
                    ])
                };
                let fail = match (accessor, terms.as_slice()) {
                    (Accessor::Bits, [hi, lo]) => Term::or(vec![
                        Term::bin(Op::Lt, lo.clone(), Term::int(0)),
                        Term::bin(Op::Lt, hi.clone(), lo.clone()),
                        Term::bin(Op::Ge, hi.clone(), Term::int(bits)),
                    ]),
                    (_, [i, ..]) => outside(i),
                    _ => return no("Bitzugriff ohne Stelle", span),
                };
                flow.exits.push(Exit {
                    cond: Term::and(vec![flow.alive.clone(), fail.clone()]),
                    kind: ExitKind::Fault(None, self.cause(FaultKind::Range, span)),
                });
                flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
                let one = Term::int(1);
                Ok(match (accessor, terms.as_slice()) {
                    (Accessor::Bit, [i]) => {
                        let bit = Term::bin(Op::BitAnd, Term::bin(Op::Shr, x, i.clone()), one.clone());
                        Term::eq(bit, one)
                    }
                    (Accessor::Bits, [hi, lo]) => {
                        let n = Term::bin(Op::Add, Term::bin(Op::Sub, hi.clone(), lo.clone()), one.clone());
                        let mask = Term::bin(Op::Sub, Term::bin(Op::Shl, one, n), Term::int(1));
                        Term::bin(Op::BitAnd, Term::bin(Op::Shr, x, lo.clone()), mask)
                    }
                    (_, [i, b]) => {
                        let m = Term::bin(Op::Shl, one, i.clone());
                        let set = Term::bin(Op::BitOr, x.clone(), m.clone());
                        let clear = Term::bin(Op::BitAnd, x, Term::bin(Op::BitXor, m, Term::int(-1)));
                        let y = Term::ite(b.clone(), set, clear);
                        Term::app(Op::Wrap { bits: width.bits(), signed: width.signed() }, vec![y])
                    }
                    _ => return no("Bitzugriff", span),
                })
            }
            // `x.wrap_u8()` und Geschwister: modulo 2^n (3.10).
            (Accessor::Wrap(w), Type::Int { .. }) => {
                if w.bits() == 64 && !w.signed() {
                    return no(super::U64, span);
                }
                let x = self.expr(base, cx, env, flow)?;
                Ok(Term::app(Op::Wrap { bits: w.bits(), signed: w.signed() }, vec![x]))
            }
            _ => no(format!("Zugriff `.{}`", accessor.name()), span),
        }
    }

    /// Die Bedingung eines Zweigs von `case` und die Werte, die er bindet
    /// (`exec.rs`, `StmtKind::Match`): eine Variante, beim Optional `Some`
    /// als 0 und `None` als 1, Werte und Bereiche.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn arm(
        &mut self,
        pattern: &ArmPattern,
        subject: &V,
        ty: TypeId,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<(Term, Vec<V>)> {
        Ok(match pattern {
            ArmPattern::Wild => (Term::bool(true), Vec::new()),
            ArmPattern::Variant { variant, .. } => match self.p.types.get(ty) {
                // `case OK(v)` ist Variante 0, `case ERR(e)` Variante 1.
                Type::Result { .. } => {
                    let err = subject.clone().part(0, span)?.leaf(span)?;
                    if *variant == 0 {
                        (err.not(), vec![subject.clone().part(1, span)?])
                    } else {
                        (err, vec![subject.clone().part(2, span)?])
                    }
                }
                Type::Optional(_) => {
                    let has = subject.clone().part(0, span)?.leaf(span)?;
                    if *variant == 0 { (has, vec![subject.clone().part(1, span)?]) } else { (has.not(), Vec::new()) }
                }
                Type::Enum(e) if self.fielded(*e) => {
                    let tag = subject.clone().part(0, span)?.leaf(span)?;
                    let slots = self.variant_parts(*e);
                    let Some(slots) = slots.get(*variant as usize) else { return no("Variante", span) };
                    let payload = slots.iter().map(|s| subject.clone().part(*s, span)).collect::<R<Vec<_>>>()?;
                    (Term::eq(tag, Term::int(i64::from(*variant))), payload)
                }
                _ => (Term::eq(subject.clone().leaf(span)?, Term::int(i64::from(*variant))), Vec::new()),
            },
            ArmPattern::Values(values) => {
                let x = subject.clone().leaf(span)?;
                let float = matches!(x.sort(), Sort::F32 | Sort::F64);
                let (ge, le) = if float { (Op::FGe, Op::FLe) } else { (Op::Ge, Op::Le) };
                let mut hits = Vec::new();
                for cv in values {
                    let lo = self.expr(&cv.lo, cx, env, flow)?;
                    hits.push(match &cv.hi {
                        None if float => Term::bin(Op::FEq, x.clone(), lo),
                        None => Term::eq(x.clone(), lo),
                        Some(hi) => {
                            let hi = self.expr(hi, cx, env, flow)?;
                            Term::and(vec![Term::bin(ge, x.clone(), lo), Term::bin(le, x.clone(), hi)])
                        }
                    });
                }
                (Term::or(hits), Vec::new())
            }
        })
    }

    /// `x.or(d)` auf einem Optional: der Ersatz nur, wenn der Wert fehlt.
    fn or_value(&mut self, base: &Expr, default: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<V> {
        let result = match self.p.types.get(base.ty) {
            Type::Optional(_) => false,
            Type::Result { .. } => true,
            _ => return no("`.or` auf diesem Wert", base.span),
        };
        let v = self.value(base, cx, env, flow)?;
        let flag = v.clone().part(0, base.span)?.leaf(base.span)?;
        let has = if result { flag.not() } else { flag };
        let inner = v.part(1, base.span)?;
        let d = self.guarded(&has.clone().not(), flow, |enc, flow| enc.value(default, cx, env, flow))?;
        Ok(V::ite(&has, inner, d))
    }

    /// Eine Pruefung um einen zusammengesetzten Wert: der Zugriff auf ein
    /// Element, das Auspacken eines Optional; sonst die Pruefung am Blatt.
    fn checked_value(
        &mut self,
        kind: &CheckedKind,
        inner: &Expr,
        node: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<V> {
        match kind {
            CheckedKind::Index { len } => self.index_access(inner, *len, node.span, kind, cx, env, flow),
            CheckedKind::Missing => self.unwrap(inner, node.span, kind, cx, env, flow),
            CheckedKind::Range(_)
            | CheckedKind::DivZero
            | CheckedKind::NonFinite
            | CheckedKind::Overflow
            | CheckedKind::Shift
            | CheckedKind::Convert
            | CheckedKind::Domain
            | CheckedKind::Valid => no("Pruefung eines zusammengesetzten Werts", node.span),
        }
    }

    /// `a[i]` unter `Checked{Index}`: ausserhalb `0..len-1` ein Range-Fault.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn index_access(
        &mut self,
        inner: &Expr,
        len: u32,
        span: Span,
        kind: &CheckedKind,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<V> {
        // Der Index einer Zuweisungsstelle prueft `place_steps`.
        let ExprKind::Index { base, index } = &inner.kind else { return no("Indexpruefung ohne Zugriff", span) };
        if matches!(base.kind, ExprKind::Input { .. }) {
            return no("Channel-Array", span);
        }
        let v = self.value(base, cx, env, flow)?;
        let (elems, dynamic) = self.places(base.ty, v, span)?;
        let i = self.expr(index, cx, env, flow)?;
        // Eine Sammlung prueft gegen ihre Laenge, nicht gegen die Kapazitaet (`index_value`).
        let bound = dynamic.unwrap_or_else(|| Term::int(i64::from(len)));
        let fail = Term::or(vec![Term::bin(Op::Lt, i.clone(), Term::int(0)), Term::bin(Op::Ge, i.clone(), bound)]);
        self.fault(kind, span, fail, cx, flow);
        Ok(Enc::select(elems, &i))
    }

    /// Das Auspacken eines Optional (`Checked{Missing}`): ohne Wert ein
    /// `MissingValue`-Fault.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn unwrap(
        &mut self,
        inner: &Expr,
        span: Span,
        kind: &CheckedKind,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<V> {
        let result = match self.p.types.get(inner.ty) {
            Type::Optional(_) => false,
            Type::Result { .. } => true,
            _ => return no("Auspacken dieses Werts", span),
        };
        let v = self.value(inner, cx, env, flow)?;
        let flag = v.clone().part(0, span)?.leaf(span)?;
        let missing = if result { flag } else { flag.not() };
        self.fault(kind, span, missing, cx, flow);
        v.part(1, span)
    }

    /// Wurzel und Weg einer Zuweisungsstelle, die Indizes ausgewertet wie im
    /// Interpreter (`path`, `walk_mut`): aeusserer Index zuerst, ein
    /// negativer faultet sofort, ein zu grosser, wenn alle ausgewertet sind,
    /// innerer zuerst.
    pub(super) fn place_steps<'a>(
        &mut self,
        place: &'a Place,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<PlacePath<'a>> {
        let mut indices = Vec::new();
        let mut pending = Vec::new();
        let mut cur = place;
        loop {
            match cur {
                Place::Var(_) | Place::Output(_) | Place::Port(_) => break,
                Place::Field(b, _) => {
                    indices.push(None);
                    cur = b;
                }
                Place::Index(b, i) => {
                    let (index, check) = match &i.kind {
                        ExprKind::Checked { expr, kind: kind @ CheckedKind::Index { .. } } => {
                            (self.expr(expr, cx, env, flow)?, Some((kind.clone(), i.span)))
                        }
                        _ => (self.expr(i, cx, env, flow)?, None),
                    };
                    if let Some((kind, span)) = check {
                        self.fault(&kind, span, Term::bin(Op::Lt, index.clone(), Term::int(0)), cx, flow);
                        pending.push(Pending { kind, span, index: index.clone(), depth: indices.len() });
                    }
                    indices.push(Some(index));
                    cur = b;
                }
                Place::Index2(..) => return no("Matrixelement", span),
            }
        }
        // Von der Wurzel her gezaehlt.
        indices.reverse();
        let n = indices.len();
        for p in &mut pending {
            p.depth = n - 1 - p.depth;
        }
        pending.reverse();
        Ok((cur, indices, pending))
    }

    /// Die Schritte einer Stelle unter einer Wurzel des Typs `ty`, und die
    /// Grenze je Index: ein Array aus dem Typ, eine Sammlung aus dem Wert.
    fn typed_steps(&self, place: &Place, indices: Vec<Option<Term>>, ty: TypeId, span: Span) -> R<Vec<(Step, Bound)>> {
        let mut fields = Vec::new();
        let mut cur = place;
        loop {
            match cur {
                Place::Field(b, f) => {
                    fields.push(Some(*f as usize));
                    cur = b;
                }
                Place::Index(b, _) | Place::Index2(b, ..) => {
                    fields.push(None);
                    cur = b;
                }
                _ => break,
            }
        }
        fields.reverse();
        let mut ty = ty;
        let mut out = Vec::new();
        for (field, index) in fields.into_iter().zip(indices) {
            match (field, index, self.p.types.get(ty).clone()) {
                (Some(f), _, Type::Record(r)) => {
                    out.push((Step::Field(f), Bound::Fixed(0)));
                    ty = self.p.records[r.index()].fields[f].ty;
                }
                (None, Some(index), _) => {
                    let Some((elem, bound)) = self.element_of(ty) else { return no("Index auf diesem Wert", span) };
                    let offset = if matches!(bound, Bound::Dynamic) { 1 } else { 0 };
                    out.push((Step::Index { index, offset }, bound));
                    // Ein Byte hat keinen eigenen Typ; darunter geht es nicht weiter.
                    ty = elem.unwrap_or(ty);
                }
                _ => return no("Zuweisungsstelle", span),
            }
        }
        Ok(out)
    }

    /// `old` mit `v` an der Stelle; zuvor die oberen Grenzen der Indizes,
    /// von der Wurzel her, wie `walk_mut`.
    #[allow(clippy::too_many_arguments)]
    fn write_into(
        &mut self,
        place: &Place,
        indices: Vec<Option<Term>>,
        pending: Vec<Pending>,
        ty: TypeId,
        old: &V,
        v: V,
        cx: &Cx<'_>,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        let typed = self.typed_steps(place, indices, ty, span)?;
        let steps: Vec<Step> = typed.iter().map(|(s, _)| s.clone()).collect();
        for p in pending {
            let bound = match typed[p.depth].1 {
                Bound::Fixed(n) => Term::int(i64::from(n)),
                Bound::Dynamic => Enc::at(old.clone(), &steps[..p.depth], span)?.part(0, span)?.leaf(span)?,
            };
            self.fault(&p.kind, p.span, Term::bin(Op::Ge, p.index, bound), cx, flow);
        }
        Enc::update(old.clone(), &steps, v, span)
    }

    /// Der Typ einer Stelle in einer Maschine.
    fn place_type(&self, place: &Place, cx: &Cx<'_>) -> Option<TypeId> {
        Some(match place {
            Place::Var(id) => self.machine(cx.m?).vars[id.index()].ty,
            Place::Output(c) => self.p.channels[c.index()].ty,
            Place::Field(b, f) => {
                let Type::Record(r) = self.p.types.get(self.place_type(b, cx)?) else { return None };
                self.p.records[r.index()].fields[*f as usize].ty
            }
            Place::Index(b, _) => self.element_of(self.place_type(b, cx)?)?.0?,
            Place::Port(_) | Place::Index2(..) => return None,
        })
    }

    /// `push`, `append` und `clear` auf Bytes und Vektoren (3.9, `exec.rs`):
    /// alles oder nichts, das Ergebnis sagt, ob es passte. Die Stelle hat
    /// keinen Index, ihr Lesen also keine Wirkung.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn collection_method(
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
        let Some(ty) = self.place_type(receiver, cx) else { return no("Sammlungsmethode", span) };
        let Some(cap) = self.collection(ty) else { return no("Sammlungsmethode", span) };
        if self.has_index(receiver) {
            return no("Sammlungsmethode auf einem Element", span);
        }
        let mut xs = Vec::new();
        for a in args {
            xs.push(self.value(a, cx, env, flow)?);
        }
        let old = self.place_value(receiver, cx, env, span)?;
        let (new, done) = self.collection_update(method, ty, cap, old, &xs, span)?;
        self.assign(receiver, new, cx, env, flow, span)?;
        if let Some(t) = target {
            self.assign(t, V::Leaf(done), cx, env, flow, span)?;
        }
        Ok(())
    }

    /// `push`, `append` und `clear` auf einer Lokalen einer Funktion.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn local_collection_method(
        &mut self,
        target: Option<&Place>,
        receiver: &Place,
        method: Method,
        args: &[Expr],
        cx: &mut Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let Place::Var(id) = receiver else { return no("Sammlungsmethode auf einem Teil einer Lokalen", span) };
        let Some(ty) = self.local_types.get(id).copied() else { return no("Lokale", span) };
        let Some(cap) = self.collection(ty) else { return no("Sammlungsmethode", span) };
        let mut xs = Vec::new();
        for a in args {
            xs.push(self.value(a, cx, env, flow)?);
        }
        let Some(old) = cx.locals.as_ref().and_then(|l| l.get(id)).cloned() else { return no("Lokale", span) };
        let (new, done) = self.collection_update(method, ty, cap, old, &xs, span)?;
        self.assign_local(receiver, new, cx, env, flow, span)?;
        if let Some(t) = target {
            self.assign_local(t, V::Leaf(done), cx, env, flow, span)?;
        }
        Ok(())
    }

    /// Die Sammlung nach `push`, `append` oder `clear` (3.9, `exec.rs`) und
    /// ob es passte.
    fn collection_update(
        &mut self,
        method: Method,
        ty: TypeId,
        cap: u32,
        old: V,
        xs: &[V],
        span: Span,
    ) -> R<(V, Term)> {
        let shape = self.shape(ty, span)?;
        let (elems, len) = self.places(ty, old, span)?;
        let len = len.expect("Sammlung");
        let cap = Term::int(i64::from(cap));
        Ok(match (method, xs) {
            (Method::Push, [x]) => {
                let room = Term::bin(Op::Lt, len.clone(), cap);
                let parts: Vec<V> = elems
                    .into_iter()
                    .enumerate()
                    .map(|(k, e)| {
                        let here = Term::and(vec![room.clone(), Term::eq(len.clone(), Term::int(k as i64))]);
                        V::ite(&here, x.clone(), e)
                    })
                    .collect();
                let grown = Term::ite(room.clone(), Term::bin(Op::Add, len.clone(), Term::int(1)), len);
                (V::Node(std::iter::once(V::Leaf(grown)).chain(parts).collect()), room)
            }
            (Method::Append, [other]) => {
                let (theirs, their_len) = self.places(ty, other.clone(), span)?;
                let their_len = their_len.expect("Sammlung");
                let total = Term::bin(Op::Add, len.clone(), their_len);
                let fits = Term::bin(Op::Le, total.clone(), cap);
                let V::Node(zero) = self.zero_value(&shape, span)? else { return no("Sammlung", span) };
                let parts: Vec<V> = elems
                    .into_iter()
                    .zip(zero.into_iter().skip(1))
                    .enumerate()
                    .map(|(k, (e, z))| {
                        let k = Term::int(k as i64);
                        let from = Term::bin(Op::Sub, k.clone(), len.clone());
                        let appended =
                            V::ite(&Term::bin(Op::Lt, k.clone(), total.clone()), Enc::select(theirs.clone(), &from), z);
                        let next = V::ite(&Term::bin(Op::Lt, k, len.clone()), e.clone(), appended);
                        V::ite(&fits, next, e)
                    })
                    .collect();
                let grown = Term::ite(fits.clone(), total, len);
                (V::Node(std::iter::once(V::Leaf(grown)).chain(parts).collect()), fits)
            }
            (Method::Clear, []) => (self.zero_value(&shape, span)?, Term::bool(true)),
            _ => return no("Sammlungsmethode", span),
        })
    }

    fn has_index(&self, place: &Place) -> bool {
        match place {
            Place::Field(b, _) => self.has_index(b),
            Place::Index(..) | Place::Index2(..) => true,
            Place::Var(_) | Place::Output(_) | Place::Port(_) => false,
        }
    }

    /// Der Wert einer Stelle ohne Index.
    fn place_value(&mut self, place: &Place, cx: &Cx<'_>, env: &Env, span: Span) -> R<V> {
        let Some(m) = cx.m else { return no("Stelle ausserhalb einer Maschine", span) };
        let mut fields = Vec::new();
        let mut cur = place;
        let base = loop {
            match cur {
                Place::Var(id) => break self.loc_var(m, *id),
                Place::Output(c) => break self.loc_out(*c),
                Place::Field(b, f) => {
                    fields.push(*f as usize);
                    cur = b;
                }
                _ => return no("Stelle", span),
            }
        };
        let root = match cur {
            Place::Var(id) => self.machine(m).vars[id.index()].ty,
            Place::Output(c) => self.p.channels[c.index()].ty,
            _ => return no("Stelle", span),
        };
        let mut v = self.load(env, &base, &self.shape(root, span)?, span)?;
        for f in fields.into_iter().rev() {
            v = v.part(f, span)?;
        }
        Ok(v)
    }

    /// `for x in s` ueber Bytes oder einen Vektor: je Platz ein Durchlauf,
    /// der nur laeuft, solange der Platz unter der Laenge liegt.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_collection(
        &mut self,
        var_loc: &str,
        var_ty: TypeId,
        iter: &Expr,
        body: &Block,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let v = self.value(iter, cx, env, flow)?;
        let (items, len) = self.places(iter.ty, v, span)?;
        let Some(len) = len else { return no("`for … in`", span) };
        self.unrolled = self.unrolled.saturating_add(items.len() as i64);
        if self.unrolled > UNROLL_LIMIT {
            return no(format!("mehr als {UNROLL_LIMIT} Durchlaeufe von Schleifen auf einem Pfad"), span);
        }
        self.breaks.push(Vec::new());
        for (k, item) in items.into_iter().enumerate() {
            let inside = Term::bin(Op::Lt, Term::int(k as i64), len.clone());
            let mut env_k = env.clone();
            let mut fk = Flow::new(Term::and(vec![flow.alive.clone(), inside.clone()]));
            self.put(&mut env_k, var_loc, var_ty, item, &fk.alive.clone(), span)?;
            self.loop_path.push(k as i64);
            self.block(body, cx, &mut env_k, &mut fk)?;
            self.loop_path.pop();
            *env = ite_env(&inside, &env_k, env);
            flow.exits.extend(fk.exits);
            flow.alive = Term::or(vec![Term::and(vec![flow.alive.clone(), inside.not()]), fk.alive]);
        }
        self.left_loop(flow);
        Ok(())
    }

    /// Schreibt `v` an eine Stelle im Zustand einer Maschine, unter `alive`.
    pub(super) fn assign(
        &mut self,
        place: &Place,
        v: V,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let m = cx.m.expect("Maschine");
        let (root, indices, pending) = self.place_steps(place, cx, env, flow, span)?;
        let (base, ty) = match root {
            Place::Var(id) => (self.loc_var(m, *id), self.machine(m).vars[id.index()].ty),
            Place::Output(c) => (self.loc_out(*c), self.p.channels[c.index()].ty),
            _ => return no("Zuweisung an einen Port", span),
        };
        let shape = self.shape(ty, span)?;
        let old = self.load(env, &base, &shape, span)?;
        let new = self.write_into(place, indices, pending, ty, &old, v, cx, flow, span)?;
        Enc::store(env, &base, &shape, V::ite(&flow.alive, new, old));
        Ok(())
    }

    /// Schreibt `v` an eine Stelle unter einer Lokalen einer Funktion.
    pub(super) fn assign_local(
        &mut self,
        place: &Place,
        v: V,
        cx: &mut Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let (root, indices, pending) = self.place_steps(place, cx, env, flow, span)?;
        let Place::Var(id) = root else { return no("Zuweisung in einer Funktion", span) };
        let Some(old) = cx.locals.as_ref().and_then(|l| l.get(id)).cloned() else { return no("Lokale", span) };
        let Some(ty) = self.local_types.get(id).copied() else { return no("Lokale", span) };
        let new = self.write_into(place, indices, pending, ty, &old, v, cx, flow, span)?;
        cx.locals.as_mut().expect("Lokale").insert(*id, V::ite(&flow.alive, new, old));
        Ok(())
    }
}
