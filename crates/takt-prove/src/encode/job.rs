//! Jobs im Modell (4.5, `machine::JobRun`): `job v = f(args)` rechnet die
//! reine Funktion sofort, und ihr Ergebnis gilt ab der ersten Aktivierung
//! der Maschine, deren Tick die Faelligkeit erreicht. Ein neuer Lauf ersetzt
//! einen laufenden, ein Fault-Pfad bricht die Laeufe mit `CANCELLED` ab.
//!
//! Die Faelligkeit ist der Modell-Tick `start + ceil(duration / T0)`, den
//! eine Aufzeichnung im Stimulus verlegt: `job_start` nimmt die frueheste
//! Aufzeichnung ab dem Modell-Tick. Je Start waehlt der Solver die
//! Verspaetung frei; liegt die Faelligkeit des vorigen Laufs nicht vor dem
//! neuen Modell-Tick, ist sie wie im Interpreter auch die des neuen, denn
//! dieselbe Aufzeichnung gilt fuer beide. So ergibt jede Wahl einen Stimulus,
//! und jeder Stimulus eine Wahl.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{Expr, JobField};
use takt_mir::{MachineId, NativeId, TypeId, VarId};

use super::value::V;
use super::{Cx, Enc, Env, Flow, R, no};
use crate::term::{Op, Sort, Term};

/// `JobErr` im Prelude: CANCELLED = 0, PENDING = 2 (`JobRun::err`).
const CANCELLED: i64 = 0;
const PENDING: i64 = 2;

/// Die groesste Verspaetung in Nanosekunden: Mit ihr bleibt die
/// Faelligkeit ab `now` in 64 Bit.
const LATEST: i64 = 1 << 62;

