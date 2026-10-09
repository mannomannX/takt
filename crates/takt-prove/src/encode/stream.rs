//! Ereignisstroeme im Modell (M11 Schritt 27c, Referenz 8.6, 9.6, 9.7).
//!
//! Ein Strom ist ein Ring aus Plaetzen: die naechste Nummer, die Zahl der
//! Elemente und der Platz des aeltesten (`s.stream.<s>.next`, `.len`,
//! `.head`), je Platz Zeitstempel und Wert (`.buf[i].t`, `.buf[i].v…`),
//! dazu die Zaehler `dropped`, `overflowed` und `malformed`. Jeder Leser
//! haelt seinen Cursor (`s.<m>.cur.<i>`, 9.6). Das Fenster einer
//! Aktivierung steht nach dem Zustellen fest; was ein Konstrukt untersucht,
//! merkt die Kodierung als Marke, und am Tick-Ende ruecken die Cursor vor.
//! Ein `send` auf einen internen Strom kommt nach dem Verwerfen am Tick-Ende
//! in den Ring: Im Interpreter liegt es bis zum Zustellen des naechsten
//! Ticks in der Warteschlange, und dazwischen liest es niemand.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Accessor, Expr, ExprKind, MatchKind, StreamRef};
use takt_mir::machine::{FaultKind, Guard, Handler};
use takt_mir::pattern::Pattern;
use takt_mir::program::{Binding, Direction, Overflow};
use takt_mir::stmt::Block;
use takt_mir::types::Type;
use takt_mir::{MachineId, TypeId, VarId};

use super::text::Text;
use super::value::V;
use super::{Cx, Enc, Env, Exit, ExitKind, Flow, Mode, NOW, R, UNROLL_LIMIT, ite_env, no};
use crate::term::{Op, Sort, Term};

/// Ein Strom, wie das Modell ihn fuehrt.
#[derive(Clone, Debug)]
pub(super) struct Stream {
    key: StreamRef,
    name: String,
    elem: TypeId,
    /// Plaetze des Rings: die Kapazitaet in Elementen, gekappt durch die in
    /// Bytes (8.6) — ein Element fester Groesse belegt immer gleich viele.
    cap: u32,
    /// Die Schranke in Bytes, wenn die Elemente verschieden viele belegen.
    budget: Option<u32>,
    /// Was der Rand je Tick hoechstens liefert (`MAXPT`, 8.6); ein Strom
    /// eines Schreibers ausserhalb des Modells (13.3) liefert bis zur
    /// Kapazitaet, ein interner sonst nichts.
    maxpt: Option<u32>,
    overflow: Overflow,
    wake: bool,
    /// Vom Rand: ein Ueberlauf faultet die Leser beim naechsten Schritt.
    channel: bool,
    /// Ein Record-Strom vom Rand: Was sich nicht dekodieren laesst, zaehlt
    /// als `malformed` (8.6).
    decodes: bool,
    /// Die Leser unter den kodierten Maschinen: Maschine und Index ihres Cursors.
    readers: Vec<(MachineId, usize)>,
    /// Liest eine Maschine ausserhalb des Modells mit (13.3)?
    foreign_readers: bool,
    /// Der Ausgabestrom, der ihn ueber eine `sim`-Bindung speist (8.3).
    fed: Option<usize>,
}

/// Ein Platz des Fensters: ob er belegt ist, Nummer, Zeitstempel, Wert.
#[derive(Clone, Debug)]
pub(super) struct Item {
    present: Term,
    seq: Term,
    t: Term,
    value: V,
}

/// Das Fenster eines Lesers in einem Tick (9.6, `windows`).
#[derive(Clone, Debug)]
pub(super) struct Window {
    count: Term,
    /// Die Nummer hinter dem letzten Element.
    end: Term,
    items: Vec<Item>,
}

/// Ein untersuchtes Element (9.6, `examined`): Leser, Cursor, wo es gilt,
/// und seine Nummer.
#[derive(Clone, Debug)]
pub(super) struct Mark {
    m: MachineId,
    cursor: usize,
    cond: Term,
    seq: Term,
}

/// Ein `send` dieses Ticks auf einen internen Strom oder ein
/// Schreibvorgang auf einen Port (`port`), der seinen Ueberlauf erst beim
/// Zustellen zaehlt.
#[derive(Clone, Debug)]
pub(super) struct Queued {
    pub(super) stream: usize,
    pub(super) cond: Term,
    pub(super) t: Term,
    pub(super) value: V,
    pub(super) port: bool,
}

/// Was ein Guard ueber einem Strom ergibt (8.7): ob er feuert, die Bindung
/// und das untersuchte Element — beides gilt erst, wenn der Uebergang
/// genommen wird.
pub(super) struct GuardHit {
    pub(super) fired: Term,
    pub(super) bind: Option<(VarId, V)>,
    pub(super) mark: Option<(usize, Term)>,
}

/// Der Strom eines Ausdrucks: ein Channel mit Strom-Typ oder ein interner.
pub(super) fn stream_of(e: &Expr) -> Option<StreamRef> {
    match &e.kind {
        ExprKind::Input { channel, .. } | ExprKind::Output(channel) => Some(StreamRef::Channel(*channel)),
        ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
        _ => None,
    }
}

fn loc(s: &Stream, part: &str) -> String {
    format!("s.stream.{}.{part}", s.name)
}

fn slot(s: &Stream, i: u32) -> String {
    format!("s.stream.{}.buf[{i}]", s.name)
}

