//! Ausgabestroeme im Modell (M11 Schritt 27c-3, Referenz 8.8, 8.3). Der
//! Sendepuffer ist ein Ring aus Bytes mit Kopf und Laenge
//! (`s.tx.<o>.head`, `.len`, `.q[i]`), dazu der Ausschnitt, den der Treiber
//! beim letzten Commit abgeholt hat (`.sent.len`, `.sent[j]`). `send` legt
//! die Bytes des Werts an, wenn sie in den freien Platz passen; am Ende
//! jedes Ticks holt der Treiber `max_rate · T0` Bytes ab, ohne `max_rate`
//! alles. Ein `sim`-gebundener Eingabestrom liest den Ausschnitt im
//! naechsten Tick als seine Elemente.
//!
//! Stellt ein Modell den Lesekanal `mmio/ADR/r` eines Ports als Strom, ist
//! der Port sein Treiber (12.10, `TxBuffer::port`): Jedes Lesen entnimmt
//! ein Element, das vor dem Tick im Puffer stand, und der Commit holt ab,
//! was gelesen wurde. Das zuletzt entnommene Element steht im Zustand
//! (`.last`).
//!
//! Speist der Strom einen Eingabestrom mit Elementen verschiedener Laenge
//! (`Program::keeps_elements`), ist jedes `send` ein Element (8.3); was ein
//! Commit vollendet, steht als `.el[j]` mit seiner Zahl `.el.n` im Zustand.
//! Holt der Treiber alles ab, sind das genau die `send` des Ticks,
//! hoechstens so viele, wie eine Aktivierung senden kann
//! (`analysis::sends`). Sonst warten Elemente ueber den Commit hinaus: ihre
//! Laengen im Ring `.el.lens[j]` ab `.el.lhead`, ihre Zahl `.el.cnt`, und
//! was vom ersten schon abgeholt ist, im Uebertrag `.el.carry`.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Accessor, Expr, ExprKind, StreamRef};
use takt_mir::machine::FaultKind;
use takt_mir::program::{Direction, Overflow};
use takt_mir::types::Type;
use takt_mir::{ChannelId, TypeId};

use super::text::Text;
use super::value::{Shape, V};
use super::{Cx, Enc, Env, Exit, ExitKind, Flow, R, no};
use crate::term::{Node, Op, Term};

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
    /// Ist er das Modell eines Ports: die Bytes eines Elements.
    port: Option<u32>,
    /// Behaelt er die Grenzen seiner Elemente?
    keep: Option<Keep>,
}

/// Ein Sendepuffer, der die Grenzen seiner Elemente behaelt (8.3).
#[derive(Clone, Copy, Debug)]
struct Keep {
    /// Plaetze `.el[j]` fuer die Elemente, die ein Commit vollendet.
    slots: u32,
    /// Bytes eines Elements.
    elem: u32,
    /// Warten Elemente ueber den Commit hinaus, weil der Treiber nicht je
    /// Tick alles abholt?
    queued: bool,
}

/// Was das Lesen eines Ports im laufenden Tick aus seinem Strommodell nahm
/// (12.10).
#[derive(Clone, Debug)]
pub(super) struct PortTake {
    /// Bytes, die vor dem Tick im Puffer standen.
    visible: Term,
    /// Bytes, die das Lesen dieses Ticks entnommen hat.
    taken: Term,
    /// Das zuletzt entnommene Element und seine Gestalt.
    last: V,
    shape: Shape,
}

impl Tx {
    /// Behaelt der Puffer die Grenzen seiner Elemente, und passt jedes in
    /// ein Element der Kapazitaet `cap`?
    pub(super) fn kept_fits(&self, cap: u32) -> bool {
        self.keep.is_some_and(|k| k.elem <= cap)
    }

