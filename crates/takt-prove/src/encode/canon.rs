//! Die kanonische Byteform (5.9, `bytes::write` und `bytes::read`) im
//! Modell: Schluessel einer map, Argumente einer Native, Elemente, die ein
//! Ausgabestrom ueber eine `sim`-Bindung in einen Eingabestrom speist. Teile
//! variabler Laenge verschieben, was folgt; die Stelle ist dann ein Term.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::TypeId;
use takt_mir::types::{IntWidth, Type};

use super::text::{Text, utf8};
use super::value::V;
use super::wire::{pick, put};
use super::{Enc, R, U64, no};
use crate::term::{Op, Term};

fn int(i: i64) -> Term {
    Term::int(i)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

/// Die Bytes einer Zahl, das niedrigste zuerst (`to_le_bytes`).
pub(super) fn le_bytes(x: &Term, n: i64) -> Vec<Term> {
    (0..n).map(|i| Term::bin(Op::BitAnd, Term::bin(Op::Shr, x.clone(), int(8 * i)), int(0xFF))).collect()
}

/// Die `n` Bytes ab `at`, das niedrigste zuerst, als Zahl.
fn from_le(bytes: &[Term], at: &Term, n: i64) -> Term {
    (0..n).fold(int(0), |acc, i| {
        let b = pick(bytes, &add(at.clone(), int(i)));
        Term::bin(Op::BitOr, acc, Term::bin(Op::Shl, b, int(8 * i)))
    })
}

/// Eine kanonische Byteform: die Plaetze bis zur Obergrenze, dahinter null,
/// und die geltende Laenge.
pub(super) struct Form {
    pub(super) bytes: Vec<Term>,
    pub(super) len: Term,
}

impl Form {
    fn fixed(bytes: Vec<Term>) -> Form {
        let len = int(bytes.len() as i64);
        Form { bytes, len }
    }

    /// `other` hinter dieser Form.
    fn then(mut self, other: Form) -> Form {
        let total = self.bytes.len() + other.bytes.len();
        self.bytes.resize(total, int(0));
        put(&mut self.bytes, &self.len, &other.bytes);
        Form { bytes: self.bytes, len: add(self.len, other.len) }
    }

    /// Nur, wo `c` gilt; sonst leer.
    fn when(self, c: &Term) -> Form {
        Form {
            bytes: self.bytes.into_iter().map(|b| Term::ite(c.clone(), b, int(0))).collect(),
            len: Term::ite(c.clone(), self.len, int(0)),
        }
    }

    /// `a`, wo `c` gilt, sonst `b`.
    fn choose(c: &Term, a: Form, b: Form) -> Form {
        let n = a.bytes.len().max(b.bytes.len());
        let at = |f: &Form, i: usize| f.bytes.get(i).cloned().unwrap_or_else(|| int(0));
        Form {
            bytes: (0..n).map(|i| Term::ite(c.clone(), at(&a, i), at(&b, i))).collect(),
            len: Term::ite(c.clone(), a.len, b.len),
        }
    }
}

/// Ein gelesener Wert: ob er gilt, der Wert und die Stelle dahinter.
pub(super) struct Read {
    pub(super) valid: Term,
    pub(super) value: V,
    pub(super) next: Term,
}

impl Enc<'_> {
    /// Die kanonische Form eines Werts (`bytes::write`).
    pub(super) fn canonical(&self, ty: TypeId, v: V, span: Span) -> R<Form> {
        Ok(match self.p.types.get(ty).clone() {
            Type::Bool => Form::fixed(vec![Term::ite(v.leaf(span)?, int(1), int(0))]),
            Type::Int { width: IntWidth::U64, .. } => return no(U64, span),
            Type::Int { width, .. } => Form::fixed(le_bytes(&v.leaf(span)?, i64::from(width.bits() / 8))),
            Type::Duration { .. } => Form::fixed(le_bytes(&v.leaf(span)?, 8)),
            Type::Enum(e) if !self.fielded(e) => {
                let x = v.leaf(span)?;
                let d = self.p.enums[e.index()].variants.iter().enumerate().rev().fold(int(0), |acc, (i, var)| {
                    Term::ite(Term::eq(x.clone(), int(i as i64)), int(var.discriminant), acc)
                });
                Form::fixed(le_bytes(&d, 8))
            }
            Type::Enum(e) => {
                let V::Node(parts) = v else { return no("Variante", span) };
                let tag = parts.first().cloned().map_or_else(|| no("Variante", span), |t| t.leaf(span))?;
                let def = self.p.enums[e.index()].clone();
                let slots = self.variant_parts(e);
                let mut out: Option<Form> = None;
                for (i, var) in def.variants.iter().enumerate().rev() {
                    let mut f = Form::fixed(le_bytes(&int(var.discriminant), 8));
                    for (field, slot) in var.fields.iter().zip(&slots[i]) {
                        let Some(x) = parts.get(*slot).cloned() else { return no("Variante", span) };
                        f = f.then(self.canonical(field.ty, x, span)?);
                    }
                    out = Some(match out {
                        None => f,
                        Some(rest) => Form::choose(&Term::eq(tag.clone(), int(i as i64)), f, rest),
                    });
                }
                out.map_or_else(|| no("Enum ohne Varianten", span), Ok)?
            }
            Type::Record(r) => {
                let V::Node(parts) = v else { return no("Record", span) };
                let fields = self.p.records[r.index()].fields.clone();
                let mut f = Form::fixed(Vec::new());
                for (field, x) in fields.iter().zip(parts) {
                    f = f.then(self.canonical(field.ty, x, span)?);
                }
                f
            }
            Type::Array { elem, .. } => {
                let V::Node(parts) = v else { return no("Array", span) };
                let mut f = Form::fixed(Vec::new());
                for x in parts {
                    f = f.then(self.canonical(elem, x, span)?);
                }
                f
            }
            // Laenge in vier Bytes, dann die Bytes; dahinter steht null.
            Type::Bytes { .. } | Type::Str { .. } => {
                let V::Node(parts) = v else { return no("Bytes", span) };
                let mut it = parts.into_iter();
                let len = it.next().map_or_else(|| no("Bytes", span), |x| x.leaf(span))?;
                let raw = it.map(|x| x.leaf(span)).collect::<R<Vec<_>>>()?;
                Form::fixed(le_bytes(&len, 4)).then(Form { len: len.clone(), bytes: raw })
            }
            Type::Vec { elem, .. } => {
                let V::Node(parts) = v else { return no("Vektor", span) };
                let mut it = parts.into_iter();
                let len = it.next().map_or_else(|| no("Vektor", span), |x| x.leaf(span))?;
                let mut f = Form::fixed(le_bytes(&len, 4));
                for (k, x) in it.enumerate() {
                    let inside = Term::bin(Op::Lt, int(k as i64), len.clone());
                    f = f.then(self.canonical(elem, x, span)?.when(&inside));
                }
                f
            }
            _ => return no("Byteform dieser Art", span),
        })
    }

    /// Ein Wert aus seiner kanonischen Form ab `at` (`bytes::read`): ein
    /// Wahrheitswert ist 0 oder 1, eine Diskriminante gehoert zum Enum, eine
    /// Laenge liegt unter der Kapazitaet, Text ist gueltiges UTF-8.
    pub(super) fn read_canonical(&self, ty: TypeId, bytes: &[Term], at: &Term, span: Span) -> R<Read> {
        let yes = Term::bool(true);
        let fixed =
            |value: Term, n: i64| Read { valid: yes.clone(), value: V::Leaf(value), next: add(at.clone(), int(n)) };
        Ok(match self.p.types.get(ty).clone() {
            Type::Bool => {
                let b = pick(bytes, at);
                let valid = Term::or(vec![Term::eq(b.clone(), int(0)), Term::eq(b.clone(), int(1))]);
                Read { valid, value: V::Leaf(Term::eq(b, int(1))), next: add(at.clone(), int(1)) }
            }
            Type::Int { width: IntWidth::U64, .. } => return no(U64, span),
            Type::Int { width, .. } => {
                let n = i64::from(width.bits() / 8);
                let raw = from_le(bytes, at, n);
                let x = match width.bits() {
                    bits if width.signed() && bits < 64 => Term::app(Op::Wrap { bits, signed: true }, vec![raw]),
                    _ => raw,
                };
                fixed(x, n)
            }
            Type::Duration { .. } => fixed(from_le(bytes, at, 8), 8),
            Type::Enum(e) => {
                let def = self.p.enums[e.index()].clone();
                let disc = from_le(bytes, at, 8);
                let hits: Vec<Term> =
                    def.variants.iter().map(|v| Term::eq(disc.clone(), int(v.discriminant))).collect();
                let start = add(at.clone(), int(8));
                if !self.fielded(e) {
                    let index = hits
                        .iter()
                        .enumerate()
                        .rev()
                        .fold(int(0), |acc, (i, h)| Term::ite(h.clone(), int(i as i64), acc));
                    return Ok(Read { valid: Term::or(hits), value: V::Leaf(index), next: start });
                }
                let zero = self.zero_of(ty, span)?;
                let (mut valid, mut value, mut next) = (Term::bool(false), zero, start.clone());
                for (i, (var, hit)) in def.variants.iter().zip(&hits).enumerate().rev() {
                    let (mut ok, mut pos, mut fields) = (Term::bool(true), start.clone(), Vec::new());
                    for f in &var.fields {
                        let r = self.read_canonical(f.ty, bytes, &pos, span)?;
                        ok = Term::and(vec![ok, r.valid]);
                        pos = r.next;
                        fields.push(r.value);
                    }
                    let v = self.variant(ty, i as u32, fields, span)?;
                    valid = Term::ite(hit.clone(), ok, valid);
                    value = V::ite(hit, v, value);
                    next = Term::ite(hit.clone(), pos, next);
                }
                Read { valid, value, next }
            }
            Type::Record(r) => {
                let fields = self.p.records[r.index()].fields.clone();
                self.read_sequence(fields.iter().map(|f| f.ty), bytes, at, span)?
            }
            Type::Array { elem, len } => self.read_sequence((0..len).map(|_| elem), bytes, at, span)?,
            Type::Bytes { cap } | Type::Str { cap } => {
                let n = from_le(bytes, at, 4);
                let start = add(at.clone(), int(4));
                let raw: Vec<Term> = (0..i64::from(cap))
                    .map(|k| {
                        let b = pick(bytes, &add(start.clone(), int(k)));
                        Term::ite(Term::bin(Op::Lt, int(k), n.clone()), b, int(0))
                    })
                    .collect();
                let mut valid = vec![Term::bin(Op::Le, n.clone(), int(i64::from(cap)))];
                if matches!(self.p.types.get(ty), Type::Str { .. }) {
                    valid.push(utf8(&Text { len: n.clone(), bytes: raw.clone() }));
                }
                let value = V::Node(std::iter::once(n.clone()).chain(raw).map(V::Leaf).collect());
                Read { valid: Term::and(valid), value, next: add(start, n) }
            }
            Type::Vec { elem, cap } => {
                let n = from_le(bytes, at, 4);
                let zero = self.zero_of(elem, span)?;
                let (mut valid, mut pos, mut items) =
                    (vec![Term::bin(Op::Le, n.clone(), int(i64::from(cap)))], add(at.clone(), int(4)), Vec::new());
                for k in 0..i64::from(cap) {
                    let inside = Term::bin(Op::Lt, int(k), n.clone());
                    let r = self.read_canonical(elem, bytes, &pos, span)?;
                    valid.push(Term::or(vec![inside.clone().not(), r.valid]));
                    pos = Term::ite(inside.clone(), r.next, pos);
                    items.push(V::ite(&inside, r.value, zero.clone()));
                }
                Read {
                    valid: Term::and(valid),
                    value: V::Node(std::iter::once(V::Leaf(n)).chain(items).collect()),
                    next: pos,
                }
            }
            _ => return no("Byteform dieser Art", span),
        })
    }

    /// Teile nacheinander, jeder ab dem Ende des vorigen.
    fn read_sequence(&self, tys: impl Iterator<Item = TypeId>, bytes: &[Term], at: &Term, span: Span) -> R<Read> {
        let (mut valid, mut pos, mut parts) = (Vec::new(), at.clone(), Vec::new());
        for ty in tys {
            let r = self.read_canonical(ty, bytes, &pos, span)?;
            valid.push(r.valid);
            pos = r.next;
            parts.push(r.value);
        }
        Ok(Read { valid: Term::and(valid), value: V::Node(parts), next: pos })
    }
}