impl Enc<'_> {
    /// Ort eines Teils eines Job-Slots: `s.<maschine>.job.<handle>.<teil>`.
    pub(super) fn loc_job(&self, m: MachineId, slot: usize, part: &str) -> String {
        let machine = self.machine(m);
        let handle = &machine.vars[machine.layout.job_slots[slot].handle.index()].name;
        format!("s.{}.job.{handle}.{part}", machine.name)
    }

    fn job_slot(&self, m: MachineId, handle: VarId, span: Span) -> R<usize> {
        match self.machine(m).layout.job_slots.iter().position(|s| s.handle == handle) {
            Some(i) => Ok(i),
            None => no("Job-Handle ohne Slot", span),
        }
    }

    /// Die Dauer im Modell, `ceil(duration / T0)` Ticks, in Nanosekunden.
    fn job_span(&self, native: NativeId) -> i64 {
        let tick = self.p.config.tick.max(1);
        let d = self.p.natives[native.index()].duration.unwrap_or(0).max(0);
        (d.saturating_add(tick - 1) / tick).saturating_mul(tick)
    }

    fn job_ret(&self, m: MachineId, slot: usize) -> TypeId {
        self.p.natives[self.machine(m).layout.job_slots[slot].native.index()].ret
    }

    /// Die Slots einer Maschine vor dem ersten Lauf; `due` −1 liegt vor
    /// jedem Modell-Tick.
    pub(super) fn jobs_initial(&self, m: MachineId, env: &mut Env) -> R<()> {
        for i in 0..self.machine(m).layout.job_slots.len() {
            for part in ["running", "done", "cancelled"] {
                env.insert(self.loc_job(m, i, part), Term::bool(false));
            }
            env.insert(self.loc_job(m, i, "due"), Term::int(-1));
            let ret = self.job_ret(m, i);
            self.init_loc(
                env,
                &self.loc_job(m, i, "value"),
                ret,
                self.zero_of(ret, Span::default())?,
                Span::default(),
            )?;
        }
        Ok(())
    }

    /// `job v = f(args)` (`exec.rs`, `MachineEnv::job_start`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn job_start(
        &mut self,
        handle: VarId,
        native: NativeId,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let Some(m) = cx.m else { return no("Job ausserhalb einer Maschine", span) };
        let slot = self.job_slot(m, handle, span)?;
        let mut values = Vec::new();
        for a in args {
            values.push(self.value(a, cx, env, flow)?);
        }
        let value = self.native_value(native, values, span)?;
        let tick = self.p.config.tick.max(1);
        let name = format!("i.job.{}.{}.delay", self.machine(m).name, self.machine(m).vars[handle.index()].name);
        if !self.inputs.contains_key(&name) {
            let late = Term::var(name.clone(), Sort::Int);
            self.job_assumptions.push(Term::and(vec![
                Term::bin(Op::Ge, late.clone(), Term::int(0)),
                Term::bin(Op::Le, late, Term::int(LATEST / tick)),
            ]));
        }
        let late = self.input(name, Sort::Int);
        let modelled = Term::bin(Op::Add, self.now.clone(), Term::int(self.job_span(native)));
        let prev = env[&self.loc_job(m, slot, "due")].clone();
        let due = Term::ite(
            Term::bin(Op::Ge, prev.clone(), modelled.clone()),
            prev,
            Term::bin(Op::Add, modelled, Term::bin(Op::Mul, late, Term::int(tick))),
        );
        let alive = flow.alive.clone();
        for (part, new) in
            [("running", Term::bool(true)), ("done", Term::bool(false)), ("cancelled", Term::bool(false)), ("due", due)]
        {
            let at = self.loc_job(m, slot, part);
            let old = env[&at].clone();
            env.insert(at, Term::ite(alive.clone(), new, old));
        }
        let ret = self.job_ret(m, slot);
        self.put(env, &self.loc_job(m, slot, "value"), ret, value, &alive, span)
    }

    /// `poll_jobs` zu Beginn einer Aktivierung, auch einer gefaulteten
    /// Maschine: Ein faelliger Lauf endet.
    pub(super) fn jobs_poll(&self, m: MachineId, active: &Term, env: &mut Env) {
        for i in 0..self.machine(m).layout.job_slots.len() {
            let running = env[&self.loc_job(m, i, "running")].clone();
            let due = env[&self.loc_job(m, i, "due")].clone();
            let fire = Term::and(vec![active.clone(), running.clone(), Term::bin(Op::Le, due, self.now.clone())]);
            env.insert(self.loc_job(m, i, "running"), Term::and(vec![running, fire.clone().not()]));
            let done = env[&self.loc_job(m, i, "done")].clone();
            env.insert(self.loc_job(m, i, "done"), Term::or(vec![done, fire]));
        }
    }

    /// Ein Fault-Pfad bricht die laufenden Jobs der Maschine ab (5.3).
    pub(super) fn jobs_cancel(&self, m: MachineId, env: &mut Env) {
        for i in 0..self.machine(m).layout.job_slots.len() {
            let running = env[&self.loc_job(m, i, "running")].clone();
            env.insert(self.loc_job(m, i, "running"), Term::bool(false));
            for part in ["done", "cancelled"] {
                let at = self.loc_job(m, i, part);
                let old = env[&at].clone();
                env.insert(at, Term::or(vec![old, running.clone()]));
            }
        }
    }

    /// `v.done` und `v.result` (`MachineEnv::job`): `Err(PENDING)` vor der
    /// Fertigstellung, `Err(CANCELLED)` nach einem Abbruch, sonst
    /// `Ok(Ergebnis)`.
    pub(super) fn job_state(
        &mut self,
        handle: VarId,
        field: JobField,
        ty: TypeId,
        cx: &Cx<'_>,
        env: &Env,
        span: Span,
    ) -> R<V> {
        let Some(m) = cx.m else { return no("Job ausserhalb einer Maschine", span) };
        let i = self.job_slot(m, handle, span)?;
        let done = env[&self.loc_job(m, i, "done")].clone();
        if matches!(field, JobField::Done) {
            return Ok(V::Leaf(done));
        }
        let failed = Term::or(vec![done.clone().not(), env[&self.loc_job(m, i, "cancelled")].clone()]);
        let ret = self.job_ret(m, i);
        let value = self.load(env, &self.loc_job(m, i, "value"), &self.shape(ret, span)?, span)?;
        let V::Node(mut parts) = self.zero_of(ty, span)? else { return no("Ergebnis eines Jobs", span) };
        let zero = parts[1].clone();
        parts[0] = V::Leaf(failed.clone());
        parts[1] = V::ite(&failed, zero, value);
        parts[2] = V::Leaf(Term::ite(done, Term::int(CANCELLED), Term::int(PENDING)));
        Ok(V::Node(parts))
    }

    /// Ein Lauf ist nicht zugleich fertig; abgebrochen ist nur ein fertiger.
    pub(super) fn job_invariants(&self, m: MachineId, pre: &Env, out: &mut Vec<Term>) {
        for i in 0..self.machine(m).layout.job_slots.len() {
            let running = pre[&self.loc_job(m, i, "running")].clone();
            let done = pre[&self.loc_job(m, i, "done")].clone();
            let cancelled = pre[&self.loc_job(m, i, "cancelled")].clone();
            out.push(Term::or(vec![running.not(), done.clone().not()]));
            out.push(Term::or(vec![cancelled.not(), done]));
            self.typed_loc(pre, &self.loc_job(m, i, "value"), self.job_ret(m, i), out);
        }
    }
}
