//! Text im Modell (M11 Schritt 27c-2, Referenz 3.9, 8.7). Ein `str<N>` ist
//! seine Laenge und seine Bytes bis zur Kapazitaet, dahinter null; eine
//! `line<N>` traegt dazu `.truncated`. Ein Muster ist der Vorwaertsdurchlauf
//! des Interpreters (`takt_interp::pattern`) ueber diese Bytes: Ein Literal
//! vergleicht Byte fuer Byte, eine Klasse zaehlt ihren Lauf, `str` und `{_}`
//! enden am ersten Vorkommen des folgenden Literals. Solange die Stelle
//! feststeht, rechnet der Durchlauf mit ihr als Zahl, danach als Term.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::TypeId;
use takt_mir::expr::{Expr, MatchKind};
use takt_mir::pattern::{Format, FormatPiece, PatternPiece};
use takt_mir::types::{IntWidth, Type};

use super::value::{Shape, V};
use super::{Cx, Enc, Env, Flow, R, no};
use crate::term::{Op, Term};

/// Ein Text: Laenge und Bytes, hinter der Laenge null.
#[derive(Clone, Debug)]
pub(super) struct Text {
    pub(super) len: Term,
    pub(super) bytes: Vec<Term>,
}

/// Eine Stelle im Text: fest oder berechnet.
#[derive(Clone, Debug)]
pub(super) enum Pos {
    At(i64),
    Sym(Term),
}

impl Pos {
    fn plus(&self, k: i64) -> Pos {
        match self {
            Pos::At(i) => Pos::At(i + k),
            Pos::Sym(t) => Pos::Sym(add(t.clone(), Term::int(k))),
        }
    }
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

fn sub(a: Term, b: Term) -> Term {
    Term::bin(Op::Sub, a, b)
}

fn int(i: i64) -> Term {
    Term::int(i)
}

fn within(x: &Term, lo: i64, hi: i64) -> Term {
    Term::and(vec![Term::bin(Op::Ge, x.clone(), int(lo)), Term::bin(Op::Le, x.clone(), int(hi))])
}

/// Beginnt an diesem Byte ein Zeichen (kein Folgebyte `10xxxxxx`)?
pub(super) fn lead(c: &Term) -> Term {
    Term::or(vec![Term::bin(Op::Lt, c.clone(), int(0x80)), Term::bin(Op::Ge, c.clone(), int(0xC0))])
}

fn min(a: Term, b: Term) -> Term {
    Term::ite(Term::bin(Op::Lt, a.clone(), b.clone()), a, b)
}

fn max(a: Term, b: Term) -> Term {
    Term::ite(Term::bin(Op::Gt, a.clone(), b.clone()), a, b)
}

/// Die Gestalt eines Texts: Laenge, Bytes, bei einer Zeile `.truncated`.
pub(super) fn text_shape(cap: u32, line: bool) -> Shape {
    let mut parts: Vec<(String, Shape)> = std::iter::once((".len".to_string(), Shape::Count(cap)))
        .chain((0..cap).map(|i| (format!("[{i}]"), Shape::Byte)))
        .collect();
    if line {
        parts.push((".truncated".into(), Shape::Flag));
    }
    Shape::Node(parts)
}

impl Text {
    /// Der Text eines Werts mit Textgestalt (oder einer Bytefolge).
    pub(super) fn of(v: V, span: Span) -> R<Text> {
        let V::Node(parts) = v else { return no("Text", span) };
        let mut it = parts.into_iter();
        let Some(len) = it.next() else { return no("Text", span) };
        let len = len.leaf(span)?;
        let bytes = it.filter_map(|p| match p {
            V::Leaf(t) if t.sort() == crate::term::Sort::Int => Some(t),
            _ => None,
        });
        Ok(Text { len, bytes: bytes.collect() })
    }

    /// Ein konstanter Text.
    pub(super) fn literal(s: &str) -> Text {
        Text { len: int(s.len() as i64), bytes: s.bytes().map(|b| int(i64::from(b))).collect() }
    }