/// `x` im Ring: `x` liegt unter dem Doppelten der Kapazitaet.
fn wrap(x: Term, cap: u32) -> Term {
    let cap = Term::int(i64::from(cap));
    Term::ite(Term::bin(Op::Ge, x.clone(), cap.clone()), Term::bin(Op::Sub, x.clone(), cap), x)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

fn sub(a: Term, b: Term) -> Term {
    Term::bin(Op::Sub, a, b)
}

fn bump(env: &mut Env, at: &str, by: &Term) {
    let old = env[at].clone();
    env.insert(at.to_string(), Term::ite(by.clone(), add(old.clone(), Term::int(1)), old));
}

impl Enc<'_> {
    /// Die Stroeme des Modells. Elemente variabler Laenge, Ausgabestroeme
    /// mit Leser und Eingabestroeme aus einem `sim`-Ausgang lehnt es ab.
    pub(super) fn stream_defs(&self) -> R<Vec<Stream>> {
        let mut out = Vec::new();
        let runnable = takt_mir::analysis::schedule::runnable(self.p);
        let readers = |key: StreamRef| -> (Vec<(MachineId, usize)>, bool) {
            let mut own = Vec::new();
            let mut foreign = false;
            for &m in &runnable {
                for (i, r) in self.machine(m).layout.cursors.iter().enumerate() {
                    if *r != key {
                        continue;
                    }
                    if self.order.contains(&m) {
                        own.push((m, i));
                    } else {
                        foreign = true;
                    }
                }
            }
            (own, foreign)
        };
        for (i, c) in self.p.channels.iter().enumerate() {
            let Type::Stream(elem) = self.p.types.get(c.ty) else { continue };
            let key = StreamRef::Channel(takt_mir::ChannelId(i as u32));
            let (own, foreign) = readers(key);
            if c.dir == Direction::Output {
                if !own.is_empty() || foreign {
                    return no("Ausgabestrom mit Leser", c.span);
                }
                continue;
            }
            let source = self.p.channels.iter().position(|o| {
                o.dir == Direction::Output
                    && matches!((&o.binding, &c.binding), (Binding::Sim(a), Binding::Hw(b)) if a == b)
            });
            let fed = match source {
                Some(o) => match self.txs.iter().position(|t| t.channel.index() == o) {
                    Some(t) => Some(t),
                    None => return no("Eingabestrom aus dem `sim`-Ausgang einer anderen Maschine", c.span),
                },
                None => None,
            };
            let maxpt = match fed {
                // Was der Treiber abholt, kommt ohne Vertrag an (`apply_sim_bindings`).
                Some(t) => {
                    let width = self.txs[t].per_tick_bytes();
                    match self.p.types.get(*elem) {
                        Type::Int { width: takt_mir::types::IntWidth::U8, .. } | Type::Record(_) | Type::Enum(_) => {}
                        Type::Bytes { cap } if *cap >= width => {}
                        _ => return no("Eingabestrom aus einem `sim`-Ausgang mit Elementen dieser Art", c.span),
                    }
                    None
                }
                None => match takt_hal::edge::maxpt_of(c, self.p.config.tick) {
                    Some(m) => Some(m),
                    None => return no("Eingabestrom ohne `max_rate`", c.span),
                },
            };
            let cap = c.attrs.capacity.unwrap_or(16);
            let bytes = c.attrs.capacity_bytes.unwrap_or(cap.saturating_mul(256));
            let (cap, budget) = self.ring_slots(*elem, cap, bytes, c.span)?;
            // Ein Record vom Rand steht im Stimulus als `Name(…)`; ein Feld
            // variabler Laenge hat dort keine Textform.
            if budget.is_some() && fed.is_none() && matches!(self.p.types.get(*elem), Type::Record(_)) {
                return no("Record-Element variabler Laenge vom Rand", c.span);
            }
            out.push(Stream {
                key,
                name: c.name.clone(),
                elem: *elem,
                cap,
                budget,
                maxpt,
                overflow: c.attrs.overflow.unwrap_or_default(),
                wake: c.attrs.wake,
                channel: fed.is_none(),
                decodes: fed.is_none() && matches!(self.p.types.get(*elem), Type::Record(_)),
                readers: own,
                foreign_readers: foreign,
                fed,
            });
        }
        for (i, s) in self.p.streams.iter().enumerate() {
            let key = StreamRef::Internal(takt_mir::StreamId(i as u32));
            let (own, foreign) = readers(key);
            let bytes = s.capacity_bytes.unwrap_or(s.capacity.saturating_mul(256));
            let (cap, budget) = self.ring_slots(s.elem, s.capacity, bytes, s.span)?;
            let foreign_writer = s.writer.is_some_and(|w| !self.order.contains(&w));
            out.push(Stream {
                key,
                name: s.name.clone(),
                elem: s.elem,
                cap,
                budget,
                maxpt: foreign_writer.then_some(cap),
                overflow: s.overflow,
                wake: false,
                channel: false,
                decodes: false,
                readers: own,
                foreign_readers: foreign,
                fed: None,
            });
        }
        Ok(out)
    }

    /// Die Plaetze des Rings und, bei Elementen variabler Laenge, die
    /// Schranke in Bytes (`Buffer::push`). Belegt jedes Element gleich viele
    /// Bytes, kappt `CAPB` geteilt durch ihre Zahl schon die Plaetze.
    fn ring_slots(&self, elem: TypeId, cap: u32, bytes: u32, span: Span) -> R<(u32, Option<u32>)> {
        self.shape(elem, span)?;
        Ok(match self.element_bytes(elem) {
            Some(0) => (cap, None),
            Some(size) => (cap.min(bytes / size), None),
            None => (cap, Some(bytes)),
        })
    }

    /// Die Bytelast eines Werts (`stream::byte_len`): Text und Bytes ihre
    /// Laenge, ein Record die Summe seiner Felder und mindestens eins, ein
    /// Array die Summe, ein Vektor die seiner belegten Plaetze, sonst eins.
    fn byte_load(&self, ty: TypeId, v: &V, span: Span) -> R<Term> {
        Ok(match self.p.types.get(ty) {
            Type::Bytes { .. } | Type::Str { .. } | Type::Line { .. } => v.clone().part(0, span)?.leaf(span)?,
            Type::Record(r) => {
                let mut sum = Term::int(0);
                for (i, f) in self.p.records[r.index()].fields.iter().enumerate() {
                    sum = add(sum, self.byte_load(f.ty, &v.clone().part(i, span)?, span)?);
                }
                Term::ite(Term::bin(Op::Lt, sum.clone(), Term::int(1)), Term::int(1), sum)
            }
            Type::Array { elem, len } => {
                let mut sum = Term::int(0);
                for i in 0..*len as usize {
                    sum = add(sum, self.byte_load(*elem, &v.clone().part(i, span)?, span)?);
                }
                sum
            }
            Type::Vec { elem, cap } => {
                let len = v.clone().part(0, span)?.leaf(span)?;
                let mut sum = Term::int(0);
                for i in 0..*cap as usize {
                    let here = Term::bin(Op::Lt, Term::int(i as i64), len.clone());
                    let load = self.byte_load(*elem, &v.clone().part(i + 1, span)?, span)?;
                    sum = add(sum, Term::ite(here, load, Term::int(0)));
                }
                sum
            }
            _ => Term::int(1),
        })
    }

    /// Die Bytes, die der Ring gerade belegt.
    fn used(&mut self, s: &Stream, env: &Env) -> R<Term> {
        let span = Span::default();
        let shape = self.shape(s.elem, span)?;
        let (head, len) = (env[&loc(s, "head")].clone(), env[&loc(s, "len")].clone());
        let mut sum = Term::int(0);
        for i in 0..s.cap {
            let d = sub(Term::int(i64::from(i)), head.clone());
            let index =
                Term::ite(Term::bin(Op::Lt, d.clone(), Term::int(0)), add(d.clone(), Term::int(i64::from(s.cap))), d);
            let v = self.load(env, &format!("{}.v", slot(s, i)), &shape, span)?;
            let load = self.byte_load(s.elem, &v, span)?;
            sum = add(sum, Term::ite(Term::bin(Op::Lt, index, len.clone()), load, Term::int(0)));
        }
        Ok(sum)
    }

    /// Ein Wert als Element eines Stroms (`as_element`): Text in einen
    /// Bytestrom, Text in eine Zeile anderer Kapazitaet.
    fn as_element(&self, v: V, from: TypeId, elem: TypeId, span: Span) -> R<V> {
        if from == elem {
            return Ok(v);
        }
        let textual = |ty| matches!(self.p.types.get(ty), Type::Bytes { .. } | Type::Str { .. } | Type::Line { .. });
        if !(textual(from) && textual(elem)) {
            return Ok(v);
        }
        let t = Text::of(v, span)?;
        Ok(match self.p.types.get(elem) {
            Type::Bytes { cap } | Type::Str { cap } => t.value(*cap, None),
            Type::Line { cap } => t.value(*cap, Some(Term::bool(false))),
            _ => return no("Element", span),
        })
    }

    /// Die Bytelast eines Elements fester Groesse (`stream::byte_len` im
    /// Interpreter); `None` bei variabler Laenge.
    fn element_bytes(&self, ty: TypeId) -> Option<u32> {
        match self.p.types.get(ty) {
            Type::Record(r) => {
                let mut sum = 0u32;
                for f in &self.p.records[r.index()].fields {
                    sum = sum.checked_add(self.element_bytes(f.ty)?)?;
                }
                Some(sum.max(1))
            }
            Type::Array { elem, len } => self.element_bytes(*elem)?.checked_mul(*len),
            // Kopf aus vier Skalaren, dann die Abtastwerte (`byte_len`).
            Type::Capture { elem, len } => self.element_bytes(*elem)?.checked_mul(*len)?.checked_add(4),
            Type::Bytes { .. } | Type::Vec { .. } | Type::Str { .. } | Type::Line { .. } => None,
            _ => Some(1),
        }
    }

    pub(super) fn stream_index(&self, key: StreamRef, span: Span) -> R<usize> {
        match self.streams.iter().position(|s| s.key == key) {
            Some(i) => Ok(i),
            None => no("Strom", span),
        }
    }

    fn cursor_of(&self, m: MachineId, key: StreamRef) -> Option<usize> {
        self.machine(m).layout.cursors.iter().position(|r| *r == key)
    }

    pub(super) fn loc_cursor(&self, m: MachineId, i: usize) -> String {
        format!("s.{}.cur.{i}", self.machine(m).name)
    }

    fn loc_missed(&self, m: MachineId, i: usize) -> String {
        format!("s.{}.missed.{i}", self.machine(m).name)
    }

    /// Ein vorgemerkter `StreamOverflow` (9.6, `pending[m]`).
    pub(super) fn loc_pending(&self, m: MachineId) -> String {
        format!("s.{}.pending", self.machine(m).name)
    }

    /// Wie viele Elemente das Fenster eines Stroms hoechstens hat.
    pub(super) fn window_slots(&self, key: StreamRef, span: Span) -> R<i64> {
        Ok(i64::from(self.streams[self.stream_index(key, span)?].cap))
    }

    /// Liest die Maschine einen Eingabestrom, der ueberlaufen kann?
    pub(super) fn has_pending(&self, m: MachineId) -> bool {
        self.streams.iter().any(|s| s.channel && s.readers.iter().any(|(r, _)| *r == m))
    }

    /// Steht die Maschine in einem `idle`-Zustand (5.10)?
    fn idle(&self, m: MachineId, env: &Env) -> Term {
        let machine = self.machine(m);
        let leaf = env[&self.loc_leaf(m)].clone();
        let hits = self
            .leaves(m)
            .into_iter()
            .filter(|l| self.chain_to(m, *l).iter().any(|s| machine.states[s.index()].idle))
            .map(|l| Term::eq(leaf.clone(), Term::int(self.code(m, l))))
            .collect();
        Term::and(vec![Term::or(hits), env[&self.loc_faulted(m)].clone().not()])
    }

    /// Ringe, Zaehler und Cursor vor dem ersten Tick: leer und null.
    pub(super) fn streams_initial(&mut self, env: &mut Env) -> R<()> {
        for s in self.streams.clone() {
            for part in ["next", "len", "head", "dropped", "overflowed", "malformed"] {
                env.insert(loc(&s, part), Term::int(0));
            }
            let span = Span::default();
            let shape = self.shape(s.elem, span)?;
            let zero = self.zero_value(&shape, span)?;
            for i in 0..s.cap {
                env.insert(format!("{}.t", slot(&s, i)), Term::int(0));
                Enc::store(env, &format!("{}.v", slot(&s, i)), &shape, zero.clone());
            }
            for &(m, c) in &s.readers {
                env.insert(self.loc_cursor(m, c), Term::int(0));
                env.insert(self.loc_missed(m, c), Term::int(0));
            }
        }
        for m in self.order.clone() {
            if self.has_pending(m) {
                env.insert(self.loc_pending(m), Term::bool(false));
            }
        }
        Ok(())
    }

    /// Ein Platz des Rings an einer berechneten Stelle.
    fn slot_at(&mut self, s: &Stream, env: &Env, pos: &Term) -> R<(Term, V)> {
        let span = Span::default();
        let shape = self.shape(s.elem, span)?;
        let mut acc: Option<(Term, V)> = None;
        for i in (0..s.cap).rev() {
            let t = env[&format!("{}.t", slot(s, i))].clone();
            let v = self.load(env, &format!("{}.v", slot(s, i)), &shape, span)?;
            acc = Some(match acc {
                None => (t, v),
                Some((ta, va)) => {
                    let here = Term::eq(pos.clone(), Term::int(i64::from(i)));
                    (Term::ite(here.clone(), t, ta), V::ite(&here, v, va))
                }
            });
        }
        match acc {
            Some(x) => Ok(x),
            None => Ok((Term::int(0), self.zero_value(&shape, span)?)),
        }
    }

    /// Haengt ein Element an, wo `cond` gilt.
    fn append(&mut self, s: &Stream, env: &mut Env, cond: &Term, t: &Term, value: &V) -> R<()> {
        if s.cap == 0 || cond.is_bool(false) {
            return Ok(());
        }
        let span = Span::default();
        let shape = self.shape(s.elem, span)?;
        let (head, len, next) =
            (env[&loc(s, "head")].clone(), env[&loc(s, "len")].clone(), env[&loc(s, "next")].clone());
        let pos = wrap(add(head, len.clone()), s.cap);
        for i in 0..s.cap {
            let here = Term::and(vec![cond.clone(), Term::eq(pos.clone(), Term::int(i64::from(i)))]);
            let at = format!("{}.t", slot(s, i));
            let old = env[&at].clone();
            env.insert(at, Term::ite(here.clone(), t.clone(), old));
            let base = format!("{}.v", slot(s, i));
            let old = self.load(env, &base, &shape, span)?;
            Enc::store(env, &base, &shape, V::ite(&here, value.clone(), old));
        }
        env.insert(loc(s, "len"), Term::ite(cond.clone(), add(len.clone(), Term::int(1)), len));
        env.insert(loc(s, "next"), Term::ite(cond.clone(), add(next.clone(), Term::int(1)), next));
        Ok(())
    }

    /// Legt ein Element ab, wo `cond` gilt (`Buffer::push`): Passt es nicht,
    /// verdraengt es unter `drop_oldest` die aeltesten, sonst laeuft der Strom
    /// ueber; ein Element ueber der Byteschranke passt nie. Wahr, wo er
    /// ueberlief.
    fn push(&mut self, s: &Stream, env: &mut Env, cond: &Term, t: &Term, value: &V) -> R<Term> {
        self.push_as(s, env, cond, t, value, s.overflow == Overflow::DropOldest)
    }

    /// `push` (`StreamBuf::push`): ohne `drop_oldest` verliert ein voller
    /// Ring das neue Element und zaehlt den Ueberlauf.
    fn push_as(&mut self, s: &Stream, env: &mut Env, cond: &Term, t: &Term, value: &V, drop_oldest: bool) -> R<Term> {
        let span = Span::default();
        let cap = Term::int(i64::from(s.cap));
        let size = match s.budget {
            Some(_) => Some(self.byte_load(s.elem, value, span)?),
            None => None,
        };
        // Passt das Element zu dem, was der Ring jetzt haelt?
        let fits = |enc: &mut Self, env: &Env| -> R<Term> {
            let room = Term::bin(Op::Lt, env[&loc(s, "len")].clone(), cap.clone());
            Ok(match (&size, s.budget) {
                (Some(size), Some(budget)) => {
                    let used = enc.used(s, env)?;
                    Term::and(vec![room, Term::bin(Op::Le, add(used, size.clone()), Term::int(i64::from(budget)))])
                }
                _ => room,
            })
        };
        let too_big = match (&size, s.budget) {
            (Some(size), Some(budget)) => Term::bin(Op::Gt, size.clone(), Term::int(i64::from(budget))),
            _ => Term::bool(false),
        };
        if !drop_oldest || s.cap == 0 {
            let ok = fits(self, env)?;
            let over = Term::and(vec![cond.clone(), ok.clone().not()]);
            bump(env, &loc(s, "overflowed"), &over);
            self.append(s, env, &Term::and(vec![cond.clone(), ok]), t, value)?;
            return Ok(over);
        }
        let over = Term::and(vec![cond.clone(), too_big.clone()]);
        bump(env, &loc(s, "overflowed"), &over);
        let go = Term::and(vec![cond.clone(), too_big.not()]);
        // Je Verdraengung ein Element; feste Groesse braucht hoechstens eine.
        let rounds = if size.is_some() { s.cap } else { 1 };
        for _ in 0..rounds {
            let (head, len) = (env[&loc(s, "head")].clone(), env[&loc(s, "len")].clone());
            let shift =
                Term::and(vec![go.clone(), Term::bin(Op::Gt, len.clone(), Term::int(0)), fits(self, env)?.not()]);
            env.insert(loc(s, "head"), Term::ite(shift.clone(), wrap(add(head.clone(), Term::int(1)), s.cap), head));
            env.insert(loc(s, "len"), Term::ite(shift.clone(), sub(len.clone(), Term::int(1)), len));
            bump(env, &loc(s, "dropped"), &shift);
        }
        self.append(s, env, &go, t, value)?;
        Ok(over)
    }

    /// Ein Element der Lieferung `j` als freie Eingabe.
    fn element_input(&mut self, s: &Stream, j: u32) -> R<V> {
        let span = Span::default();
        let shape = self.shape(s.elem, span)?;
        let base = format!("i.stream.{}.{j}.v", s.name);
        self.gather(&base, &shape, &mut |enc, at, leaf| {
            let sort = enc.leaf_sort(leaf, span)?;
            Ok(enc.input(at.to_string(), sort))
        })
    }

    /// Das Zustellen zu Tick-Beginn (9.6, `deliver`): was der Rand liefert,
    /// Element fuer Element in den Ring; ein Ueberlauf merkt `StreamOverflow`
    /// fuer jeden Leser vor, der nicht schlaeft oder den der Strom weckt.
    /// `pre` ist der Zustand davor; im Tick 0 kommt die Lieferung vor dem
    /// ersten Eintritt (`run`: Stimulus, dann `init`), und niemand schlaeft.
    pub(super) fn deliver(&mut self, pre: Option<&Env>, cur: &mut Env) -> R<()> {
        for s in self.streams.clone() {
            if let (Some(t), Some(_)) = (s.fed, pre) {
                self.feed(&s, t, cur)?;
                continue;
            }
            let Some(bound) = s.maxpt else { continue };
            let n = self.input(format!("i.stream.{}.n", s.name), Sort::Int);
            let mut over = Vec::new();
            for j in 0..bound {
                let here = Term::bin(Op::Lt, Term::int(i64::from(j)), n.clone());
                let t = self.input(format!("i.stream.{}.{j}.t", s.name), Sort::Int);
                let value = self.element_input(&s, j)?;
                let mut ok = here.clone();
                if s.decodes {
                    let bad = self.input(format!("i.stream.{}.{j}.bad", s.name), Sort::Bool);
                    bump(cur, &loc(&s, "malformed"), &Term::and(vec![here, bad.clone()]));
                    ok = Term::and(vec![ok, bad.not()]);
                }
                if !s.channel {
                    // Ein fremder Schreiber laeuft nie ueber: Sein `send` haette
                    // ihn gefaultet (9.6).
                    let room = Term::bin(Op::Lt, cur[&loc(&s, "len")].clone(), Term::int(i64::from(s.cap)));
                    ok = Term::and(vec![ok, room]);
                }
                over.push(self.push(&s, cur, &ok, &t, &value)?);
            }
            if !s.channel {
                continue;
            }
            let over = Term::or(over);
            for &(m, _) in &s.readers {
                let hit = match pre {
                    Some(pre) if !s.wake => Term::and(vec![over.clone(), self.idle(m, pre).not()]),
                    _ => over.clone(),
                };
                let at = self.loc_pending(m);
                let old = cur[&at].clone();
                cur.insert(at, Term::or(vec![old, hit]));
            }
        }
        Ok(())
    }

    /// Ein `sim`-gespeister Eingabestrom (8.3, `apply_sim_bindings`): Was der
    /// Treiber beim letzten Commit abholte, kommt zu Tick-Beginn an, in
    /// einem `stream<u8>` Byte fuer Byte, in einem Bytestrom als ein
    /// Element, sonst in Slots der kanonischen Form (`elements_of`); was sich
    /// nicht lesen laesst, zaehlt als `malformed`, ein Ueberlauf zaehlt nur.
    fn feed(&mut self, s: &Stream, t: usize, cur: &mut Env) -> R<()> {
        let tx = self.txs[t].clone();
        let sent = self.sent_text(&tx, cur);
        let now = self.now.clone();
        match self.p.types.get(s.elem) {
            Type::Bytes { cap } => {
                let any = Term::bin(Op::Gt, sent.len.clone(), Term::int(0));
                let v = sent.value(*cap, None);
                self.push(s, cur, &any, &now, &v)?;
            }
            Type::Record(_) | Type::Enum(_) => {
                let span = Span::default();
                let Ok(size) = takt_mir::bytes::max_size(self.p, s.elem) else {
                    return no("Element ohne Byteform", span);
                };
                let size = size.max(1) as usize;
                for (j, chunk) in sent.bytes.chunks_exact(size).enumerate() {
                    let present = Term::bin(Op::Le, Term::int(((j + 1) * size) as i64), sent.len.clone());
                    let r = self.read_canonical(s.elem, chunk, &Term::int(0), span)?;
                    let ok = Term::and(vec![present.clone(), r.valid.clone()]);
                    self.push(s, cur, &ok, &now, &r.value)?;
                    bump(cur, &loc(s, "malformed"), &Term::and(vec![present, r.valid.not()]));
                }
            }
            _ => {
                for (j, b) in sent.bytes.iter().enumerate() {
                    let here = Term::bin(Op::Lt, Term::int(j as i64), sent.len.clone());
                    self.push(s, cur, &here, &now, &V::Leaf(b.clone()))?;
                }
            }
        }
        Ok(())
    }

    /// Was der Rand ueber die Lieferungen eines Ticks zusichert (12.6):
    /// hoechstens `MAXPT` Elemente, Zeitstempel im Fenster des Ticks und
    /// nicht fallend, Werte in ihrem Typ. Was nicht geliefert ist, steht fest.
    pub(super) fn stream_assumptions(&mut self) -> R<Vec<Term>> {
        let mut out = Vec::new();
        let span = Span::default();
        let now = Term::var(NOW, Sort::Int);
        let tick = self.p.config.tick;
        for s in self.streams.clone() {
            let Some(bound) = s.maxpt else { continue };
            let n = self.input(format!("i.stream.{}.n", s.name), Sort::Int);
            out.push(Term::and(vec![
                Term::bin(Op::Ge, n.clone(), Term::int(0)),
                Term::bin(Op::Le, n.clone(), Term::int(i64::from(bound))),
            ]));
            let shape = self.shape(s.elem, span)?;
            let mut prev: Option<Term> = None;
            for j in 0..bound {
                let here = Term::bin(Op::Lt, Term::int(i64::from(j)), n.clone());
                let implies = |a: Term, b: Term| Term::or(vec![a.not(), b]);
                let t = self.input(format!("i.stream.{}.{j}.t", s.name), Sort::Int);
                if s.channel {
                    let inside = Term::and(vec![
                        Term::bin(Op::Gt, t.clone(), sub(now.clone(), Term::int(tick))),
                        Term::bin(Op::Le, t.clone(), now.clone()),
                    ]);
                    out.push(implies(here.clone(), inside));
                    if let Some(p) = prev {
                        out.push(implies(here.clone(), Term::bin(Op::Le, p, t.clone())));
                    }
                }
                out.push(implies(here.clone().not(), Term::eq(t.clone(), Term::int(0))));
                prev = Some(t);
                let bad = if s.decodes {
                    let bad = self.input(format!("i.stream.{}.{j}.bad", s.name), Sort::Bool);
                    out.push(implies(here.clone().not(), bad.clone().not()));
                    bad
                } else {
                    Term::bool(false)
                };
                let base = format!("i.stream.{}.{j}.v", s.name);
                let mut leaves = Vec::new();
                Enc::leaf_locs(&base, &shape, &mut leaves);
                let used = Term::and(vec![here, bad.not()]);
                // Text und Bytes: hinter der Laenge null, Text gueltiges UTF-8
                // (3.9), eine gekuerzte Zeile endet hoechstens ein Zeichen vor
                // ihrer Kapazitaet (`parse_value` schneidet an einer Zeichengrenze).
                let (width, text, line) = match self.p.types.get(s.elem) {
                    Type::Bytes { cap } => (Some(*cap), false, false),
                    Type::Str { cap } => (Some(*cap), true, false),
                    Type::Line { cap } => (Some(*cap), true, true),
                    _ => (None, false, false),
                };
                if let Some(width) = width {
                    let len = Term::var(format!("{base}.len"), Sort::Int);
                    let bytes: Vec<Term> = (0..width).map(|i| Term::var(format!("{base}[{i}]"), Sort::Int)).collect();
                    for (i, b) in bytes.iter().enumerate() {
                        let inside = Term::bin(Op::Lt, Term::int(i as i64), len.clone());
                        out.push(implies(used.clone(), Term::or(vec![inside, Term::eq(b.clone(), Term::int(0))])));
                    }
                    if text {
                        out.push(implies(used.clone(), super::text::utf8(&Text { len: len.clone(), bytes })));
                    }
                    if line {
                        let cut = Term::and(vec![used.clone(), Term::var(format!("{base}.truncated"), Sort::Bool)]);
                        out.push(implies(cut, Term::bin(Op::Ge, len, Term::int(i64::from(width) - 3))));
                    }
                }
                for (at, leaf) in leaves {
                    let x = self.input(at, self.leaf_sort(&leaf, span)?);
                    if let Some(inv) = self.leaf_invariant(x.clone(), &leaf) {
                        out.push(implies(used.clone(), inv));
                    }
                    let zero = match x.sort() {
                        Sort::F32 | Sort::F64 => Term::bin(Op::FEq, x.clone(), Enc::zero(x.sort())),
                        sort => Term::eq(x.clone(), Enc::zero(sort)),
                    };
                    out.push(implies(used.clone().not(), zero));
                }
            }
        }
        Ok(out)
    }

    /// Das Fenster eines Lesers in diesem Tick: die Elemente ab seinem
    /// Cursor, nach dem Zustellen fest (9.6). Ein Leser ohne Cursor sieht
    /// den Puffer ab seinem Anfang.
    fn window(&mut self, m: MachineId, key: StreamRef, span: Span) -> R<(Option<usize>, Window)> {
        let cursor = self.cursor_of(m, key);
        if let Some(c) = cursor
            && let Some(w) = self.windows.get(&(m, c))
        {
            return Ok((cursor, w.clone()));
        }
        let s = self.streams[self.stream_index(key, span)?].clone();
        let env = self.delivered.clone();
        let (next, len, head) =
            (env[&loc(&s, "next")].clone(), env[&loc(&s, "len")].clone(), env[&loc(&s, "head")].clone());
        let start = sub(next.clone(), len);
        let from = match cursor {
            Some(c) => {
                let cur = env[&self.loc_cursor(m, c)].clone();
                Term::ite(Term::bin(Op::Gt, cur.clone(), start.clone()), cur, start.clone())
            }
            None => start.clone(),
        };
        let count = sub(next.clone(), from.clone());
        let offset = add(head, sub(from.clone(), start));
        let mut items = Vec::new();
        for j in 0..s.cap {
            let j = Term::int(i64::from(j));
            let (t, value) = self.slot_at(&s, &env, &wrap(add(offset.clone(), j.clone()), s.cap))?;
            items.push(Item {
                present: Term::bin(Op::Lt, j.clone(), count.clone()),
                seq: add(from.clone(), j),
                t,
                value,
            });
        }
        let w = Window { count, end: next, items };
        if let Some(c) = cursor {
            self.windows.insert((m, c), w.clone());
        }
        Ok((cursor, w))
    }

    fn mark(&mut self, m: MachineId, cursor: Option<usize>, cond: Term, seq: Term) {
        if let Some(cursor) = cursor {
            self.marks.push(Mark { m, cursor, cond, seq });
        }
    }

    /// Rueckt den Unroll-Zaehler um ein Fenster vor.
    fn unroll(&mut self, n: usize, span: Span) -> R<()> {
        self.unrolled = self.unrolled.saturating_add(n as i64);
        if self.unrolled > UNROLL_LIMIT {
            return no(format!("mehr als {UNROLL_LIMIT} Durchlaeufe von Schleifen auf einem Pfad"), span);
        }
        Ok(())
    }

    /// Die Bindung eines Elements (8.7, `element_record`): die Captures,
    /// dann `.t`, `.seq` und der Inhalt unter `.data` oder `.text`.
    fn binding(&mut self, ty: TypeId, item: &Item, caps: Vec<V>, span: Span) -> R<V> {
        let Type::Record(r) = self.p.types.get(ty) else { return no("Bindung ohne Record", span) };
        let fields = self.p.records[r.index()].fields.clone();
        let mut parts = caps;
        for f in fields.iter().skip(parts.len()) {
            parts.push(match f.name.as_str() {
                "t" => V::Leaf(item.t.clone()),
                "seq" => V::Leaf(item.seq.clone()),
                "text" | "data" => item.value.clone(),
                _ => self.zero_of(f.ty, span)?,
            });
        }
        parts.truncate(fields.len());
        Ok(V::Node(parts))
    }

    /// Trifft das Muster eines Handlers oder Guards das Element, und was
    /// bindet es? Ein Record-Muster vergleicht die genannten Felder, ein
    /// Textmuster laeuft ueber den Text des Elements (8.7).
    fn hits(
        &mut self,
        pattern: Option<(MatchKind, &Pattern)>,
        item: &Item,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<(Term, Vec<V>)> {
        match pattern {
            None => Ok((Term::bool(true), Vec::new())),
            Some((_, Pattern::Record { fields, .. })) => {
                let mut conds = Vec::new();
                for (index, e) in fields {
                    let want = self.value(e, cx, env, flow)?;
                    let got = item.value.clone().part(*index as usize, span)?;
                    conds.push(Enc::equal(&got, &want));
                }
                Ok((Term::and(conds), Vec::new()))
            }
            Some((kind, Pattern::Text { pieces })) => {
                let text = Text::of(item.value.clone(), span)?;
                self.text_match(pieces, kind, &text, span)
            }
        }
    }

    /// Handler-Dispatch einer Ebene (9.7): je Strom in der Reihenfolge
    /// seines ersten Handlers, je Element des Fensters, je Handler. Der erste
    /// passende gewinnt, ein Uebergang beendet die Verarbeitung; auch ein
    /// Element, auf das keiner passt, gilt als untersucht.
    pub(super) fn dispatch(&mut self, handlers: &[Handler], cx: &Cx<'_>, env: &mut Env, flow: &mut Flow) -> R<()> {
        let m = cx.m.expect("Maschine");
        let mut keys: Vec<StreamRef> = Vec::new();
        for h in handlers {
            if !keys.contains(&h.stream) {
                keys.push(h.stream);
            }
        }
        for key in keys {
            let mine: Vec<&Handler> = handlers.iter().filter(|h| h.stream == key).collect();
            let span = mine[0].span;
            let (cursor, w) = self.window(m, key, span)?;
            self.unroll(w.items.len(), span)?;
            for item in &w.items {
                // Noch kein Handler hat das Element genommen.
                let mut rest = Term::and(vec![flow.alive.clone(), item.present.clone()]);
                let mut alive = vec![Term::and(vec![flow.alive.clone(), item.present.clone().not()])];
                for h in &mine {
                    let pattern = h.pattern.as_ref().map(|(k, p)| (*k, p));
                    let (hit, caps) = self.hits(pattern, item, cx, env, flow, h.span)?;
                    let cond = Term::and(vec![rest.clone(), hit.clone()]);
                    // Die Bindung steht vor dem Guard, auch wenn er nicht gilt.
                    if let Some(var) = h.binding {
                        let ty = self.machine(m).vars[var.index()].ty;
                        let v = self.binding(ty, item, caps, h.span)?;
                        self.put(env, &self.loc_var(m, var), ty, v, &cond, h.span)?;
                    }
                    let mut fg = Flow::new(cond.clone());
                    let g = match &h.guard {
                        Some(g) => self.expr(g, cx, env, &mut fg)?,
                        None => Term::bool(true),
                    };
                    self.flush_binds(env)?;
                    flow.exits.extend(fg.exits);
                    let fire = Term::and(vec![fg.alive.clone(), g.clone()]);
                    self.mark(m, cursor, fire.clone(), item.seq.clone());
                    let mut env_b = env.clone();
                    let mut fb = Flow::new(fire.clone());
                    self.block(&h.body, cx, &mut env_b, &mut fb)?;
                    *env = ite_env(&fire, &env_b, env);
                    flow.exits.extend(fb.exits);
                    alive.push(fb.alive);
                    rest = Term::or(vec![Term::and(vec![rest, hit.not()]), Term::and(vec![fg.alive, g.not()])]);
                }
                self.mark(m, cursor, rest.clone(), item.seq.clone());
                alive.push(rest);
                flow.alive = Term::or(alive);
            }
        }
        Ok(())
    }

    /// Ein Guard ueber einem Strom (8.7): `s as e` trifft das erste Element
    /// des Fensters, `s matches P as e` das erste passende.
    pub(super) fn stream_guard(&mut self, g: &Guard, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<GuardHit> {
        let m = cx.m.expect("Maschine");
        let (key, pattern, binding, span) = match g {
            Guard::Next { stream, binding } => (*stream, None, Some(*binding), Span::default()),
            Guard::Match { subject, kind, pattern, binding } => match stream_of(subject) {
                Some(key) => (key, Some((*kind, pattern)), *binding, subject.span),
                None => return no("Musterabgleich auf einem Wert", subject.span),
            },
            Guard::Expr(e) => return no("Guard ohne Strom", e.span),
        };
        let (cursor, w) = self.window(m, key, span)?;
        // 7.5: Traegt die Bindung den Elementtyp, ist sie das Element selbst.
        let s = self.streams[self.stream_index(key, span)?].clone();
        let bound = binding.map(|v| (v, self.machine(m).vars[v.index()].ty));
        let mut found = Term::bool(false);
        let mut value: Option<V> = None;
        let mut seq = Term::int(-1);
        for item in &w.items {
            let (hit, caps) = self.hits(pattern, item, cx, env, flow, span)?;
            let first = Term::and(vec![item.present.clone(), hit, found.clone().not()]);
            if let Some((_, ty)) = bound {
                let v = if matches!(key, StreamRef::Internal(_)) && ty == s.elem {
                    item.value.clone()
                } else {
                    self.binding(ty, item, caps, span)?
                };
                value = Some(match value {
                    None => v,
                    Some(old) => V::ite(&first, v, old),
                });
            }
            seq = Term::ite(first.clone(), item.seq.clone(), seq);
            found = Term::or(vec![found, first]);
        }
        Ok(GuardHit {
            fired: found,
            bind: bound.and_then(|(v, _)| value.map(|x| (v, x))),
            mark: cursor.map(|c| (c, seq)),
        })
    }

    /// Wendet einen genommenen Guard an: Bindung und Marke gelten, wo `take` gilt.
    pub(super) fn take_guard(&mut self, hit: GuardHit, m: MachineId, take: &Term, env: &mut Env) -> R<()> {
        if let Some((var, v)) = hit.bind {
            let ty = self.machine(m).vars[var.index()].ty;
            self.put(env, &self.loc_var(m, var), ty, v, take, Span::default())?;
        }
        if let Some((cursor, seq)) = hit.mark {
            self.marks.push(Mark { m, cursor, cond: take.clone(), seq });
        }
        Ok(())
    }

    /// `for e in s:` ueber das Fenster (8.7): Jedes betrachtete Element gilt
    /// als untersucht, ein `break` laesst den Rest stehen. Im Modus ENTRY ist
    /// das Fenster leer.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_window(
        &mut self,
        var: VarId,
        key: StreamRef,
        body: &Block,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        if cx.mode == Mode::Entry {
            return Ok(());
        }
        let m = cx.m.expect("Maschine");
        let (cursor, w) = self.window(m, key, span)?;
        self.unroll(w.items.len(), span)?;
        let (at, ty) = (self.loc_var(m, var), self.machine(m).vars[var.index()].ty);
        self.breaks.push(Vec::new());
        for (j, item) in w.items.iter().enumerate() {
            let mut env_k = env.clone();
            let mut fk = Flow::new(Term::and(vec![flow.alive.clone(), item.present.clone()]));
            let v = self.binding(ty, item, Vec::new(), span)?;
            self.put(&mut env_k, &at, ty, v, &fk.alive.clone(), span)?;
            self.mark(m, cursor, fk.alive.clone(), item.seq.clone());
            self.loop_path.push(j as i64);
            self.block(body, cx, &mut env_k, &mut fk)?;
            self.loop_path.pop();
            *env = ite_env(&item.present, &env_k, env);
            flow.exits.extend(fk.exits);
            flow.alive = Term::or(vec![Term::and(vec![flow.alive.clone(), item.present.clone().not()]), fk.alive]);
        }
        let broke = self.breaks.pop().unwrap_or_default();
        flow.alive = Term::or(std::iter::once(flow.alive.clone()).chain(broke).collect());
        Ok(())
    }

    /// `s.peek()` (8.6): das erste Element des Fensters, untersucht; im
    /// Modus ENTRY keines.
    pub(super) fn peek(&mut self, base: &Expr, cx: &Cx<'_>, flow: &Flow, span: Span) -> R<V> {
        let Some(key) = stream_of(base) else { return no("`peek` ohne Strom", span) };
        let s = self.streams[self.stream_index(key, span)?].clone();
        let zero = self.zero_of(s.elem, span)?;
        let m = cx.m.expect("Maschine");
        if cx.mode == Mode::Entry {
            return Ok(V::Node(vec![V::Leaf(Term::bool(false)), zero]));
        }
        let (cursor, w) = self.window(m, key, span)?;
        let Some(first) = w.items.first().cloned() else {
            return Ok(V::Node(vec![V::Leaf(Term::bool(false)), zero]));
        };
        self.mark(m, cursor, Term::and(vec![flow.alive.clone(), first.present.clone()]), first.seq.clone());
        Ok(V::Node(vec![V::Leaf(first.present.clone()), V::ite(&first.present, first.value, zero)]))
    }

    /// `s.count`, `.dropped`, `.overflowed`, `.malformed` (8.6, 9.6).
    pub(super) fn stream_accessor(
        &mut self,
        base: &Expr,
        acc: Accessor,
        cx: &Cx<'_>,
        env: &Env,
        span: Span,
    ) -> R<Term> {
        if let (Some(c), Accessor::Free | Accessor::Idle) = (self.tx_channel(base), acc) {
            return self.tx_accessor(c, acc, base.ty, cx, env, span)?.leaf(span);
        }
        let Some(key) = stream_of(base) else { return no("Zugriff ohne Strom", span) };
        let s = self.streams[self.stream_index(key, span)?].clone();
        let m = cx.m.expect("Maschine");
        Ok(match acc {
            Accessor::Count if cx.mode == Mode::Entry => Term::int(0),
            Accessor::Count => self.window(m, key, span)?.1.count,
            // `dropped[s]` am Puffer plus `dropped[s, m]` dieser Maschine (9.6).
            Accessor::Dropped => {
                let own = self.cursor_of(m, key).map(|c| env[&self.loc_missed(m, c)].clone());
                add(env[&loc(&s, "dropped")].clone(), own.unwrap_or_else(|| Term::int(0)))
            }
            Accessor::Overflowed => env[&loc(&s, "overflowed")].clone(),
            Accessor::Malformed => env[&loc(&s, "malformed")].clone(),
            other => return no(format!("Zugriff `.{}` auf einen Strom", other.name()), span),
        })
    }

    /// `s.skip()` (8.6): untersucht das ganze Fenster.
    pub(super) fn skip(&mut self, key: StreamRef, cx: &Cx<'_>, flow: &Flow, span: Span) -> R<()> {
        if cx.mode == Mode::Entry {
            return Ok(());
        }
        let m = cx.m.expect("Maschine");
        let (cursor, w) = self.window(m, key, span)?;
        let any = Term::bin(Op::Gt, w.count.clone(), Term::int(0));
        self.mark(m, cursor, Term::and(vec![flow.alive.clone(), any]), sub(w.end, Term::int(1)));
        Ok(())
    }

    /// `send q, e` auf einen internen Strom (8.6): Frei ist, was Puffer und
    /// die Sendungen dieses Ticks nicht belegen; sonst trifft der Ueberlauf
    /// den Schreiber, unter `overflow = drop` faellt das Element weg.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send(
        &mut self,
        key: StreamRef,
        value: &Expr,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        if let StreamRef::Channel(c) = key {
            return self.send_tx(c, value, cx, env, flow, span);
        }
        let si = self.stream_index(key, span)?;
        let s = self.streams[si].clone();
        let v = self.value(value, cx, env, flow)?;
        let v = self.as_element(v, value.ty, s.elem, span)?;
        let mut count = env[&loc(&s, "len")].clone();
        for q in self.queued.iter().filter(|q| q.stream == si) {
            count = add(count, Term::ite(q.cond.clone(), Term::int(1), Term::int(0)));
        }
        let mut full = Term::bin(Op::Ge, count, Term::int(i64::from(s.cap)));
        let mut too_big = Term::bool(false);
        if let Some(budget) = s.budget {
            let size = self.byte_load(s.elem, &v, span)?;
            let mut bytes = add(self.used(&s, env)?, size.clone());
            for q in self.queued.clone().iter().filter(|q| q.stream == si) {
                let load = self.byte_load(s.elem, &q.value, span)?;
                bytes = add(bytes, Term::ite(q.cond.clone(), load, Term::int(0)));
            }
            let budget = Term::int(i64::from(budget));
            full = Term::or(vec![full, Term::bin(Op::Gt, bytes, budget.clone())]);
            too_big = Term::bin(Op::Gt, size, budget);
        }
        // `drop_oldest` verdraengt beim Zustellen; abgelehnt wird nur, was nie passt.
        let full = match s.overflow {
            Overflow::DropOldest if s.cap > 0 => too_big,
            Overflow::DropOldest => Term::bool(true),
            _ => full,
        };
        let fits = Term::and(vec![flow.alive.clone(), full.clone().not()]);
        if s.overflow != Overflow::Drop {
            let over = Term::and(vec![flow.alive.clone(), full]);
            bump(env, &loc(&s, "overflowed"), &over);
            flow.exits
                .push(Exit { cond: over, kind: ExitKind::Fault(None, self.cause(FaultKind::StreamOverflow, span)) });
            flow.alive = fits.clone();
        }
        self.queued.push(Queued { stream: si, cond: fits, t: self.now.clone(), value: v, port: false });
        Ok(())
    }

    /// Das Tick-Ende (9.6): Cursor hinter das zuletzt untersuchte Element,
    /// eine schlafende Maschine verwirft ihre Nicht-Wake-Stroeme; dann faellt
    /// weg, was kein Leser mehr sehen kann, und die Sendungen des Ticks kommen
    /// in den Ring.
    pub(super) fn advance_streams(&mut self, cur: &mut Env) -> R<()> {
        let marks = std::mem::take(&mut self.marks);
        let streams = self.streams.clone();
        for s in &streams {
            for &(m, c) in &s.readers {
                let mut ex = Term::int(-1);
                for mk in marks.iter().filter(|x| x.m == m && x.cursor == c) {
                    let later = Term::and(vec![mk.cond.clone(), Term::bin(Op::Gt, mk.seq.clone(), ex.clone())]);
                    ex = Term::ite(later, mk.seq.clone(), ex);
                }
                let at = self.loc_cursor(m, c);
                let old = cur[&at].clone();
                let after = add(ex.clone(), Term::int(1));
                let moved = Term::ite(Term::bin(Op::Ge, ex, Term::int(0)), after.clone(), old.clone());
                if s.wake {
                    cur.insert(at, moved);
                    continue;
                }
                // 5.10: Was die Maschine in diesem Tick untersucht hat, ist
                // konsumiert, nicht verworfen.
                let idle = self.idle(m, cur);
                let end = cur[&loc(s, "next")].clone();
                let before = Term::ite(Term::bin(Op::Gt, after.clone(), old.clone()), after, old);
                let missed = self.loc_missed(m, c);
                let was = cur[&missed].clone();
                cur.insert(missed, Term::ite(idle.clone(), add(was.clone(), sub(end.clone(), before)), was));
                cur.insert(at, Term::ite(idle, end, moved));
            }
        }
        for s in &streams {
            let (next, len, head) =
                (cur[&loc(s, "next")].clone(), cur[&loc(s, "len")].clone(), cur[&loc(s, "head")].clone());
            let start = sub(next, len.clone());
            let mut cursors: Vec<Term> = s.readers.iter().map(|&(m, c)| cur[&self.loc_cursor(m, c)].clone()).collect();
            if s.foreign_readers {
                cursors.push(self.input(format!("i.stream.{}.foreign", s.name), Sort::Int));
            }
            // Ohne Leser behaelt der Strom nichts (FB-426).
            let gone = match cursors.into_iter().reduce(|a, b| Term::ite(Term::bin(Op::Lt, a.clone(), b.clone()), a, b))
            {
                None => len.clone(),
                Some(min) => {
                    let e = sub(min, start);
                    let e = Term::ite(Term::bin(Op::Lt, e.clone(), Term::int(0)), Term::int(0), e);
                    Term::ite(Term::bin(Op::Gt, e.clone(), len.clone()), len.clone(), e)
                }
            };
            if s.cap > 0 {
                cur.insert(loc(s, "head"), wrap(add(head, gone.clone()), s.cap));
            }
            cur.insert(loc(s, "len"), sub(len, gone));
        }
        self.flush_sends(cur)
    }

    /// Die Sendungen eines Ticks oder des Starts kommen in den Ring; unter
    /// `drop_oldest` verdraengen sie die aeltesten (9.6, `deliver`).
    pub(super) fn flush_sends(&mut self, cur: &mut Env) -> R<()> {
        for q in std::mem::take(&mut self.queued) {
            let s = self.streams[q.stream].clone();
            if q.port {
                self.push_as(&s, cur, &q.cond, &q.t, &q.value, false)?;
            } else if s.overflow == Overflow::DropOldest {
                self.push(&s, cur, &q.cond, &q.t, &q.value)?;
            } else {
                self.append(&s, cur, &q.cond, &q.t, &q.value)?;
            }
        }
        Ok(())
    }

    /// Was in jedem erreichbaren Zustand fuer Ring und Cursor gilt.
    pub(super) fn stream_invariants(&self, pre: &Env, out: &mut Vec<Term>) {
        let within = |x: &Term, lo: Term, hi: Term| {
            Term::and(vec![Term::bin(Op::Ge, x.clone(), lo), Term::bin(Op::Le, x.clone(), hi)])
        };
        for s in &self.streams {
            let (next, len, head) =
                (pre[&loc(s, "next")].clone(), pre[&loc(s, "len")].clone(), pre[&loc(s, "head")].clone());
            out.push(within(&len, Term::int(0), Term::int(i64::from(s.cap))));
            out.push(within(&head, Term::int(0), Term::int(i64::from(s.cap.max(1) - 1))));
            out.push(Term::bin(Op::Le, len, next.clone()));
            for part in ["dropped", "overflowed", "malformed"] {
                out.push(Term::bin(Op::Ge, pre[&loc(s, part)].clone(), Term::int(0)));
            }
            for &(m, c) in &s.readers {
                out.push(within(&pre[&self.loc_cursor(m, c)], Term::int(0), next.clone()));
                out.push(Term::bin(Op::Ge, pre[&self.loc_missed(m, c)].clone(), Term::int(0)));
            }
        }
    }
}
