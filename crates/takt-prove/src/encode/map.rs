//! `map<K, V, N>` im Modell (3.9, `takt_native::map`): N Slots aus `.has`,
//! `.key` und `.value`, ein freier Slot null. Der Hash ist FNV-1a ueber die
//! kanonische Byteform des Schluessels (5.9), aufgefuellt auf ihre
//! Obergrenze; gesucht wird linear ab `hash % N`, entfernt per
//! Rueckwaertsverschiebung — dieselbe Sondierung wie im Interpreter und im
//! erzeugten Code, also dieselbe Slot-Folge, ueber die `for (k, v) in m`
//! laeuft.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::TypeId;
use takt_mir::expr::Expr;
use takt_mir::stmt::{Block, Method};
use takt_mir::types::Type;

use super::value::V;
use super::{Cx, Enc, Env, Flow, R, ite_env, no};
use crate::term::{Op, Term};

fn int(i: i64) -> Term {
    Term::int(i)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

/// Das Element `i` (`i` liegt in den Grenzen).
fn sel(items: &[Term], i: &Term) -> Term {
    let mut it = items.iter().enumerate().rev();
    let Some((_, last)) = it.next() else { return int(0) };
    it.fold(last.clone(), |acc, (k, x)| Term::ite(Term::eq(i.clone(), int(k as i64)), x.clone(), acc))
}

/// Ein Slot: belegt, Schluessel, Wert.
#[derive(Clone)]
struct Slot {
    has: Term,
    key: V,
    value: V,
}

impl Slot {
    fn of(v: V, span: Span) -> R<Slot> {
        let V::Node(mut parts) = v else { return no("Slot einer map", span) };
        let (Some(value), Some(key), Some(has)) = (parts.pop(), parts.pop(), parts.pop()) else {
            return no("Slot einer map", span);
        };
        Ok(Slot { has: has.leaf(span)?, key, value })
    }

    fn value(self) -> V {
        V::Node(vec![V::Leaf(self.has), self.key, self.value])
    }

    fn ite(c: &Term, a: &Slot, b: &Slot) -> Slot {
        Slot {
            has: Term::ite(c.clone(), a.has.clone(), b.has.clone()),
            key: V::ite(c, a.key.clone(), b.key.clone()),
            value: V::ite(c, a.value.clone(), b.value.clone()),
        }
    }
}

/// Eine Map: Schluessel- und Werttyp, Slots, je Slot der Hash seines
/// Schluessels.
struct Table {
    key: TypeId,
    slots: Vec<Slot>,
    hashes: Vec<Term>,
}

impl Table {
    fn cap(&self) -> i64 {
        self.slots.len() as i64
    }

    fn home(&self, hash: &Term) -> Term {
        Term::bin(Op::Rem, hash.clone(), int(self.cap()))
    }

    fn step(&self, i: &Term, s: i64) -> Term {
        Term::bin(Op::Rem, add(i.clone(), int(s)), int(self.cap()))
    }

    fn occupied(&self) -> Vec<Term> {
        self.slots.iter().map(|s| s.has.clone()).collect()
    }

    /// `find`: ob der Schluessel steht, und sein Slot.
    fn find(&self, key: &V, hash: &Term) -> (Term, Term) {
        let occupied = self.occupied();
        let hits: Vec<Term> = self
            .slots
            .iter()
            .zip(&self.hashes)
            .map(|(s, h)| Term::and(vec![s.has.clone(), Term::eq(h.clone(), hash.clone()), Enc::equal(&s.key, key)]))
            .collect();
        let home = self.home(hash);
        let (mut done, mut found, mut at) = (Term::bool(false), Term::bool(false), int(0));
        for s in 0..self.cap() {
            let i = self.step(&home, s);
            let hit = sel(&hits, &i);
            let take = Term::and(vec![done.clone().not(), hit.clone()]);
            found = Term::or(vec![found, take.clone()]);
            at = Term::ite(take, i.clone(), at);
            done = Term::or(vec![done, sel(&occupied, &i).not(), hit]);
        }
        (found, at)
    }

    /// `free_slot`: ob ein Slot frei ist, und der erste ab der Hash-Position.
    fn free(&self, hash: &Term) -> (Term, Term) {
        let occupied = self.occupied();
        let home = self.home(hash);
        let (mut done, mut found, mut at) = (Term::bool(false), Term::bool(false), int(0));
        for s in 0..self.cap() {
            let i = self.step(&home, s);
            let empty = sel(&occupied, &i).not();
            let take = Term::and(vec![done.clone().not(), empty.clone()]);
            found = Term::or(vec![found, take.clone()]);
            at = Term::ite(take, i, at);
            done = Term::or(vec![done, empty]);
        }
        (found, at)
    }

    fn select(&self, i: &Term) -> (Slot, Term) {
        let mut it = self.slots.iter().zip(&self.hashes).enumerate().rev();
        let Some((_, (last, h))) = it.next() else { unreachable!("eine map hat Slots") };
        it.fold((last.clone(), h.clone()), |(acc, ah), (k, (s, h))| {
            let here = Term::eq(i.clone(), int(k as i64));
            (Slot::ite(&here, s, &acc), Term::ite(here, h.clone(), ah))
        })
    }

    /// Schreibt `slot` mit seinem Hash nach `i`, wo `c` gilt.
    fn write(&mut self, c: &Term, i: &Term, slot: &Slot, hash: &Term) {
        for (k, (s, h)) in self.slots.iter_mut().zip(&mut self.hashes).enumerate() {
            let here = Term::and(vec![c.clone(), Term::eq(i.clone(), int(k as i64))]);
            *s = Slot::ite(&here, slot, s);
            *h = Term::ite(here, hash.clone(), h.clone());
        }
    }

    /// `remove_at`: leert `at` und schliesst die Sondierkette per
    /// Rueckwaertsverschiebung, wo `c` gilt.
    fn remove_at(&mut self, c: &Term, at: &Term, empty: &Slot) {
        self.write(c, at, empty, &int(0));
        let (mut hole, mut j, mut stopped) = (at.clone(), at.clone(), c.clone().not());
        for _ in 0..self.cap() {
            j = self.step(&j, 1);
            let (slot, hash) = self.select(&j);
            stopped = Term::or(vec![stopped, slot.has.clone().not()]);
            let home = self.home(&hash);
            let after_hole = Term::bin(Op::Gt, home.clone(), hole.clone());
            let up_to_j = Term::bin(Op::Le, home, j.clone());
            let stays = Term::ite(
                Term::bin(Op::Le, hole.clone(), j.clone()),
                Term::and(vec![after_hole.clone(), up_to_j.clone()]),
                Term::or(vec![after_hole, up_to_j]),
            );
            let moves = Term::and(vec![stopped.clone().not(), stays.not()]);
            self.write(&moves, &hole, &slot, &hash);
            self.write(&moves, &j, empty, &int(0));
            hole = Term::ite(moves, j.clone(), hole);
        }
    }

    fn len(&self) -> Term {
        self.slots.iter().fold(int(0), |n, s| add(n, Term::ite(s.has.clone(), int(1), int(0))))
    }
}

impl Enc<'_> {
    /// Die Slots einer Map mit den Hashes ihrer Schluessel.
    fn table(&self, ty: TypeId, v: V, span: Span) -> R<Table> {
        let Type::Map { key, .. } = self.p.types.get(ty) else { return no("map", span) };
        let key = *key;
        let V::Node(parts) = v else { return no("map", span) };
        let mut slots = Vec::new();
        let mut hashes = Vec::new();
        for p in parts {
            let s = Slot::of(p, span)?;
            hashes.push(self.key_hash(key, s.key.clone(), span)?);
            slots.push(s);
        }
        if slots.is_empty() {
            return no("map ohne Slots", span);
        }
        Ok(Table { key, slots, hashes })
    }

    fn empty_slot(&self, ty: TypeId, span: Span) -> R<Slot> {
        let Type::Map { key, value, .. } = self.p.types.get(ty) else { return no("map", span) };
        let (key, value) = (*key, *value);
        Ok(Slot { has: Term::bool(false), key: self.zero_of(key, span)?, value: self.zero_of(value, span)? })
    }

    /// FNV-1a ueber die auf ihre Obergrenze aufgefuellte Byteform.
    fn key_hash(&self, ty: TypeId, key: V, span: Span) -> R<Term> {
        let size = match takt_mir::bytes::max_size(self.p, ty) {
            Ok(n) => n as usize,
            Err(_) => return no("Schluessel ohne Byteform", span),
        };
        let form = self.canonical(ty, key, span)?;
        let mut h = int(0x811c_9dc5);
        for i in 0..size {
            let b = form.bytes.get(i).cloned().unwrap_or_else(|| int(0));
            let mixed = Term::bin(Op::Mul, Term::bin(Op::BitXor, h, b), int(0x0100_0193));
            h = Term::bin(Op::BitAnd, mixed, int(0xFFFF_FFFF));
        }
        Ok(h)
    }

    /// `insert`, `remove` und `clear` (`maps.rs`): die neue Map und das
    /// Ergebnis.
    pub(super) fn map_update(&self, method: Method, ty: TypeId, old: V, xs: &[V], span: Span) -> R<(V, Term)> {
        let empty = self.empty_slot(ty, span)?;
        let mut t = self.table(ty, old, span)?;
        let done = match (method, xs) {
            (Method::Insert, [k, v]) => {
                let hash = self.key_hash(t.key, k.clone(), span)?;
                let (found, at) = t.find(k, &hash);
                let (free, slot) = t.free(&hash);
                let ok = Term::or(vec![found.clone(), free]);
                let target = Term::ite(found, at, slot);
                let entry = Slot { has: Term::bool(true), key: k.clone(), value: v.clone() };
                t.write(&ok, &target, &entry, &hash);
                ok
            }
            (Method::Remove, [k]) => {
                let hash = self.key_hash(t.key, k.clone(), span)?;
                let (found, at) = t.find(k, &hash);
                t.remove_at(&found, &at, &empty);
                found
            }
            (Method::Clear, []) => {
                let all = Term::bool(true);
                for k in 0..t.cap() {
                    t.write(&all, &int(k), &empty, &int(0));
                }
                all
            }
            _ => return no("Methode einer map", span),
        };
        Ok((V::Node(t.slots.into_iter().map(Slot::value).collect()), done))
    }

    /// `m.get(k)` als Optional.
    pub(super) fn map_get(&self, ty: TypeId, m: V, key: V, span: Span) -> R<V> {
        let t = self.table(ty, m, span)?;
        let hash = self.key_hash(t.key, key.clone(), span)?;
        let (found, at) = t.find(&key, &hash);
        let (slot, _) = t.select(&at);
        let zero = self.empty_slot(ty, span)?.value;
        Ok(V::Node(vec![V::Leaf(found.clone()), V::ite(&found, slot.value, zero)]))
    }

    /// Die Slots in ihrer Folge: belegt, Schluessel, Wert.
    pub(super) fn map_entries(&self, ty: TypeId, m: V, span: Span) -> R<Vec<(Term, V, V)>> {
        Ok(self.table(ty, m, span)?.slots.into_iter().map(|s| (s.has, s.key, s.value)).collect())
    }

    /// `m.len`: die belegten Slots.
    pub(super) fn map_len(&self, ty: TypeId, m: V, span: Span) -> R<Term> {
        Ok(self.table(ty, m, span)?.len())
    }

    /// `for (k, v) in m` (`exec.rs`): die belegten Slots in ihrer Folge.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_map(
        &mut self,
        vars: (&str, TypeId, &str, TypeId),
        iter: &Expr,
        body: &Block,
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let v = self.value(iter, cx, env, flow)?;
        let entries = self.map_entries(iter.ty, v, span)?;
        self.unroll_steps(entries.len() as i64, span)?;
        let (k_loc, k_ty, v_loc, v_ty) = vars;
        self.breaks.push(Vec::new());
        for (inside, key, value) in entries {
            let mut env_k = env.clone();
            let mut fk = Flow::new(Term::and(vec![flow.alive.clone(), inside.clone()]));
            let alive = fk.alive.clone();
            self.put(&mut env_k, k_loc, k_ty, key, &alive, span)?;
            self.put(&mut env_k, v_loc, v_ty, value, &alive, span)?;
            self.block(body, cx, &mut env_k, &mut fk)?;
            *env = ite_env(&inside, &env_k, env);
            flow.exits.extend(fk.exits);
            flow.alive = Term::or(vec![Term::and(vec![flow.alive.clone(), inside.not()]), fk.alive]);
        }
        self.left_loop(flow);
        Ok(())
    }
}