    fn cap(&self) -> i64 {
        self.bytes.len() as i64
    }

    /// Als Wert der Kapazitaet `cap`: was dahinter laege, faellt weg — der
    /// Typ garantiert, dass nichts dort liegt.
    pub(super) fn value(&self, cap: u32, truncated: Option<Term>) -> V {
        let mut parts = vec![V::Leaf(self.len.clone())];
        for i in 0..cap as usize {
            parts.push(V::Leaf(self.bytes.get(i).cloned().unwrap_or_else(|| int(0))));
        }
        if let Some(t) = truncated {
            parts.push(V::Leaf(t));
        }
        V::Node(parts)
    }

    /// Das Byte an `p`; -1 hinter dem Ende.
    pub(super) fn at(&self, p: &Pos) -> Term {
        match p {
            Pos::At(i) => match usize::try_from(*i).ok().and_then(|i| self.bytes.get(i)) {
                Some(b) => Term::ite(Term::bin(Op::Lt, int(*i), self.len.clone()), b.clone(), int(-1)),
                None => int(-1),
            },
            Pos::Sym(t) => {
                let mut acc = int(-1);
                for (i, b) in self.bytes.iter().enumerate().rev() {
                    acc = Term::ite(Term::eq(t.clone(), int(i as i64)), b.clone(), acc);
                }
                Term::ite(Term::bin(Op::Lt, t.clone(), self.len.clone()), acc, int(-1))
            }
        }
    }

    /// Ein Ausschnitt ab `from` der Laenge `len` als Text der Kapazitaet `cap`.
    pub(super) fn slice(&self, from: &Pos, len: &Term, cap: i64) -> Text {
        let bytes = (0..cap)
            .map(|j| Term::ite(Term::bin(Op::Lt, int(j), len.clone()), self.at(&from.plus(j)), int(0)))
            .collect();
        Text { len: min(len.clone(), int(cap)), bytes }
    }

    /// Gleich als Text (`value::same`): Laenge und Bytes, `.truncated` zaehlt nicht.
    pub(super) fn equal(&self, other: &Text) -> Term {
        let mut conds = vec![Term::eq(self.len.clone(), other.len.clone())];
        for (a, b) in self.bytes.iter().zip(&other.bytes) {
            conds.push(Term::eq(a.clone(), b.clone()));
        }
        // Die laengere Kapazitaet traegt hinter der kuerzeren nur Nullen,
        // wenn die Laengen gleich sind.
        let short = self.cap().min(other.cap());
        conds.push(Term::bin(Op::Le, self.len.clone(), int(short)));
        Term::and(conds)
    }

    /// Haengt `other` an; `cap` begrenzt das Ergebnis.
    fn concat(&self, other: &Text, cap: i64) -> Text {
        let bytes = (0..cap)
            .map(|p| {
                let mine = Term::bin(Op::Lt, int(p), self.len.clone());
                let theirs = other.at(&Pos::Sym(sub(int(p), self.len.clone())));
                let here = self.bytes.get(p as usize).cloned().unwrap_or_else(|| int(0));
                let b = Term::ite(mine, here, theirs);
                // Hinter dem Ende null: `at` liefert dort -1.
                Term::ite(Term::bin(Op::Lt, b.clone(), int(0)), int(0), b)
            })
            .collect();
        Text { len: add(self.len.clone(), other.len.clone()), bytes }
    }

    /// Kuerzt auf hoechstens `max` Bytes an einer Zeichengrenze (`format::truncate`).
    pub(super) fn truncate(&self, max: i64) -> Text {
        let over = Term::bin(Op::Gt, self.len.clone(), int(max));
        // Die groesste Zeichengrenze bis `max`: ein Zeichen hat hoechstens vier Bytes.
        let mut cut = int(0);
        for c in (max - 3).max(0)..=max {
            let boundary = match self.bytes.get(c as usize) {
                Some(b) => lead(b),
                None => Term::bool(true),
            };
            cut = Term::ite(boundary, int(c), cut);
        }
        let len = Term::ite(over, cut, self.len.clone());
        let bytes = (0..max.min(self.cap()))
            .map(|p| Term::ite(Term::bin(Op::Lt, int(p), len.clone()), self.bytes[p as usize].clone(), int(0)))
            .collect();
        Text { len, bytes }
    }
}

impl Enc<'_> {
    /// Ein Text-Typ: Kapazitaet und ob er eine Zeile ist.
    pub(super) fn text_type(&self, ty: TypeId) -> Option<(u32, bool)> {
        match self.p.types.get(ty) {
            Type::Str { cap } => Some((*cap, false)),
            Type::Line { cap } => Some((*cap, true)),
            _ => None,
        }
    }

