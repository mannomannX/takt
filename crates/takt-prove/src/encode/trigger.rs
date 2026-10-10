//! Trigger im Modell (7.5, `system::trigger_phase`): je Trigger ein
//! `armed` im Zustand seiner Maschine und ein eigener Cursor auf seinem
//! Quellstrom. Die Phase liegt ab dem ersten Tick zwischen Zustellung und
//! Schritten. Ein nicht armierter Trigger rueckt hinter das Fenster; ein
//! armierter nimmt das erste passende Element, disarmiert sich, plant seine
//! Ausgaben gegen die Reaktion `event.t + bound` und legt `event` in
//! `fired`. Ein Fault der Phase wird der armierenden Maschine vorgemerkt
//! (FB-484).

use std::collections::BTreeMap;
use std::ops::Not;

use takt_mir::expr::StreamRef;
use takt_mir::machine::{FaultKind, Guard};
use takt_mir::stmt::{Place, StmtKind};
use takt_mir::{MachineId, TriggerId};

use super::fault::Cause;
use super::stream::{OVERFLOW, Queued, stream_of};
use super::value::V;
use super::{Cx, Enc, Env, Exit, ExitKind, Flow, Mode, R, no};
use crate::term::{Op, Term};

impl Enc<'_> {
    /// Die Trigger einer Maschine (`layout.trigger_flags`).
    fn owned(&self, m: MachineId) -> Vec<TriggerId> {
        self.machine(m).layout.trigger_flags.clone()
    }

    pub(super) fn loc_armed(&self, m: MachineId, t: TriggerId) -> String {
        format!("s.{}.armed.{}", self.machine(m).name, self.p.triggers[t.index()].name)
    }

    fn loc_trigger_cursor(&self, t: TriggerId) -> String {
        format!("s.trig.{}.cur", self.p.triggers[t.index()].name)
    }

    /// Die Trigger, deren Maschine kodiert ist.
    fn encoded(&self) -> Vec<(TriggerId, MachineId)> {
        let owner = |i: usize| self.p.triggers[i].owner.filter(|m| self.order.contains(m));
        (0..self.p.triggers.len()).filter_map(|i| Some((TriggerId(i as u32), owner(i)?))).collect()
    }

    /// Eine Maschine vor ihrem Eintritt: keiner ihrer Trigger ist armiert.
    pub(super) fn triggers_initial(&self, m: MachineId, env: &mut Env) {
        for t in self.owned(m) {
            env.insert(self.loc_armed(m, t), Term::bool(false));
        }
    }

    /// Die Cursor der Trigger vor dem ersten Tick.
    pub(super) fn trigger_cursors_initial(&self, env: &mut Env) {
        for (t, _) in self.encoded() {
            env.insert(self.loc_trigger_cursor(t), Term::int(0));
        }
    }

    /// `arm` und `disarm` (7.5): Es armiert nur die Maschine, der der
    /// Trigger gehoert (Pruefung 55).
    pub(super) fn arm_trigger(&self, t: TriggerId, on: bool, cx: &Cx<'_>, env: &mut Env, flow: &Flow) -> R<()> {
        let span = self.p.triggers[t.index()].span;
        let Some(m) = cx.m.filter(|m| self.owned(*m).contains(&t)) else { return no("`arm` ohne Besitzer", span) };
        let at = self.loc_armed(m, t);
        let old = env[&at].clone();
        env.insert(at, Term::ite(flow.alive.clone(), Term::bool(on), old));
        Ok(())
    }

    /// `t.armed`: Zustand der Maschine, der der Trigger gehoert.
    pub(super) fn armed(&self, t: TriggerId, cx: &Cx<'_>, env: &Env) -> R<Term> {
        let span = self.p.triggers[t.index()].span;
        let Some(m) = cx.m.filter(|m| self.owned(*m).contains(&t)) else {
            return no("`armed` eines fremden Triggers", span);
        };
        Ok(env[&self.loc_armed(m, t)].clone())
    }

    /// Ein Fault-Pfad disarmiert die Trigger der Maschine (5.3, 7.5).
    pub(super) fn disarm_all(&self, m: MachineId, env: &mut Env) {
        self.triggers_initial(m, env);
    }

    /// Die vorgemerkten Ursachen einer Maschine mit ihrer Nummer in
    /// `pending`: der Ueberlauf eines Stroms, dann die der Trigger-Phase.
    pub(super) fn pending_causes(&self, m: MachineId) -> Vec<(i64, Cause)> {
        let overflow = (OVERFLOW, self.cause(FaultKind::StreamOverflow, takt_diag::Span::default()));
        let phase = self.trigger_faults.get(&m).into_iter().flatten().cloned();
        std::iter::once(overflow).chain(phase.enumerate().map(|(i, c)| (OVERFLOW + 1 + i as i64, c))).collect()
    }

    /// Merkt `cause` vor, wo `cond` gilt und noch nichts wartet.
    fn pend(&mut self, m: MachineId, cond: Term, cause: Cause, cur: &mut Env) {
        let list = self.trigger_faults.entry(m).or_default();
        list.push(cause);
        let code = OVERFLOW + list.len() as i64;
        let at = self.loc_pending(m);
        let old = cur[&at].clone();
        let free = Term::eq(old.clone(), Term::int(0));
        cur.insert(at, Term::ite(Term::and(vec![cond, free]), Term::int(code), old));
    }

    /// Die Trigger-Phase eines Ticks (`trigger_phase`, `trigger_fires`).
    pub(super) fn trigger_phase(&mut self, actives: &BTreeMap<MachineId, Term>, cur: &mut Env) -> R<()> {
        for (id, m) in self.encoded() {
            let t = self.p.triggers[id.index()].clone();
            let Guard::Match { subject, kind, pattern, .. } = &t.guard else {
                return no("Trigger ohne Muster", t.span);
            };
            let Some(key) = stream_of(subject) else { return no("Trigger ohne Quellstrom", subject.span) };
            let at_cur = self.loc_trigger_cursor(id);
            let w = self.window_from(key, Some(cur[&at_cur].clone()), subject.span)?;
            self.unroll_steps(w.items.len() as i64, subject.span)?;
            let armed = cur[&self.loc_armed(m, id)].clone();
            let pre = cur.clone();
            let cx = Cx { m: Some(m), leaf: None, mode: Mode::Run, pre: &pre, active: actives, locals: None };
            let fired_ty = self.p.streams[t.fired.index()].elem;
            let mut flow = Flow::new(armed.clone());
            let (mut found, mut seq, mut time) = (Term::bool(false), Term::int(0), Term::int(0));
            let mut event = self.zero_of(fired_ty, t.span)?;
            for item in &w.items {
                let (hit, caps) = self.hits(Some((*kind, pattern)), item, &cx, &pre, &mut flow, t.span)?;
                let first = Term::and(vec![armed.clone(), item.present.clone(), hit, found.clone().not()]);
                let v = self.binding(fired_ty, item, caps, t.span)?;
                event = V::ite(&first, v, event);
                seq = Term::ite(first.clone(), item.seq.clone(), seq);
                time = Term::ite(first.clone(), item.t.clone(), time);
                found = Term::or(vec![found, first]);
            }
            if !flow.exits.is_empty() {
                return no("ein Trigger-Muster, das faulten kann", t.span);
            }
            // Ohne Treffer ist das ganze Fenster gesehen; nicht armiert folgt
            // der Cursor dem Strom.
            let after = Term::bin(Op::Add, seq, Term::int(1));
            cur.insert(at_cur, Term::ite(found.clone(), after, w.end.clone()));
            cur.insert(self.loc_armed(m, id), Term::and(vec![armed, found.clone().not()]));
            // Zeit und Werte, dann die Reaktion, dann je Output der Eintrag.
            let mut plan = Flow::new(found.clone());
            self.event = Some(event.clone());
            let planned = self.trigger_plan(&t, &cx, &pre, &mut plan);
            self.event = None;
            let (at, writes) = planned?;
            let react = Term::bin(Op::Add, time, Term::int(t.bound));
            let late = Term::and(vec![plan.alive.clone(), Term::bin(Op::Lt, at.clone(), react)]);
            plan.exits.push(Exit {
                cond: late.clone(),
                kind: ExitKind::Fault(None, self.cause(FaultKind::Timing, t.time.span)),
            });
            plan.alive = Term::and(vec![plan.alive.clone(), late.not()]);
            for (c, v, span) in writes {
                self.enqueue(c, &at, v, cur, &mut plan, span)?;
            }
            for exit in plan.exits {
                let ExitKind::Fault(_, cause) = exit.kind else { return no("Trigger-Phase ohne Fault", t.span) };
                self.pend(m, exit.cond, cause, cur);
            }
            // `fired` traegt das Element, sichtbar im naechsten Tick (8.6).
            let si = self.stream_index(StreamRef::Internal(t.fired), t.span)?;
            self.queued.push(Queued { stream: si, cond: found, t: self.now.clone(), value: event });
        }
        Ok(())
    }

    /// Die Zeit und die Schreibvorgaenge des `then` mit `event` gebunden.
    fn trigger_plan(
        &mut self,
        t: &takt_mir::program::Trigger,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<TriggerPlan> {
        let at = self.expr(&t.time, cx, env, flow)?;
        let mut writes = Vec::new();
        for s in &t.then.stmts {
            let StmtKind::Assign { target: Place::Output(c), value } = &s.kind else {
                return no("`then` mit anderem als Output-Zuweisungen", s.span);
            };
            writes.push((*c, self.expr(value, cx, env, flow)?, s.span));
        }
        Ok((at, writes))
    }
}

/// Zeit und Schreibvorgaenge eines Triggers (7.5).
type TriggerPlan = (Term, Vec<(takt_mir::ChannelId, Term, takt_diag::Span)>);
