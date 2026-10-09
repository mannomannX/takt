//! Drahtformat der Records im Modell (3.7): `f.encode()` und `R.decode(b)`
//! Byte fuer Byte wie `takt-interp/src/wire.rs`. Ein Feld hinter
//! `bytes<N> with len = n` liegt um `N - n` frueher; die Verschiebung ist
//! ein Term, ein Byte an einer berechneten Stelle eine Auswahl unter den
//! Plaetzen. Ohne Laengenfeld falten alle Stellen zu Konstanten.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::types::{Const, Endian, IntWidth, Type};
use takt_mir::{RecordId, TypeId};

use super::value::V;
use super::{Enc, R, U64, int_bound, no};
use crate::term::{Node, Op, Term};

fn int(i: i64) -> Term {
    Term::int(i)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

fn sub(a: Term, b: Term) -> Term {
    Term::bin(Op::Sub, a, b)
}

/// Das Byte an `i`; ausserhalb null.
fn pick(bytes: &[Term], i: &Term) -> Term {
    if let Node::Int(k) = &*i.0 {
        return usize::try_from(*k).ok().and_then(|k| bytes.get(k)).cloned().unwrap_or_else(|| int(0));
    }
    bytes
        .iter()
        .enumerate()
        .rev()
        .fold(int(0), |acc, (k, b)| Term::ite(Term::eq(i.clone(), int(k as i64)), b.clone(), acc))
}

/// Schreibt `bytes` ab `at` in `out`.
pub(super) fn put(out: &mut [Term], at: &Term, bytes: &[Term]) {
    if let Node::Int(a) = &*at.0 {
        for (k, b) in bytes.iter().enumerate() {
            if let Some(slot) = usize::try_from(*a).ok().and_then(|a| out.get_mut(a + k)) {
                *slot = b.clone();
            }
        }
        return;
    }
    let width = int(bytes.len() as i64);
    for (j, slot) in out.iter_mut().enumerate() {
        let d = sub(int(j as i64), at.clone());
        let inside = Term::and(vec![Term::bin(Op::Ge, d.clone(), int(0)), Term::bin(Op::Lt, d.clone(), width.clone())]);
        *slot = Term::ite(inside, pick(bytes, &d), slot.clone());
    }
}

/// Die `n` Bytes einer Zahl in der Byte-Reihenfolge des Layouts (`put`).
fn int_bytes(x: &Term, n: i64, endian: Endian) -> Vec<Term> {
    (0..n)
        .map(|i| {
            let shift = match endian {
                Endian::Little => 8 * i,
                Endian::Big => 8 * (n - 1 - i),
            };
            Term::bin(Op::BitAnd, Term::bin(Op::Shr, x.clone(), int(shift)), int(0xFF))
        })
        .collect()
}

/// Ein Ausschnitt der Quelle von `decode`: ab `base`, `len` Byte lang.
struct View<'a> {
    bytes: &'a [Term],
    base: Term,
    len: Term,
}

impl View<'_> {
    /// Das Byte an `p` im Ausschnitt.
    fn at(&self, p: &Term) -> Term {
        pick(self.bytes, &add(self.base.clone(), p.clone()))
    }

    /// Der Ausschnitt ab `p`, `len` Byte lang.
    fn sub(&self, p: &Term, len: i64) -> View<'_> {
        View { bytes: self.bytes, base: add(self.base.clone(), p.clone()), len: int(len) }
    }

    /// Die `n` Bytes ab `p` als vorzeichenlose Zahl (`uint`).
    fn uint(&self, n: i64, endian: Endian) -> Term {
        let mut acc = int(0);
        for i in 0..n {
            let shift = match endian {
                Endian::Little => 8 * i,
                Endian::Big => 8 * (n - 1 - i),
            };
            let b = Term::bin(Op::Shl, self.at(&int(i)), int(shift));
            acc = Term::bin(Op::BitOr, acc, b);
        }
        acc
    }
}