    /// Ein Text als Wert seines Typs; eine Zeile ist nicht gekuerzt.
    pub(super) fn text_value(&self, t: &Text, ty: TypeId, span: Span) -> R<V> {
        match self.text_type(ty) {
            Some((cap, line)) => Ok(t.value(cap, line.then(|| Term::bool(false)))),
            None => no("Text an dieser Stelle", span),
        }
    }

    /// Gleicht ein Textmuster ab (`pattern::match_text`, `match_has`): ob es
    /// trifft und was es bindet.
    pub(super) fn text_match(
        &mut self,
        pieces: &[PatternPiece],
        kind: MatchKind,
        text: &Text,
        span: Span,
    ) -> R<(Term, Vec<V>)> {
        super::pattern::text_match(pieces, kind, text, span)
    }

    /// `x matches P as m` auf einem Wert (8.7): Text- oder Record-Muster;
    /// die Bindung traegt die Captures und den Wert unter `.text`/`.data`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn value_match(
        &mut self,
        subject: &Expr,
        kind: MatchKind,
        pattern: &takt_mir::pattern::Pattern,
        binding: Option<takt_mir::VarId>,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<Term> {
        let v = self.value(subject, cx, env, flow)?;
        let (ok, caps) = match pattern {
            takt_mir::pattern::Pattern::Text { pieces } => {
                let text = Text::of(v.clone(), span)?;
                self.text_match(pieces, kind, &text, span)?
            }
            takt_mir::pattern::Pattern::Record { fields, .. } => {
                let mut conds = Vec::new();
                for (index, e) in fields {
                    let want = self.value(e, cx, env, flow)?;
                    conds.push(Enc::equal(&v.clone().part(*index as usize, span)?, &want));
                }
                (Term::and(conds), Vec::new())
            }
        };
        if let Some(var) = binding {
            let Some(m) = cx.m.filter(|_| cx.locals.as_ref().is_none_or(|l| !l.contains_key(&var))) else {
                return no("Musterbindung in einer Funktion", span);
            };
            let ty = self.machine(m).vars[var.index()].ty;
            let Type::Record(r) = self.p.types.get(ty) else { return no("Bindung ohne Record", span) };
            let fields = self.p.records[r.index()].fields.clone();
            let mut parts = caps;
            for f in fields.iter().skip(parts.len()) {
                parts.push(match f.name.as_str() {
                    "text" | "data" => v.clone(),
                    _ => self.zero_of(f.ty, span)?,
                });
            }
            parts.truncate(fields.len());
            let cond = Term::and(vec![flow.alive.clone(), ok.clone()]);
            self.binds.push((self.loc_var(m, var), ty, V::Node(parts), cond));
        }
        Ok(ok)
    }

    /// Schreibt die Bindungen, die Musterabgleiche im laufenden Ausdruck
    /// vorgemerkt haben.
    pub(super) fn flush_binds(&mut self, env: &mut Env) -> R<()> {
        for (at, ty, v, cond) in std::mem::take(&mut self.binds) {
            self.put(env, &at, ty, v, &cond, Span::default())?;
        }
        Ok(())
    }

    /// Ein Wert, gelesen ueber die vorgemerkten Bindungen hinweg.
    pub(super) fn bound_value(&self, at: &str, v: V) -> V {
        self.binds.iter().filter(|(l, ..)| l == at).fold(v, |acc, (_, _, b, cond)| V::ite(cond, b.clone(), acc))
    }