    /// Holt der Treiber jeden Commit alles ab? Dann steht der Kopf immer
    /// bei null, und das Modell fuehrt ihn nicht. Ein Port holt nur ab, was
    /// er las.
    fn empties(&self) -> bool {
        self.port.is_none() && self.per_tick >= self.cap
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

/// Der Eintrag `pos` einer Folge, ausserhalb null; steht `pos` fest, ohne
/// Auswahl.
fn pick(items: &[Term], pos: &Term) -> Term {
    match &*pos.0 {
        Node::Int(i) => usize::try_from(*i).ok().and_then(|i| items.get(i)).cloned().unwrap_or_else(|| int(0)),
        _ => items
            .iter()
            .enumerate()
            .rev()
            .fold(int(0), |acc, (i, x)| Term::ite(Term::eq(pos.clone(), int(i as i64)), x.clone(), acc)),
    }
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
            let port = self.port_model(ChannelId(i as u32)).then(|| takt_mir::bytes::max_size(self.p, *elem));
            let port = match port {
                Some(Ok(size)) => Some(size),
                Some(Err(_)) => return no("Port, dessen Strommodell keine feste Byteform hat", c.span),
                None => None,
            };
            let per_tick = if port.is_some() { cap } else { per_tick(c, self.p.config.tick).min(cap) };
            let keep = if self.p.keeps_elements(ChannelId(i as u32)) {
                let (Type::Line { cap: n } | Type::Str { cap: n } | Type::Bytes { cap: n }) = self.p.types.get(*elem)
                else {
                    return no("Ausgabestrom, dessen Elemente keine Textgestalt haben", c.span);
                };
                // Ein Commit vollendet hoechstens die Elemente im Puffer: ohne
                // Warteschlange die `send` seines Ticks, mit ihr bis zu
                // `capacity`, auch leere.
                let queued = per_tick < cap;
                let slots = if queued {
                    self.model_budget(u64::from(cap) * u64::from(*n) * (u64::from(per_tick) + 4), c.span)?;
                    cap
                } else {
                    let sends = takt_mir::analysis::sends::max_sends(self.p, StreamRef::Channel(ChannelId(i as u32)));
                    u32::try_from(sends.min(u64::from(cap))).unwrap_or(cap)
                };
                Some(Keep { slots, elem: *n, queued })
            } else {
                None
            };
            out.push(Tx { channel: ChannelId(i as u32), name: c.name.clone(), elem: *elem, cap, per_tick, port, keep });
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

    /// Leere Puffer vor dem ersten Tick; vor dem ersten Lesen liest ein
    /// Port den Default.
    pub(super) fn tx_initial(&self, env: &mut Env) -> R<()> {
        for t in self.txs.iter().filter(|t| t.port.is_some()) {
            let shape = self.shape(t.elem, Span::default())?;
            Enc::store(env, &loc(t, "last"), &shape, self.zero_value(&shape, Span::default())?);
        }
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
            if let Some(k) = t.keep {
                env.insert(loc(t, "el.n"), int(0));
                for j in 0..k.slots {
                    env.insert(loc(t, &format!("el[{j}].len")), int(0));
                    for i in 0..k.elem {
                        env.insert(loc(t, &format!("el[{j}][{i}]")), int(0));
                    }
                }
                if k.queued {
                    for part in ["el.cnt", "el.lhead", "el.carry.n"] {
                        env.insert(loc(t, part), int(0));
                    }
                    for j in 0..t.cap {
                        env.insert(loc(t, &format!("el.lens[{j}]")), int(0));
                    }
                    for i in 0..k.elem {
                        env.insert(loc(t, &format!("el.carry[{i}]")), int(0));
                    }
                }
            }
        }
        Ok(())
    }

    /// Zu Beginn eines Ticks: Was jetzt im Puffer eines Strommodells steht,
    /// kann ein Port in diesem Tick lesen; gelesen ist noch nichts, und kein
    /// Element ist gesendet.
    pub(super) fn tx_begin(&mut self, env: &Env) -> R<()> {
        self.port_takes.clear();
        self.tx_elements.clear();
        for t in self.txs.clone().into_iter().filter(|t| t.port.is_some()) {
            let shape = self.shape(t.elem, Span::default())?;
            let last = self.load(env, &loc(&t, "last"), &shape, Span::default())?;
            let take = PortTake { visible: env[&loc(&t, "len")].clone(), taken: int(0), last, shape };
            self.port_takes.insert(t.channel, take);
        }
        Ok(())
    }

    /// Ein Lesen des Ports, dessen Modell der Strom `c` ist, wo `alive`
    /// gilt: das naechste Element, das vor dem Tick im Puffer stand, sonst
    /// das zuletzt entnommene (`Image::port_next`).
    pub(super) fn port_next(&mut self, c: ChannelId, env: &Env, alive: &Term, span: Span) -> R<V> {
        let t = self.tx_of(c, span)?;
        let (Some(size), Some(take)) = (t.port, self.port_takes.get(&c).cloned()) else {
            return no("Strommodell eines Ports ausserhalb eines Ticks", span);
        };
        let at = add(t.head(env), take.taken.clone());
        let bytes: Vec<Term> =
            (0..size).map(|j| self.tx_byte(&t, wrap(add(at.clone(), int(i64::from(j))), t.cap), env)).collect();
        let read = self.read_canonical(t.elem, &bytes, &int(0), span)?;
        let after = add(take.taken.clone(), int(i64::from(size)));
        let fresh = Term::and(vec![alive.clone(), Term::bin(Op::Le, after.clone(), take.visible.clone())]);
        let value = V::ite(&fresh, read.value, take.last);
        let take = PortTake { taken: Term::ite(fresh, after, take.taken), last: value.clone(), ..take };
        self.port_takes.insert(c, take);
        Ok(value)
    }

    /// Das Byte am Platz `pos` des Rings; steht der Platz fest, ohne Auswahl.
    fn tx_byte(&self, t: &Tx, pos: Term, env: &Env) -> Term {
        match &*pos.0 {
            Node::Int(i) => env[&loc(t, &format!("q[{i}]"))].clone(),
            _ => (0..t.cap).rev().fold(int(0), |byte, i| {
                Term::ite(Term::eq(pos.clone(), int(i64::from(i))), env[&loc(t, &format!("q[{i}]"))].clone(), byte)
            }),
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
                    bytes.extend(self.buffer_bytes(item, elem, span)?);
                }
                Ok(Text { len: int(bytes.len() as i64), bytes })
            }
            _ => {
                let v = self.value(value, cx, env, flow)?;
                let bytes = self.buffer_bytes(v, elem, span)?;
                Ok(Text { len: int(bytes.len() as i64), bytes })
            }
        }
    }

    /// Ein Element in der Form des Sendepuffers (`element_bytes`): seine
    /// kanonische Form, mit Nullen auf `max_size` gefuellt, damit der Leser
    /// die Grenzen findet (FB-189).
    fn buffer_bytes(&self, v: V, elem: TypeId, span: Span) -> R<Vec<Term>> {
        let Ok(size) = takt_mir::bytes::max_size(self.p, elem) else { return no("Element ohne Byteform", span) };
        let mut bytes = self.canonical(elem, v, span)?.bytes;
        bytes.resize(size as usize, int(0));
        Ok(bytes)
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
        let mut full = Term::bin(Op::Gt, data.len.clone(), free);
        // Ein Puffer mit Grenzen fasst hoechstens `capacity` Elemente; ohne
        // Warteschlange hat ihn der Treiber zu Tickbeginn geleert.
        if let Some(k) = t.keep {
            let count = if k.queued {
                env[&loc(&t, "el.cnt")].clone()
            } else {
                self.tx_elements
                    .get(&c)
                    .into_iter()
                    .flatten()
                    .fold(int(0), |n, (_, alive)| add(n, Term::ite(alive.clone(), int(1), int(0))))
            };
            full = Term::or(vec![full, Term::bin(Op::Ge, count, int(i64::from(t.cap)))]);
        }
        let drop = matches!(self.p.channels[c.index()].attrs.overflow, Some(Overflow::Drop));
        let fits = Term::and(vec![flow.alive.clone(), full.clone().not()]);
        if !drop {
            let cause = self.cause(FaultKind::StreamOverflow, span);
            flow.exits
                .push(Exit { cond: Term::and(vec![flow.alive.clone(), full]), kind: ExitKind::Fault(None, cause) });
            flow.alive = fits.clone();
        }
        let tail = wrap(add(head, len.clone()), t.cap);
        self.model_budget(u64::from(t.cap) * (3 * data.bytes.len() as u64 + 8), span)?;
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
        match t.keep {
            // Die Laenge des neuen Elements hinter die wartenden.
            Some(k) if k.queued => {
                let cnt = env[&loc(&t, "el.cnt")].clone();
                let at = wrap(add(env[&loc(&t, "el.lhead")].clone(), cnt.clone()), t.cap);
                for j in 0..t.cap {
                    let here = Term::and(vec![fits.clone(), Term::eq(at.clone(), int(i64::from(j)))]);
                    let slot = loc(&t, &format!("el.lens[{j}]"));
                    let old = env[&slot].clone();
                    env.insert(slot, Term::ite(here, data.len.clone(), old));
                }
                env.insert(loc(&t, "el.cnt"), Term::ite(fits.clone(), add(cnt.clone(), int(1)), cnt));
            }
            Some(_) => self.tx_elements.entry(c).or_default().push((data.clone(), fits.clone())),
            None => {}
        }
        env.insert(loc(&t, "len"), Term::ite(fits, add(len.clone(), data.len), len));
        Ok(())
    }

    /// Der Treiber holt ab (`TxBuffer::drain`): bis zu `per_tick` Bytes vom
    /// Kopf, im Tick 0 wie in jedem anderen; an einem Port, was sein Lesen
    /// entnahm. Steht der Kopf fest, liest jedes Byte seinen Platz ohne
    /// Auswahl ueber den Ring.
    pub(super) fn drain_tx(&self, env: &mut Env) -> R<()> {
        for t in &self.txs {
            let (head, len) = (t.head(env), env[&loc(t, "len")].clone());
            let take = self.port_takes.get(&t.channel);
            let n = match take {
                Some(take) => take.taken.clone(),
                None => {
                    let per = int(i64::from(t.per_tick));
                    Term::ite(Term::bin(Op::Lt, len.clone(), per.clone()), len.clone(), per)
                }
            };
            if let Some(take) = take {
                Enc::store(env, &loc(t, "last"), &take.shape, take.last.clone());
            }
            for j in 0..t.per_tick {
                let byte = self.tx_byte(t, wrap(add(head.clone(), int(i64::from(j))), t.cap), env);
                let taken = Term::bin(Op::Lt, int(i64::from(j)), n.clone());
                env.insert(loc(t, &format!("sent[{j}]")), Term::ite(taken, byte, int(0)));
            }
            env.insert(loc(t, "sent.len"), n.clone());
            match t.keep {
                Some(k) if k.queued => self.drain_queued(t, k, &n, env)?,
                Some(k) => self.complete_elements(t, k, env),
                None => {}
            }
            if !t.empties() {
                env.insert(loc(t, "head"), wrap(add(head, n.clone()), t.cap));
            }
            env.insert(loc(t, "len"), sub(len, n));
        }
        Ok(())
    }

    /// Die Elemente, die der Commit vollendet, wenn sie ueber Commits warten
    /// (`TxBuffer::drain`): Ein Element ist da, wenn sein letztes Byte
    /// abgeholt ist, ein leeres, sobald es vorn steht; vom ersten
    /// unvollendeten kommt das Abgeholte in den Uebertrag. `n` Bytes hat der
    /// Commit abgeholt, sie stehen in `.sent[j]`.
    fn drain_queued(&self, t: &Tx, k: Keep, n: &Term, env: &mut Env) -> R<()> {
        self.model_budget(u64::from(k.slots) * u64::from(k.elem) * (u64::from(t.per_tick) + 4), Span::default())?;
        let at = |part: &str| loc(t, part);
        let taken: Vec<Term> = (0..t.per_tick).map(|j| env[&at(&format!("sent[{j}]"))].clone()).collect();
        let lens: Vec<Term> = (0..t.cap).map(|j| env[&at(&format!("el.lens[{j}]"))].clone()).collect();
        let (cnt, lhead) = (env[&at("el.cnt")].clone(), env[&at("el.lhead")].clone());
        let mut carry_n = env[&at("el.carry.n")].clone();
        let mut carry: Vec<Term> = (0..k.elem).map(|i| env[&at(&format!("el.carry[{i}]"))].clone()).collect();
        // `off`: abgeholte Bytes, die schon einem vollendeten Element gehoeren;
        // `open`: Noch blieb kein Element unvollendet.
        let (mut off, mut open, mut done) = (int(0), Term::bool(true), int(0));
        for j in 0..k.slots {
            let waiting = Term::and(vec![open.clone(), Term::bin(Op::Lt, int(i64::from(j)), cnt.clone())]);
            let len = pick(&lens, &wrap(add(lhead.clone(), int(i64::from(j))), t.cap));
            let rest = sub(n.clone(), off.clone());
            let whole =
                Term::and(vec![waiting.clone(), Term::bin(Op::Le, sub(len.clone(), carry_n.clone()), rest.clone())]);
            let partial = Term::and(vec![waiting, whole.clone().not()]);
            // Das Element: der Uebertrag, dann das Abgeholte ab `off`.
            let bytes: Vec<Term> = (0..k.elem)
                .map(|i| {
                    let from_carry = Term::bin(Op::Lt, int(i64::from(i)), carry_n.clone());
                    let fresh = pick(&taken, &sub(add(off.clone(), int(i64::from(i))), carry_n.clone()));
                    Term::ite(from_carry, carry[i as usize].clone(), fresh)
                })
                .collect();
            let slot = |part: &str| at(&format!("el[{j}]{part}"));
            env.insert(slot(".len"), Term::ite(whole.clone(), len.clone(), int(0)));
            for (i, b) in bytes.iter().enumerate() {
                let inside = Term::and(vec![whole.clone(), Term::bin(Op::Lt, int(i as i64), len.clone())]);
                env.insert(slot(&format!("[{i}]")), Term::ite(inside, b.clone(), int(0)));
            }
            // Bleibt es unvollendet, kommt der Rest in den Uebertrag; ein
            // vollendetes leert ihn.
            let kept = add(carry_n.clone(), rest);
            carry = bytes
                .iter()
                .zip(&carry)
                .enumerate()
                .map(|(i, (b, c))| {
                    let grown = Term::and(vec![partial.clone(), Term::bin(Op::Lt, int(i as i64), kept.clone())]);
                    Term::ite(whole.clone(), int(0), Term::ite(grown, b.clone(), c.clone()))
                })
                .collect();
            off = Term::ite(whole.clone(), add(off.clone(), sub(len, carry_n.clone())), off);
            carry_n = Term::ite(whole.clone(), int(0), Term::ite(partial.clone(), kept, carry_n));
            done = add(done, Term::ite(whole, int(1), int(0)));
            open = Term::and(vec![open, partial.not()]);
        }
        env.insert(at("el.n"), done.clone());
        env.insert(at("el.lhead"), wrap(add(lhead, done.clone()), t.cap));
        env.insert(at("el.cnt"), sub(cnt, done));
        env.insert(at("el.carry.n"), carry_n);
        for (i, c) in carry.into_iter().enumerate() {
            env.insert(at(&format!("el.carry[{i}]")), c);
        }
        Ok(())
    }

    /// Die Elemente, die der Commit vollendet: die `send` des Ticks, die ihr
    /// Element anlegten, in ihrer Reihenfolge auf die Plaetze `.el[j]`.
    fn complete_elements(&self, t: &Tx, k: Keep, env: &mut Env) {
        let (slots, cap) = (k.slots, k.elem);
        let records = self.tx_elements.get(&t.channel).cloned().unwrap_or_default();
        let mut placed: Vec<(Term, Vec<Term>)> = (0..slots).map(|_| (int(0), vec![int(0); cap as usize])).collect();
        let mut before = int(0);
        for (text, alive) in records {
            for (j, (len, bytes)) in placed.iter_mut().enumerate() {
                let here = Term::and(vec![alive.clone(), Term::eq(before.clone(), int(j as i64))]);
                *len = Term::ite(here.clone(), text.len.clone(), len.clone());
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = Term::ite(here.clone(), text.bytes.get(i).cloned().unwrap_or_else(|| int(0)), b.clone());
                }
            }
            before = add(before, Term::ite(alive, int(1), int(0)));
        }
        env.insert(loc(t, "el.n"), before);
        for (j, (len, bytes)) in placed.into_iter().enumerate() {
            env.insert(loc(t, &format!("el[{j}].len")), len);
            for (i, b) in bytes.into_iter().enumerate() {
                env.insert(loc(t, &format!("el[{j}][{i}]")), b);
            }
        }
    }

    /// Die Elemente, die der letzte Commit vollendete, als Texte mit der
    /// Bedingung, dass es sie gibt; `None` fuer einen Puffer ohne Grenzen.
    pub(super) fn done_elements(&self, t: &Tx, env: &Env) -> Option<Vec<(Term, Text)>> {
        let Keep { slots, elem: cap, .. } = t.keep?;
        let n = env[&loc(t, "el.n")].clone();
        Some(
            (0..slots)
                .map(|j| {
                    let present = Term::bin(Op::Lt, int(i64::from(j)), n.clone());
                    let len = env[&loc(t, &format!("el[{j}].len"))].clone();
                    let bytes = (0..cap).map(|i| env[&loc(t, &format!("el[{j}][{i}]"))].clone()).collect();
                    (present, Text { len, bytes })
                })
                .collect(),
        )
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
                // 8.8: auch wenn er einen `sim`-gebundenen Eingabestrom speist.
                let sent = self.sent_text(&t, env);
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
            if t.port.is_some() {
                self.typed_loc(pre, &loc(t, "last"), t.elem, out);
            }
            out.push(within(pre[&loc(t, "len")].clone(), i64::from(t.cap)));
            if !t.empties() {
                out.push(within(pre[&loc(t, "head")].clone(), i64::from(t.cap.max(1) - 1)));
            }
            out.push(within(pre[&loc(t, "sent.len")].clone(), i64::from(t.per_tick)));
            if let Some(k) = t.keep {
                out.push(within(pre[&loc(t, "el.n")].clone(), i64::from(k.slots)));
                for j in 0..k.slots {
                    out.push(within(pre[&loc(t, &format!("el[{j}].len"))].clone(), i64::from(k.elem)));
                }
                if k.queued {
                    out.push(within(pre[&loc(t, "el.cnt")].clone(), i64::from(t.cap)));
                    out.push(within(pre[&loc(t, "el.lhead")].clone(), i64::from(t.cap.max(1) - 1)));
                    out.push(within(pre[&loc(t, "el.carry.n")].clone(), i64::from(k.elem)));
                    for j in 0..t.cap {
                        out.push(within(pre[&loc(t, &format!("el.lens[{j}]"))].clone(), i64::from(k.elem)));
                    }
                }
            }
        }
    }
}
