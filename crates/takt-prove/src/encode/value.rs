//! Zusammengesetzte Werte im Modell (M11 Schritt 27a). Ein Record, ein
//! Array, ein Optional und ein Enum mit Feldern sind ein Baum aus skalaren
//! Termen; ein Ort im Zustand haelt je Blatt eine Variable (`s.m.r.x`,
//! `s.m.a[3]`, `s.m.o.has`, `s.m.c.tag`). Ein fehlender Wert und die Felder
//! einer anderen Variante sind null wie im Standardwert des Interpreters
//! (`Value::default_for`); darum gilt Gleichheit Blatt fuer Blatt.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Accessor, CheckedKind, Expr, ExprKind};
use takt_mir::stmt::{ArmPattern, Place};
use takt_mir::types::Type;
use takt_mir::{EnumId, MachineId, TypeId};

use super::{Cx, Enc, Env, Exit, ExitKind, Flow, R, no};
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
    fn part(self, i: usize, span: Span) -> R<V> {
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
    /// Teile mit ihrem Pfad: `.feld`, `[i]`.
    Node(Vec<(String, Shape)>),
}

/// Ein Schritt auf dem Weg zu einem Teil.
#[derive(Clone, Debug)]
pub(super) enum Step {
    Field(usize),
    Index(Term),
}