    /// `s.starts_with(t)` und `s.contains(t)` (3.9).
    pub(super) fn text_test(&mut self, s: &Text, t: &Text, contains: bool) -> Term {
        let at = |i: i64| {
            let mut conds = vec![Term::bin(Op::Le, add(int(i), t.len.clone()), s.len.clone())];
            for (j, b) in t.bytes.iter().enumerate() {
                let inside = Term::bin(Op::Lt, int(j as i64), t.len.clone());
                conds.push(Term::or(vec![inside.not(), Term::eq(s.at(&Pos::At(i + j as i64)), b.clone())]));
            }
            Term::and(conds)
        };
        if !contains {
            return at(0);
        }
        Term::or((0..=s.cap()).map(at).collect())
    }

    /// Ein Formatstring als Wert seines Texttyps.
    pub(super) fn format(&mut self, f: &Format, ty: TypeId, cx: &Cx<'_>, env: &Env, flow: &Flow, span: Span) -> R<V> {
        if self.text_type(ty).is_none() {
            return no("Formatstring ohne Texttyp", span);
        }
        let text = self.render(f, cx, env, flow)?;
        self.text_value(&text, ty, span)
    }

    /// Ein Formatstring (3.9, `format::render`): Platzhalter ausgewertet, ein
    /// Fault darin wird `<invalid>`, das Ergebnis auf `len_max` Bytes an einer
    /// Zeichengrenze gekuerzt.
    pub(super) fn render(&mut self, f: &Format, cx: &Cx<'_>, env: &Env, flow: &Flow) -> R<Text> {
        // Was hinter `len_max` laege, faellt beim Kuerzen weg.
        let width = i64::from(f.len_max).saturating_add(64);
        let mut out = Text::literal("");
        for piece in &f.pieces {
            let part = match piece {
                FormatPiece::Text(t) => Text::literal(t),
                FormatPiece::Expr { expr, spec } => {
                    // Ein Fault im Platzhalter faultet nicht (3.9): Die
                    // Pruefstellen darin bleiben aussen vor.
                    let sites = std::mem::take(&mut self.sites);
                    let mut fx = Flow::new(flow.alive.clone());
                    let shown = self.display(expr, spec.as_deref(), cx, env, &mut fx);
                    self.sites = sites;
                    let shown = shown?;
                    let failed = Term::or(fx.exits.iter().map(|x| x.cond.clone()).collect());
                    if failed.is_bool(false) {
                        shown
                    } else {
                        let invalid = Text::literal("<invalid>");
                        let w = shown.cap().max(invalid.cap());
                        let pick = |a: &Text, b: &Text| Text {
                            len: Term::ite(failed.clone(), a.len.clone(), b.len.clone()),
                            bytes: (0..w as usize)
                                .map(|i| {
                                    let x = a.bytes.get(i).cloned().unwrap_or_else(|| int(0));
                                    let y = b.bytes.get(i).cloned().unwrap_or_else(|| int(0));
                                    Term::ite(failed.clone(), x, y)
                                })
                                .collect(),
                        };
                        pick(&invalid, &shown)
                    }
                }
            };
            let joined = (out.cap() + part.cap()).min(width);
            out = out.concat(&part, joined);
        }
        Ok(out.truncate(i64::from(f.len_max)))
    }

    /// Ein Wert in Textform (`format::display`): Ganzzahlen dezimal, `hex`
    /// oder mit Nullen aufgefuellt, Wahrheitswerte, Varianten ohne Felder und
    /// Text.
    fn display(&mut self, e: &Expr, spec: Option<&str>, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Text> {
        let span = e.span;
        if self.text_type(e.ty).is_some() {
            let v = self.value(e, cx, env, flow)?;
            return Text::of(v, span);
        }
        match self.p.types.get(e.ty).clone() {
            Type::Int { width, .. } => {
                let x = self.expr(e, cx, env, flow)?;
                let unsigned = width == IntWidth::U64;
                Ok(match spec {
                    Some("hex") => hex_text(&x),
                    Some(s) if s.starts_with('0') => decimal_text(&x, s.parse().unwrap_or(0), unsigned),
                    _ => decimal_text(&x, 0, unsigned),
                })
            }
            Type::Bool => {
                let b = self.expr(e, cx, env, flow)?;
                Ok(choose(&[(b.clone(), Text::literal("true")), (b.not(), Text::literal("false"))]))
            }
            Type::Enum(id) if !self.fielded(id) => {
                let x = self.expr(e, cx, env, flow)?;
                let names: Vec<(Term, Text)> = self.p.enums[id.index()]
                    .variants
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (Term::eq(x.clone(), int(i as i64)), Text::literal(&v.name)))
                    .collect();
                Ok(choose(&names))
            }
            _ => no("Format eines Werts dieser Art", span),
        }
    }
}

