//! Ausgabestroeme im Modell (M11 Schritt 27c-3, Referenz 8.8, 8.3). Der
//! Sendepuffer ist ein Ring aus Bytes mit Kopf und Laenge
//! (`s.tx.<o>.head`, `.len`, `.q[i]`), dazu der Ausschnitt, den der Treiber
//! beim letzten Commit abgeholt hat (`.sent.len`, `.sent[j]`). `send` legt
//! die Bytes des Werts an, wenn sie in den freien Platz passen; am Ende
//! jedes Ticks holt der Treiber `max_rate · T0` Bytes ab, ohne `max_rate`
//! alles. Ein `sim`-gebundener Eingabestrom liest den Ausschnitt im
//! naechsten Tick als seine Elemente.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Accessor, Expr, ExprKind};
use takt_mir::program::{Binding, Direction, Overflow};
use takt_mir::types::Type;
use takt_mir::{ChannelId, TypeId};

use super::text::Text;
use super::value::V;
use super::{Cx, Enc, Env, Exit, ExitKind, Flow, R, no};
use crate::term::{Op, Term};

/// Ein Ausgabestrom, den eine kodierte Maschine schreibt.
#[derive(Clone, Debug)]
pub(super) struct Tx {
    pub(super) channel: ChannelId,
    name: String,
    elem: TypeId,
    /// Plaetze des Sendepuffers in Bytes.
    cap: u32,
    /// Was der Treiber je Commit hoechstens abholt, gekappt durch den Puffer.
    per_tick: u32,
    /// Speist er einen `sim`-gebundenen Eingabestrom? Dann nimmt der Rand
    /// den Ausschnitt zu Tick-Beginn, und `.sent` ist im Schritt leer.
    pub(super) feeds: bool,
}

impl Tx {
    /// Wie viele Bytes ein Commit hoechstens abholt.
    pub(super) fn per_tick_bytes(&self) -> u32 {
        self.per_tick
    }

    /// Holt der Treiber jeden Commit alles ab? Dann steht der Kopf immer
    /// bei null, und das Modell fuehrt ihn nicht.
    fn empties(&self) -> bool {
        self.per_tick >= self.cap
    }

    fn head(&self, env: &Env) -> Term {
        if self.empties() { int(0) } else { env[&loc(self, "head")].clone() }
    }
}

fn loc(t: &Tx, part: &str) -> String {
    format!("s.tx.{}.{part}", t.name)
}

