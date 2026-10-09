//! Geplante Ausgaben im Modell (9.8, `system::schedule`,
//! `image::apply_scheduled`): je eingeplantem Output `MAX_SCHED` Plaetze aus
//! Zeitpunkt und Wert. Ein gleicher Zeitpunkt ueberschreibt, sonst nimmt der
//! erste freie Platz den Eintrag; am Tick-Ende kommt der spaeteste faellige
//! Wert in den Latch, und die faelligen Plaetze werden frei. Die Folge der
//! Plaetze traegt keine Bedeutung, denn die Zeitpunkte einer Warteschlange
//! sind verschieden.

use std::ops::Not;

use takt_diag::Span;
use takt_interp::system::MAX_SCHED;
use takt_mir::machine::FaultKind;
use takt_mir::{ChannelId, MachineId};

use super::{Enc, Env, Exit, ExitKind, Flow, R};
use crate::term::{Op, Term};

/// Ein Platz einer Warteschlange: belegt, Zeitpunkt, Wert.
struct Slot {
    has: Term,
    t: Term,
    v: Term,
}

impl Enc<'_> {
    /// Die Outputs mit Warteschlange (`layout.output_queues`) und ihre Maschine.
    fn queues(&self) -> Vec<(MachineId, ChannelId)> {
        self.order.iter().flat_map(|&m| self.machine(m).layout.output_queues.iter().map(move |&c| (m, c))).collect()
    }

    fn loc_slot(&self, c: ChannelId, i: u32, part: &str) -> String {
        format!("s.sched.{}.{i}.{part}", self.p.channels[c.index()].name)
    }

    fn slots(&self, c: ChannelId, env: &Env) -> Vec<Slot> {
        (0..MAX_SCHED)
            .map(|i| Slot {
                has: env[&self.loc_slot(c, i, "has")].clone(),
                t: env[&self.loc_slot(c, i, "t")].clone(),
                v: env[&self.loc_slot(c, i, "v")].clone(),
            })
            .collect()
    }

    /// Schreibt die Plaetze unter `alive`; ein freier Platz ist null.
    fn store_slots(&self, c: ChannelId, slots: Vec<Slot>, alive: &Term, env: &mut Env) -> R<()> {
        let zero = Enc::zero(self.sort_of(self.p.channels[c.index()].ty, Span::default())?);
        for (i, s) in (0..MAX_SCHED).zip(slots) {
            let t = Term::ite(s.has.clone(), s.t, Term::int(0));
            let v = Term::ite(s.has.clone(), s.v, zero.clone());
            for (part, new) in [("has", s.has), ("t", t), ("v", v)] {
                let at = self.loc_slot(c, i, part);
                let old = env[&at].clone();
                env.insert(at, Term::ite(alive.clone(), new, old));
            }
        }
        Ok(())
    }

    /// Leere Warteschlangen vor dem ersten Tick.
    pub(super) fn sched_initial(&self, env: &mut Env) -> R<()> {
        for (_, c) in self.queues() {
            let zero = Enc::zero(self.sort_of(self.p.channels[c.index()].ty, Span::default())?);
            for i in 0..MAX_SCHED {
                env.insert(self.loc_slot(c, i, "has"), Term::bool(false));
                env.insert(self.loc_slot(c, i, "t"), Term::int(0));
                env.insert(self.loc_slot(c, i, "v"), zero.clone());
            }
        }
        Ok(())
    }

    /// Ein Schreibvorgang aus `at T:` (`schedule`): ein Zeitpunkt nicht in der
    /// Zukunft ist ein `TimingFault`, eine volle Warteschlange ein
    /// `ScheduleOverflow`.
    pub(super) fn schedule(
        &mut self,
        c: ChannelId,
        t: &Term,
        v: Term,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let fault = |fail: Term, kind: FaultKind, flow: &mut Flow| {
            flow.exits.push(Exit {
                cond: Term::and(vec![flow.alive.clone(), fail.clone()]),
                kind: ExitKind::Fault(None, self.cause(kind, span)),
            });
            flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        };
        fault(Term::bin(Op::Le, t.clone(), self.now.clone()), FaultKind::Timing, flow);
        let slots = self.slots(c, env);
        let same: Vec<Term> =
            slots.iter().map(|s| Term::and(vec![s.has.clone(), Term::eq(s.t.clone(), t.clone())])).collect();
        let found = Term::or(same.clone());
        let full = Term::and(slots.iter().map(|s| s.has.clone()).collect());
        fault(Term::and(vec![found.clone().not(), full]), FaultKind::ScheduleOverflow, flow);
        let mut before = Term::bool(true);
        let mut out = Vec::new();
        for (s, hit) in slots.into_iter().zip(same) {
            let first_free = Term::and(vec![found.clone().not(), s.has.clone().not(), before.clone()]);
            before = Term::and(vec![before, s.has.clone()]);
            let write = Term::or(vec![hit, first_free.clone()]);
            out.push(Slot {
                has: Term::or(vec![s.has, first_free.clone()]),
                t: Term::ite(first_free, t.clone(), s.t),
                v: Term::ite(write, v.clone(), s.v),
            });
        }
        let alive = flow.alive.clone();
        self.store_slots(c, out, &alive, env)
    }

    /// `cancel o`: die Warteschlange leer.
    pub(super) fn cancel_schedule(&self, c: ChannelId, alive: &Term, env: &mut Env) -> R<()> {
        let free = (0..MAX_SCHED).map(|_| Slot { has: Term::bool(false), t: Term::int(0), v: Term::int(0) }).collect();
        self.store_slots(c, free, alive, env)
    }

    /// Ein Fault-Pfad leert die Warteschlangen der Maschine (5.3).
    pub(super) fn clear_schedules(&self, m: MachineId, env: &mut Env) -> R<()> {
        for c in self.machine(m).layout.output_queues.clone() {
            self.cancel_schedule(c, &Term::bool(true), env)?;
        }
        Ok(())
    }

    /// `apply_scheduled(k)` vor dem Commit: der spaeteste faellige Wert in
    /// den Latch, die faelligen Plaetze frei.
    pub(super) fn apply_scheduled(&self, cur: &mut Env) -> R<()> {
        for (_, c) in self.queues() {
            let slots = self.slots(c, cur);
            let due: Vec<Term> = slots
                .iter()
                .map(|s| Term::and(vec![s.has.clone(), Term::bin(Op::Le, s.t.clone(), self.now.clone())]))
                .collect();
            let at = self.loc_out(c);
            let mut value = cur[&at].clone();
            for (j, (s, d)) in slots.iter().zip(&due).enumerate() {
                // Der spaeteste: kein anderer faelliger liegt spaeter.
                let later = slots
                    .iter()
                    .zip(&due)
                    .enumerate()
                    .filter(|(k, _)| *k != j)
                    .map(|(_, (o, od))| Term::and(vec![od.clone(), Term::bin(Op::Gt, o.t.clone(), s.t.clone())]));
                let latest = Term::and(vec![d.clone(), Term::or(later.collect()).not()]);
                value = Term::ite(latest, s.v.clone(), value);
            }
            cur.insert(at, value);
            let rest = slots
                .into_iter()
                .zip(due)
                .map(|(s, d)| Slot { has: Term::and(vec![s.has, d.not()]), t: s.t, v: s.v })
                .collect();
            self.store_slots(c, rest, &Term::bool(true), cur)?;
        }
        Ok(())
    }
}