/// Einer von mehreren Texten; genau eine Bedingung gilt.
fn choose(options: &[(Term, Text)]) -> Text {
    let cap = options.iter().map(|(_, t)| t.cap()).max().unwrap_or(0) as usize;
    let mut out = Text { len: int(0), bytes: vec![int(0); cap] };
    for (c, t) in options.iter().rev() {
        out = Text {
            len: Term::ite(c.clone(), t.len.clone(), out.len),
            bytes: (0..cap)
                .map(|i| Term::ite(c.clone(), t.bytes.get(i).cloned().unwrap_or_else(|| int(0)), out.bytes[i].clone()))
                .collect(),
        };
    }
    out
}

/// Eine Ganzzahl dezimal (`i.to_string()`), mit `width` als `{:0width$}`;
/// ein `u64` ohne Vorzeichen mit bis zu 20 Ziffern.
fn decimal_text(x: &Term, width: i64, unsigned: bool) -> Text {
    let negative = if unsigned { Term::bool(false) } else { Term::bin(Op::Lt, x.clone(), int(0)) };
    // Die Ziffern von hinten: |(x / 10^k) % 10|, ohne `|x|`, das bei
    // `i64::MIN` ueberliefe.
    let mut digits = Vec::new();
    let mut power = 1u64;
    let mut count = int(1);
    for k in 0..if unsigned { 20 } else { 19 } {
        let p = int(power as i64);
        let r = if unsigned {
            Term::bin(Op::URem, Term::bin(Op::UDiv, x.clone(), p.clone()), int(10))
        } else {
            let r = Term::bin(Op::Rem, Term::bin(Op::Div, x.clone(), p.clone()), int(10));
            Term::ite(Term::bin(Op::Lt, r.clone(), int(0)), sub(int(0), r.clone()), r)
        };
        digits.push(r);
        if k > 0 {
            let big = if unsigned {
                Term::bin(Op::UGe, x.clone(), p)
            } else {
                Term::or(vec![Term::bin(Op::Ge, x.clone(), p), Term::bin(Op::Le, x.clone(), int(-(power as i64)))])
            };
            count = add(count, Term::ite(big, int(1), int(0)));
        }
        power = power.saturating_mul(10);
    }
    let sign = Term::ite(negative.clone(), int(1), int(0));
    let natural = add(count.clone(), sign.clone());
    let len = max(natural.clone(), int(width));
    let pad = sub(len.clone(), natural);
    let cap = 20.max(width);
    let bytes = (0..cap)
        .map(|p| {
            let p = int(p);
            // Stelle der Ziffer: von vorn nach Vorzeichen und Nullen.
            let index = sub(
                sub(sub(count.clone(), int(1)), sub(p.clone(), sign.clone())),
                Term::bin(Op::Sub, int(0), pad.clone()),
            );
            let mut d = int(0);
            for (k, dk) in digits.iter().enumerate().rev() {
                d = Term::ite(Term::eq(index.clone(), int(k as i64)), dk.clone(), d);
            }
            let zero = Term::bin(Op::Lt, sub(p.clone(), sign.clone()), pad.clone());
            let b = Term::ite(
                Term::and(vec![negative.clone(), Term::eq(p.clone(), int(0))]),
                int(i64::from(b'-')),
                Term::ite(zero, int(i64::from(b'0')), add(int(i64::from(b'0')), d)),
            );
            Term::ite(Term::bin(Op::Lt, p, len.clone()), b, int(0))
        })
        .collect();
    Text { len, bytes }
}