fn int(i: i64) -> Term {
    Term::int(i)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

fn sub(a: Term, b: Term) -> Term {
    Term::bin(Op::Sub, a, b)
}

/// `x` im Ring der Groesse `cap`, wenn `0 <= x < 2 cap`.
fn wrap(x: Term, cap: u32) -> Term {
    let cap = int(i64::from(cap));
    Term::ite(Term::bin(Op::Ge, x.clone(), cap.clone()), sub(x.clone(), cap), x)
}

/// Die Bytes je Commit (`TxBuffer::per_tick`): `max_rate · T0`, mindestens
/// eins; ohne `max_rate` alles.
fn per_tick(c: &takt_mir::program::Channel, tick: i64) -> u32 {
    let hz = match c.attrs.max_rate.as_ref().map(|e| &e.kind) {
        Some(takt_mir::expr::ExprKind::Int(n)) => u64::try_from(*n).ok(),
        Some(takt_mir::expr::ExprKind::Float(f)) if *f >= 0.0 => Some(*f as u64),
        _ => None,
    };
    match hz {
        Some(hz) => u32::try_from((hz.saturating_mul(tick.max(0) as u64) / 1_000_000_000).max(1)).unwrap_or(u32::MAX),
        None => u32::MAX,
    }
}

impl Enc<'_> {
    /// Die Ausgabestroeme der kodierten Maschinen.
    pub(super) fn tx_defs(&self) -> R<Vec<Tx>> {
        let mut out = Vec::new();
        for (i, c) in self.p.channels.iter().enumerate() {
            let Type::Stream(elem) = self.p.types.get(c.ty) else { continue };
            if c.dir != Direction::Output || c.owner.is_some_and(|o| !self.order.contains(&o)) {
                continue;
            }
            let cap = c.attrs.capacity.unwrap_or(256);
            let feeds = self.p.channels.iter().any(|inp| {
                inp.dir == Direction::Input
                    && matches!((&c.binding, &inp.binding), (Binding::Sim(a), Binding::Hw(b)) if a == b)
            });
            out.push(Tx {
                channel: ChannelId(i as u32),
                name: c.name.clone(),
                elem: *elem,
                cap,
                per_tick: per_tick(c, self.p.config.tick).min(cap),
                feeds,
            });
        }
        Ok(out)
    }

    /// Der Ausgabestrom eines Ausdrucks: Ein Strom steht immer als
    /// `Input { channel }` da (8.6), die Richtung traegt der Channel.
    pub(super) fn tx_channel(&self, base: &Expr) -> Option<ChannelId> {
        match &base.kind {
            ExprKind::Input { channel, .. } | ExprKind::Output(channel)
                if self.p.channels[channel.index()].dir == Direction::Output =>
            {
                Some(*channel)
            }
            _ => None,
        }
    }

    pub(super) fn tx_of(&self, c: ChannelId, span: Span) -> R<Tx> {
        match self.txs.iter().find(|t| t.channel == c) {
            Some(t) => Ok(t.clone()),
            None => no("Ausgabestrom einer anderen Maschine", span),
        }
    }

    /// Leere Puffer vor dem ersten Tick.
    pub(super) fn tx_initial(&self, env: &mut Env) {
        for t in &self.txs {
            for part in ["len", "sent.len"] {
                env.insert(loc(t, part), int(0));
            }
            if !t.empties() {
                env.insert(loc(t, "head"), int(0));
            }
            for i in 0..t.cap {
                env.insert(loc(t, &format!("q[{i}]")), int(0));
            }
            for j in 0..t.per_tick {
                env.insert(loc(t, &format!("sent[{j}]")), int(0));
            }
        }
    }

    /// Die Bytes, die `send` fuer einen Wert anlegt (`element_bytes`): Text
    /// und Bytes wie sie sind, ein Array Element fuer Element, eine
    /// Ganzzahl oder ein Wahrheitswert in der kanonischen Form des
    /// Elementtyps (5.9: little-endian in ihrer Breite).
    fn send_bytes(&mut self, value: &Expr, elem: TypeId, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Text> {
        let span = value.span;
        match self.p.types.get(value.ty).clone() {
            Type::Bytes { .. } | Type::Str { .. } | Type::Line { .. } => {
                Text::of(self.value(value, cx, env, flow)?, span)
            }
            Type::Array { .. } => {
                let V::Node(items) = self.value(value, cx, env, flow)? else { return no("Array", span) };
                let mut bytes = Vec::new();
                for item in items {
                    bytes.extend(self.scalar_bytes(item.leaf(span)?, elem, span)?);
                }
                Ok(Text { len: int(bytes.len() as i64), bytes })
            }
            _ => {
                let x = self.expr(value, cx, env, flow)?;
                let bytes = self.scalar_bytes(x, elem, span)?;
                Ok(Text { len: int(bytes.len() as i64), bytes })
            }
        }
    }

    /// Ein Skalar in der kanonischen Form des Elementtyps.
    fn scalar_bytes(&self, x: Term, elem: TypeId, span: Span) -> R<Vec<Term>> {
        let width = match self.p.types.get(elem) {
            Type::Int { width, .. } => width.bits() / 8,
            Type::Bool => return Ok(vec![Term::ite(x, int(1), int(0))]),
            _ => return no("`send` eines Werts dieser Art auf einen Ausgabestrom", span),
        };
        Ok((0..width)
            .map(|k| Term::bin(Op::BitAnd, Term::bin(Op::Shr, x.clone(), int(i64::from(8 * k))), int(0xFF)))
            .collect())
    }

    /// `send o, e` auf einen Ausgabestrom (8.8): Passt der Wert nicht in den
    /// freien Platz, ist das ein `StreamOverflow`, unter `overflow = drop`
    /// faellt er weg.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_tx(
        &mut self,
        c: ChannelId,
        value: &Expr,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let t = self.tx_of(c, span)?;
        let data = self.send_bytes(value, t.elem, cx, env, flow)?;
        let (head, len) = (t.head(env), env[&loc(&t, "len")].clone());
        let free = sub(int(i64::from(t.cap)), len.clone());
        let full = Term::bin(Op::Gt, data.len.clone(), free);
        let drop = matches!(self.p.channels[c.index()].attrs.overflow, Some(Overflow::Drop));
        let fits = Term::and(vec![flow.alive.clone(), full.clone().not()]);
        if !drop {
            flow.exits.push(Exit { cond: Term::and(vec![flow.alive.clone(), full]), kind: ExitKind::Fault(None) });
            flow.alive = fits.clone();
        }
        let tail = wrap(add(head, len.clone()), t.cap);
        for i in 0..t.cap {
            // Abstand des Platzes hinter dem Ende des Puffers.
            let d = sub(int(i64::from(i)), tail.clone());
            let d = Term::ite(Term::bin(Op::Lt, d.clone(), int(0)), add(d.clone(), int(i64::from(t.cap))), d);
            let inside = Term::and(vec![fits.clone(), Term::bin(Op::Lt, d.clone(), data.len.clone())]);
            let mut byte = int(0);
            for (k, b) in data.bytes.iter().enumerate().rev() {
                byte = Term::ite(Term::eq(d.clone(), int(k as i64)), b.clone(), byte);
            }
            let at = loc(&t, &format!("q[{i}]"));
            let old = env[&at].clone();
            env.insert(at, Term::ite(inside, byte, old));
        }
        env.insert(loc(&t, "len"), Term::ite(fits, add(len.clone(), data.len), len));
        Ok(())
    }

    /// Der Treiber holt ab (`TxBuffer::drain`): bis zu `per_tick` Bytes vom
    /// Kopf, im Tick 0 wie in jedem anderen.
    pub(super) fn drain_tx(&self, env: &mut Env) {
        for t in &self.txs {
            let (head, len) = (t.head(env), env[&loc(t, "len")].clone());
            let per = int(i64::from(t.per_tick));
            let n = Term::ite(Term::bin(Op::Lt, len.clone(), per.clone()), len.clone(), per);
            for j in 0..t.per_tick {
                let pos = wrap(add(head.clone(), int(i64::from(j))), t.cap);
                let mut byte = int(0);
                for i in (0..t.cap).rev() {
                    byte = Term::ite(
                        Term::eq(pos.clone(), int(i64::from(i))),
                        env[&loc(t, &format!("q[{i}]"))].clone(),
                        byte,
                    );
                }
                let taken = Term::bin(Op::Lt, int(i64::from(j)), n.clone());
                env.insert(loc(t, &format!("sent[{j}]")), Term::ite(taken, byte, int(0)));
            }
            env.insert(loc(t, "sent.len"), n.clone());
            if !t.empties() {
                env.insert(loc(t, "head"), wrap(add(head, n.clone()), t.cap));
            }
            env.insert(loc(t, "len"), sub(len, n));
        }
    }

    /// Der Ausschnitt des letzten Commits als Text.
    pub(super) fn sent_text(&self, t: &Tx, env: &Env) -> Text {
        Text {
            len: env[&loc(t, "sent.len")].clone(),
            bytes: (0..t.per_tick).map(|j| env[&loc(t, &format!("sent[{j}]"))].clone()).collect(),
        }
    }

    /// `o.free`, `o.idle` und `o.sent` (8.8): der freie Platz jetzt, ob der
    /// Puffer zu Tick-Beginn leer war, was der Treiber zuletzt abholte.
    pub(super) fn tx_accessor(
        &mut self,
        c: ChannelId,
        acc: Accessor,
        ty: TypeId,
        cx: &Cx<'_>,
        env: &Env,
        span: Span,
    ) -> R<V> {
        let t = self.tx_of(c, span)?;
        Ok(match acc {
            Accessor::Free => V::Leaf(sub(int(i64::from(t.cap)), env[&loc(&t, "len")].clone())),
            Accessor::Idle => V::Leaf(Term::eq(cx.pre[&loc(&t, "len")].clone(), int(0))),
            Accessor::Sent => {
                let Type::Optional(inner) = self.p.types.get(ty) else { return no("`.sent`", span) };
                let Type::Bytes { cap } = self.p.types.get(*inner) else { return no("`.sent`", span) };
                // Ein `sim`-gebundener Eingabestrom hat den Ausschnitt schon genommen.
                let sent = if t.feeds { Text { len: int(0), bytes: Vec::new() } } else { self.sent_text(&t, env) };
                let has = Term::bin(Op::Gt, sent.len.clone(), int(0));
                V::Node(vec![V::Leaf(has), sent.value(*cap, None)])
            }
            other => return no(format!("Zugriff `.{}` auf einen Ausgabestrom", other.name()), span),
        })
    }

    /// Die Invarianten der Sendepuffer.
    pub(super) fn tx_invariants(&self, pre: &Env, out: &mut Vec<Term>) {
        let within =
            |x: Term, hi: i64| Term::and(vec![Term::bin(Op::Ge, x.clone(), int(0)), Term::bin(Op::Le, x, int(hi))]);
        for t in &self.txs {
            out.push(within(pre[&loc(t, "len")].clone(), i64::from(t.cap)));
            if !t.empties() {
                out.push(within(pre[&loc(t, "head")].clone(), i64::from(t.cap.max(1) - 1)));
            }
            out.push(within(pre[&loc(t, "sent.len")].clone(), i64::from(t.per_tick)));
        }
    }
}