impl Enc<'_> {
    /// `f.encode()` (3.7): der Record als `bytes<SIZE>`.
    pub(super) fn wire_encode(&self, r: RecordId, v: V, span: Span) -> R<V> {
        let (bytes, len) = self.encode_record(r, v, span)?;
        Ok(V::Node(std::iter::once(len).chain(bytes).map(V::Leaf).collect()))
    }

    /// `R.decode(b)` (3.7): ob die Bytes ein Record sind, und der Record.
    pub(super) fn wire_decode(&self, r: RecordId, b: V, span: Span) -> R<(Term, V)> {
        let V::Node(parts) = b else { return no("`decode` ohne Bytes", span) };
        let mut leaves = Vec::new();
        for p in parts {
            leaves.push(p.leaf(span)?);
        }
        let Some((len, bytes)) = leaves.split_first() else { return no("`decode` ohne Bytes", span) };
        self.decode_record(r, &View { bytes, base: int(0), len: len.clone() }, span)
    }

    /// Die Bytes eines Records bis `wire_size` und ihre Laenge; hinter der
    /// Laenge null.
    fn encode_record(&self, r: RecordId, v: V, span: Span) -> R<(Vec<Term>, Term)> {
        let def = &self.p.records[r.index()];
        let Some(layout) = &def.layout else { return no("Record ohne Drahtformat", span) };
        let (endian, size) = (layout.endian, self.wire_size(r, span)?);
        let V::Node(parts) = v else { return no("Record", span) };
        // Die Laenge jedes Feldes mit `len`, unter seinem Laengenfeld.
        let mut lens = Vec::new();
        for (i, f) in def.fields.iter().enumerate() {
            if let (Some(j), Some(b)) = (f.len_field, parts.get(i)) {
                lens.push((j as usize, b.clone().part(0, span)?.leaf(span)?));
            }
        }
        let mut out = vec![int(0); size as usize];
        let mut shift = int(0);
        for (i, f) in def.fields.iter().enumerate() {
            let at = sub(int(self.offset(f.offset, span)?), shift.clone());
            let width = self.field_size(f.ty, span)?;
            let Some(value) = parts.get(i).cloned() else { return no("Feld", span) };
            let value = match (&f.const_value, lens.iter().find(|(j, _)| *j == i)) {
                (Some(c), _) => V::Leaf(wire_const(c, span)?),
                (None, Some((_, n))) => V::Leaf(n.clone()),
                (None, None) => value,
            };
            let bytes = if f.len_field.is_some() {
                // Die Plaetze hinter der Laenge sind null; was danach kommt,
                // ueberschreibt sie.
                let V::Node(b) = value else { return no("Laengenfeld", span) };
                let mut it = b.into_iter();
                let n = it.next().map_or_else(|| no("Laengenfeld", span), |n| n.leaf(span))?;
                shift = add(shift, sub(int(width), n));
                it.map(|x| x.leaf(span)).collect::<R<Vec<_>>>()?
            } else {
                self.wire_bytes(f.ty, value, endian, span)?
            };
            put(&mut out, &at, &bytes);
        }
        let len = self.wire_length(r, &shift, span)?;
        if lens.is_empty() {
            return Ok((out, len));
        }
        let out = out
            .into_iter()
            .enumerate()
            .map(|(j, b)| Term::ite(Term::bin(Op::Lt, int(j as i64), len.clone()), b, int(0)))
            .collect();
        Ok((out, len))
    }

    /// Die Bytes eines Feldwerts (`write`).
    fn wire_bytes(&self, ty: TypeId, v: V, endian: Endian, span: Span) -> R<Vec<Term>> {
        Ok(match self.p.types.get(ty) {
            Type::Bool => vec![Term::ite(v.leaf(span)?, int(1), int(0))],
            Type::Int { width, .. } => int_bytes(&v.leaf(span)?, self.int_bytes_of(*width, span)?, endian),
            Type::Enum(e) => {
                let def = &self.p.enums[e.index()];
                if self.fielded(*e) {
                    return no("Enum mit Feldern im Drahtformat", span);
                }
                let x = v.leaf(span)?;
                let d = def.variants.iter().enumerate().rev().fold(int(0), |acc, (i, var)| {
                    Term::ite(Term::eq(x.clone(), int(i as i64)), int(var.discriminant), acc)
                });
                int_bytes(&d, self.field_size(ty, span)?, endian)
            }
            Type::Bytes { .. } => {
                let V::Node(parts) = v else { return no("Bytes", span) };
                parts.into_iter().skip(1).map(|x| x.leaf(span)).collect::<R<Vec<_>>>()?
            }
            Type::Array { elem, .. } => {
                let V::Node(parts) = v else { return no("Array", span) };
                let mut out = Vec::new();
                for p in parts {
                    out.extend(self.wire_bytes(*elem, p, endian, span)?);
                }
                out
            }
            Type::Record(r) => self.encode_record(*r, v, span)?.0,
            Type::Float { .. } => return no("Gleitkommafeld im Drahtformat", span),
            _ => return no("Feldtyp im Drahtformat", span),
        })
    }

    /// Ein Record aus einem Ausschnitt (`decode`): ob er gilt — lang genug,
    /// Konstanten, Ranges und Diskriminanten getroffen — und seine Felder.
    fn decode_record(&self, r: RecordId, view: &View<'_>, span: Span) -> R<(Term, V)> {
        let def = &self.p.records[r.index()];
        let Some(layout) = &def.layout else { return no("Record ohne Drahtformat", span) };
        let endian = layout.endian;
        let mut ok = Vec::new();
        let mut fields: Vec<V> = Vec::new();
        let mut shift = int(0);
        for f in &def.fields {
            let at = sub(int(self.offset(f.offset, span)?), shift.clone());
            let width = self.field_size(f.ty, span)?;
            let value = match f.len_field {
                Some(j) => {
                    let Some(n) = fields.get(j as usize).cloned() else { return no("Laengenfeld", span) };
                    let n = n.leaf(span)?;
                    // Die Laenge steht im Laengenfeld; ueber der Obergrenze
                    // ist der Rahmen fremd (3.7).
                    ok.push(Term::bin(Op::Ge, n.clone(), int(0)));
                    ok.push(Term::bin(Op::Le, n.clone(), int(width)));
                    ok.push(Term::bin(Op::Le, add(at.clone(), n.clone()), view.len.clone()));
                    shift = add(shift, sub(int(width), n.clone()));
                    let bytes = (0..width).map(|k| {
                        let b = view.at(&add(at.clone(), int(k)));
                        V::Leaf(Term::ite(Term::bin(Op::Lt, int(k), n.clone()), b, int(0)))
                    });
                    V::Node(std::iter::once(V::Leaf(n.clone())).chain(bytes).collect())
                }
                None => {
                    ok.push(Term::bin(Op::Le, add(at.clone(), int(width)), view.len.clone()));
                    let (valid, v) = self.wire_read(f.ty, &view.sub(&at, width), endian, span)?;
                    ok.push(valid);
                    v
                }
            };
            if let Some(want) = &f.const_value {
                ok.push(Term::eq(value.clone().leaf(span)?, wire_const(want, span)?));
            }
            if let Type::Int { range: Some(range), .. } = self.p.types.get(f.ty) {
                let x = value.clone().leaf(span)?;
                ok.push(Term::bin(Op::Ge, x.clone(), int(int_bound(&range.lo))));
                ok.push(Term::bin(Op::Le, x, int(int_bound(&range.hi))));
            }
            fields.push(value);
        }
        ok.push(Term::bin(Op::Ge, view.len.clone(), self.wire_length(r, &shift, span)?));
        Ok((Term::and(ok), V::Node(fields)))
    }

    /// Ein Feldwert aus seinem Ausschnitt (`read`) und ob er gilt.
    fn wire_read(&self, ty: TypeId, view: &View<'_>, endian: Endian, span: Span) -> R<(Term, V)> {
        let yes = Term::bool(true);
        Ok(match self.p.types.get(ty) {
            Type::Bool => (yes, V::Leaf(Term::eq(view.at(&int(0)), int(0)).not())),
            Type::Int { width, .. } => {
                let n = view.uint(self.int_bytes_of(*width, span)?, endian);
                let x = match width.bits() {
                    bits if width.signed() && bits < 64 => Term::app(Op::Wrap { bits, signed: true }, vec![n]),
                    _ => n,
                };
                (yes, V::Leaf(x))
            }
            Type::Enum(e) => {
                if self.fielded(*e) {
                    return no("Enum mit Feldern im Drahtformat", span);
                }
                // Der Wert muss eine deklarierte Diskriminante treffen (3.7).
                let n = view.uint(self.field_size(ty, span)?, endian);
                let variants = &self.p.enums[e.index()].variants;
                let hit = |d: i64| Term::eq(n.clone(), int(d));
                let valid = Term::or(variants.iter().map(|v| hit(v.discriminant)).collect());
                let index = variants
                    .iter()
                    .enumerate()
                    .rev()
                    .fold(int(0), |acc, (i, v)| Term::ite(hit(v.discriminant), int(i as i64), acc));
                (valid, V::Leaf(index))
            }
            Type::Bytes { cap } => {
                let bytes = (0..i64::from(*cap)).map(|k| V::Leaf(view.at(&int(k))));
                (yes, V::Node(std::iter::once(V::Leaf(int(i64::from(*cap)))).chain(bytes).collect()))
            }
            Type::Array { elem, len } => {
                let width = self.field_size(*elem, span)?;
                let (mut valid, mut items) = (Vec::new(), Vec::new());
                for i in 0..i64::from(*len) {
                    let (ok, v) = self.wire_read(*elem, &view.sub(&int(i * width), width), endian, span)?;
                    valid.push(ok);
                    items.push(v);
                }
                (Term::and(valid), V::Node(items))
            }
            Type::Record(r) => self.decode_record(*r, view, span)?,
            Type::Float { .. } => return no("Gleitkommafeld im Drahtformat", span),
            _ => return no("Feldtyp im Drahtformat", span),
        })
    }

    /// Die Breite einer Ganzzahl im Draht in Bytes.
    fn int_bytes_of(&self, width: IntWidth, span: Span) -> R<i64> {
        if width == IntWidth::U64 {
            return no(U64, span);
        }
        Ok(i64::from(width.bits() / 8))
    }

    /// Groesse eines Feldtyps in Bytes (`field_size`).
    fn field_size(&self, ty: TypeId, span: Span) -> R<i64> {
        Ok(match self.p.types.get(ty) {
            Type::Bool => 1,
            Type::Int { width, .. } => i64::from(width.bits() / 8),
            Type::Bytes { cap } => i64::from(*cap),
            Type::Array { elem, len } => self.field_size(*elem, span)? * i64::from(*len),
            Type::Enum(e) => self.p.enums[e.index()].layout.map_or(1, |w| i64::from(w.bits() / 8)),
            Type::Record(r) => self.wire_size(*r, span)?,
            Type::Float { .. } => return no("Gleitkommafeld im Drahtformat", span),
            _ => return no("Feldtyp im Drahtformat", span),
        })
    }

    fn wire_size(&self, r: RecordId, span: Span) -> R<i64> {
        match self.p.records[r.index()].wire_size {
            Some(n) => Ok(i64::from(n)),
            None => no("Record ohne Drahtformat", span),
        }
    }

    fn offset(&self, offset: Option<u32>, span: Span) -> R<i64> {
        match offset {
            Some(o) => Ok(i64::from(o)),
            None => no("Feld ohne Stelle im Drahtformat", span),
        }
    }

    /// Die Laenge eines Rahmens, dessen variable Felder zusammen `shift`
    /// Byte unter ihrer Obergrenze bleiben (`length`).
    fn wire_length(&self, r: RecordId, shift: &Term, span: Span) -> R<Term> {
        let size = int(self.wire_size(r, span)?);
        if let Node::Int(0) = &*shift.0 {
            return Ok(size);
        }
        let def = &self.p.records[r.index()];
        let mut end = 0;
        for f in &def.fields {
            end = end.max(self.offset(f.offset, span)? + self.field_size(f.ty, span)?);
        }
        let end = sub(int(end), shift.clone());
        let aligned = match def.layout.as_ref().and_then(|l| l.align) {
            Some(a) if a > 1 => {
                let a = int(i64::from(a));
                let up = Term::bin(Op::Div, add(end, sub(a.clone(), int(1))), a.clone());
                Term::bin(Op::Mul, up, a)
            }
            _ => end,
        };
        Ok(Term::ite(Term::eq(shift.clone(), int(0)), size, aligned))
    }
}

/// Der Wert eines Konstantenfelds (`const_value`).
fn wire_const(c: &Const, span: Span) -> R<Term> {
    match c {
        Const::Int(i) => Ok(int(*i)),
        Const::Bool(b) => Ok(Term::bool(*b)),
        Const::Float(_) | Const::Duration(_) => no("Konstantenfeld dieses Typs im Drahtformat", span),
    }
}