/// Eine Ganzzahl hexadezimal in Kleinbuchstaben, als `u64` (`{:x}`).
fn hex_text(x: &Term) -> Text {
    let nibble = |k: i64| Term::bin(Op::BitAnd, Term::bin(Op::Shr, x.clone(), int(4 * k)), int(15));
    let mut count = int(1);
    for k in 1..16 {
        let rest = Term::bin(Op::Shr, x.clone(), int(4 * k));
        count = add(count, Term::ite(Term::eq(rest, int(0)), int(0), int(1)));
    }
    let bytes = (0..16)
        .map(|p| {
            let index = sub(sub(count.clone(), int(1)), int(p));
            let mut d = int(0);
            for k in (0..16).rev() {
                d = Term::ite(Term::eq(index.clone(), int(k)), nibble(k), d);
            }
            let ch = Term::ite(Term::bin(Op::Lt, d.clone(), int(10)), add(d.clone(), int(48)), add(d, int(87)));
            Term::ite(Term::bin(Op::Lt, int(p), count.clone()), ch, int(0))
        })
        .collect();
    Text { len: count, bytes }
}

/// Ist der Text gueltiges UTF-8 (Unicode 3.9, Tabelle 3-7)? Ein Zustand je
/// Byte: bereit, noch 1 bis 3 Folgebytes, die vier Bereiche mit enger
/// erstem Folgebyte (nach `E0`, `ED`, `F0`, `F4`), Fehler.
pub(super) fn utf8(t: &Text) -> Term {
    const READY: i64 = 0;
    const AFTER_E0: i64 = 4;
    const AFTER_ED: i64 = 5;
    const AFTER_F0: i64 = 6;
    const AFTER_F4: i64 = 7;
    const BAD: i64 = 8;
    let mut state = int(READY);
    for (i, b) in t.bytes.iter().enumerate() {
        let is = |s: i64| Term::eq(state.clone(), int(s));
        let cont = within(b, 0x80, 0xBF);
        let lead = Term::ite(
            Term::bin(Op::Lt, b.clone(), int(0x80)),
            int(READY),
            Term::ite(
                within(b, 0xC2, 0xDF),
                int(1),
                Term::ite(
                    Term::eq(b.clone(), int(0xE0)),
                    int(AFTER_E0),
                    Term::ite(
                        Term::or(vec![within(b, 0xE1, 0xEC), within(b, 0xEE, 0xEF)]),
                        int(2),
                        Term::ite(
                            Term::eq(b.clone(), int(0xED)),
                            int(AFTER_ED),
                            Term::ite(
                                Term::eq(b.clone(), int(0xF0)),
                                int(AFTER_F0),
                                Term::ite(
                                    within(b, 0xF1, 0xF3),
                                    int(3),
                                    Term::ite(Term::eq(b.clone(), int(0xF4)), int(AFTER_F4), int(BAD)),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
        );
        let step = |ok: Term, then: i64| Term::ite(ok, int(then), int(BAD));
        let next = Term::ite(
            is(READY),
            lead,
            Term::ite(
                Term::or(vec![is(1), is(2), is(3)]),
                Term::ite(cont.clone(), sub(state.clone(), int(1)), int(BAD)),
                Term::ite(
                    is(AFTER_E0),
                    step(within(b, 0xA0, 0xBF), 1),
                    Term::ite(
                        is(AFTER_ED),
                        step(within(b, 0x80, 0x9F), 1),
                        Term::ite(
                            is(AFTER_F0),
                            step(within(b, 0x90, 0xBF), 2),
                            Term::ite(is(AFTER_F4), step(within(b, 0x80, 0x8F), 2), int(BAD)),
                        ),
                    ),
                ),
            ),
        );
        state = Term::ite(Term::bin(Op::Lt, int(i as i64), t.len.clone()), next, state);
    }
    Term::eq(state, int(READY))
}