impl Enc<'_> {
    /// Ein Typ, dessen Wert im Modell mehr als ein Blatt hat?
    pub(super) fn composite(&self, ty: TypeId) -> bool {
        match self.p.types.get(ty) {
            Type::Record(_) | Type::Array { .. } | Type::Optional(_) => true,
            Type::Enum(e) => self.fielded(*e),
            _ => false,
        }
    }

    fn fielded(&self, e: EnumId) -> bool {
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
            Type::Optional(t) => {
                Shape::Node(vec![(".has".into(), Shape::Flag), (".value".into(), self.shape(*t, span)?)])
            }
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

    fn leaf_sort(&self, s: &Shape, span: Span) -> R<Sort> {
        match s {
            Shape::Leaf(ty) => self.sort_of(*ty, span),
            Shape::Flag => Ok(Sort::Bool),
            Shape::Tag(_) => Ok(Sort::Int),
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

    /// Das Element `index` (ohne Pruefung: ausserhalb ist es das letzte).
    fn select(elems: Vec<V>, index: &Term) -> V {
        let mut it = elems.into_iter().enumerate().rev();
        let Some((_, mut acc)) = it.next() else { return V::Node(Vec::new()) };
        for (k, e) in it {
            acc = V::ite(&Term::eq(index.clone(), Term::int(k as i64)), e, acc);
        }
        acc
    }

    /// Der Wert mit `new` an der Stelle `steps` (aeusserster Schritt zuerst).
    fn update(v: V, steps: &[Step], new: V, span: Span) -> R<V> {
        let Some((step, rest)) = steps.split_first() else { return Ok(new) };
        let V::Node(mut parts) = v else { return no("Teil eines Werts", span) };
        match step {
            Step::Field(i) => {
                let Some(old) = parts.get(*i).cloned() else { return no("Feld", span) };
                parts[*i] = Enc::update(old, rest, new, span)?;
            }
            Step::Index(index) => {
                for (k, part) in parts.iter_mut().enumerate() {
                    let changed = Enc::update(part.clone(), rest, new.clone(), span)?;
                    *part = V::ite(&Term::eq(index.clone(), Term::int(k as i64)), changed, part.clone());
                }
            }
        }
        Ok(V::Node(parts))
    }

    /// Ein Fault-Zweig an einer Pruefstelle, wie in `checked`.
    fn fault(&mut self, kind: &CheckedKind, span: Span, fail: Term, cx: &Cx<'_>, flow: &mut Flow) {
        let cond = Term::and(vec![flow.alive.clone(), fail.clone()]);
        self.site(kind, span, cond.clone(), cx);
        flow.exits.push(Exit { cond, kind: ExitKind::Fault(None) });
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
            ExprKind::Var(v) => {
                if let Some(local) = cx.locals.as_ref().and_then(|l| l.get(v)) {
                    return Ok(local.clone());
                }
                let Some(m) = cx.m else { return no("Variable ausserhalb einer Maschine", span) };
                self.load(env, &self.loc_var(m, *v), &shape, span)?
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
                V::Node(parts)
            }
            ExprKind::Variant { variant, fields, .. } => {
                let mut values = Vec::new();
                for f in fields {
                    values.push(self.value(f, cx, env, flow)?);
                }
                self.variant(e.ty, *variant, values, span)?
            }
            ExprKind::Lift(x) => V::Node(vec![V::Leaf(Term::bool(true)), self.value(x, cx, env, flow)?]),
            ExprKind::Field { base, field } => self.field(base, *field as usize, cx, env, flow)?,
            ExprKind::Index { base, index } => self.element(base, index, cx, env, flow, span)?,
            ExprKind::Checked { expr: inner, kind } => self.checked_value(kind, inner, e, cx, env, flow)?,
            ExprKind::Cond { cond, then, otherwise } => {
                let c = self.expr(cond, cx, env, flow)?;
                let a = self.guarded(&c, flow, |enc, flow| enc.value(then, cx, env, flow))?;
                let b = self.guarded(&c.clone().not(), flow, |enc, flow| enc.value(otherwise, cx, env, flow))?;
                V::ite(&c, a, b)
            }
            ExprKind::Accessor { base, accessor: Accessor::Or, args } => {
                let [default] = args.as_slice() else { return no("`.or` ohne Ersatz", span) };
                self.or_value(base, default, cx, env, flow)?
            }
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
            | ExprKind::Str(_)
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
            | ExprKind::Slice { .. }
            | ExprKind::Accessor { .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::Cast { .. }
            | ExprKind::Convert { .. }
            | ExprKind::Format(_)
            | ExprKind::JobState { .. }
            | ExprKind::Stream(_)
            | ExprKind::Matches { .. }
            | ExprKind::NativeCall { .. }
            | ExprKind::MatOp { .. }
            | ExprKind::Decode { .. }
            | ExprKind::Ok(_)
            | ExprKind::Err(_)
            | ExprKind::Intrinsic { .. }) => {
                return no(format!("zusammengesetzter Ausdruck {}", super::node_name(other)), span);
            }
        })
    }

    /// Eine Variante eines Enums mit Feldern: die Variante in Teil 0, ihre
    /// Felder an ihren Stellen, alle anderen null.
    fn variant(&self, ty: TypeId, variant: u32, fields: Vec<V>, span: Span) -> R<V> {
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
        if !matches!(self.p.types.get(base.ty), Type::Array { .. }) {
            return no("Index auf diesem Wert", span);
        }
        let V::Node(elems) = self.value(base, cx, env, flow)? else { return no("Index", span) };
        let i = self.expr(index, cx, env, flow)?;
        Ok(Enc::select(elems, &i))
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
            (Accessor::Or, Type::Optional(_)) => {
                let [default] = args else { return no("`.or` ohne Ersatz", span) };
                self.or_value(base, default, cx, env, flow)?.leaf(span)
            }
            (Accessor::Len | Accessor::Count, Type::Array { len, .. }) => {
                let len = i64::from(*len);
                self.value(base, cx, env, flow)?;
                Ok(Term::int(len))
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
        if !matches!(self.p.types.get(base.ty), Type::Optional(_)) {
            return no("`.or` auf diesem Wert", base.span);
        }
        let v = self.value(base, cx, env, flow)?;
        let has = v.clone().part(0, base.span)?.leaf(base.span)?;
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
        let V::Node(elems) = self.value(base, cx, env, flow)? else { return no("Index", span) };
        let i = self.expr(index, cx, env, flow)?;
        let fail = Term::or(vec![
            Term::bin(Op::Lt, i.clone(), Term::int(0)),
            Term::bin(Op::Ge, i.clone(), Term::int(i64::from(len))),
        ]);
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
        if !matches!(self.p.types.get(inner.ty), Type::Optional(_)) {
            return no("Auspacken dieses Werts", span);
        }
        let v = self.value(inner, cx, env, flow)?;
        let has = v.clone().part(0, span)?.leaf(span)?;
        self.fault(kind, span, has.not(), cx, flow);
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
    ) -> R<(&'a Place, Vec<Step>)> {
        let mut steps = Vec::new();
        let mut above = Vec::new();
        let mut cur = place;
        loop {
            match cur {
                Place::Var(_) | Place::Output(_) | Place::Port(_) => break,
                Place::Field(b, f) => {
                    steps.push(Step::Field(*f as usize));
                    cur = b;
                }
                Place::Index(b, i) => {
                    let (index, check) = match &i.kind {
                        ExprKind::Checked { expr, kind: kind @ CheckedKind::Index { len } } => {
                            (self.expr(expr, cx, env, flow)?, Some((kind.clone(), i.span, *len)))
                        }
                        _ => (self.expr(i, cx, env, flow)?, None),
                    };
                    if let Some((kind, span, len)) = check {
                        self.fault(&kind, span, Term::bin(Op::Lt, index.clone(), Term::int(0)), cx, flow);
                        above.push((kind, span, Term::bin(Op::Ge, index.clone(), Term::int(i64::from(len)))));
                    }
                    steps.push(Step::Index(index));
                    cur = b;
                }
                Place::Index2(..) => return no("Matrixelement", span),
            }
        }
        for (kind, span, fail) in above.into_iter().rev() {
            self.fault(&kind, span, fail, cx, flow);
        }
        steps.reverse();
        Ok((cur, steps))
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
        let (root, steps) = self.place_steps(place, cx, env, flow, span)?;
        let (base, ty) = match root {
            Place::Var(id) => (self.loc_var(m, *id), self.machine(m).vars[id.index()].ty),
            Place::Output(c) => (self.loc_out(*c), self.p.channels[c.index()].ty),
            _ => return no("Zuweisung an einen Port", span),
        };
        let shape = self.shape(ty, span)?;
        let old = self.load(env, &base, &shape, span)?;
        let new = Enc::update(old.clone(), &steps, v, span)?;
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
        let (root, steps) = self.place_steps(place, cx, env, flow, span)?;
        let Place::Var(id) = root else { return no("Zuweisung in einer Funktion", span) };
        let locals = cx.locals.as_mut().expect("Lokale");
        let Some(old) = locals.get(id).cloned() else { return no("Lokale", span) };
        let new = Enc::update(old.clone(), &steps, v, span)?;
        locals.insert(*id, V::ite(&flow.alive, new, old));
        Ok(())
    }
}
