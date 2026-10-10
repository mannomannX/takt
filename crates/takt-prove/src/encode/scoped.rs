//! Gescopte Instanzen im Modell (5.11, `Sim::scoped_lifecycle`): Eine
//! Instanz laeuft, solange ihr Scope-Zustand in der Konfiguration ihres
//! Besitzers steht. Nach den Schritten eines Ticks tritt sie ein oder aus:
//! Eintritt frisch, mit `init_vars` und `initial` oder dem gemerkten Blatt
//! (`resume`, 5.12); Austritt mit den `exit:`-Bloecken von innen nach
//! aussen, ausser der Besitzer verliess den Zustand ueber einen Fault, dann
//! gehen ihre Outputs auf `safe`, ihre geplanten Ausgaben verfallen, und die
//! Konfiguration ist leer — das Blatt −1, veroeffentlicht als das Blatt,
//! das der naechste Eintritt betritt (`Sim::entry_leaf`).

use std::collections::BTreeMap;
use std::ops::Not;

use takt_diag::Span;
use takt_mir::machine::ScopedInstance;
use takt_mir::program::Direction;
use takt_mir::{MachineId, StateId};

use super::{Cx, Enc, Env, Flow, Mode, R, Target, ite_env};
use crate::encode::Unsupported;
use crate::term::Term;

impl Enc<'_> {
    /// Die gescopten Instanzen mit ihrem Besitzer, beide im Modell.
    pub(super) fn scoped(&self) -> Vec<(MachineId, ScopedInstance)> {
        takt_mir::machine::scoped_instances(self.p)
            .into_iter()
            .filter(|(owner, si)| self.order.contains(owner) && self.order.contains(&si.machine))
            .collect()
    }

    pub(super) fn is_scoped(&self, m: MachineId) -> bool {
        self.scoped().iter().any(|(_, si)| si.machine == m)
    }

    /// War der Scope zu Beginn des Ticks aktiv (`active_scoped`)?
    pub(super) fn loc_scope(&self, inst: MachineId) -> String {
        format!("s.{}.scoped", self.machine(inst).name)
    }

    /// Das Blatt, in das eine Instanz mit `resume` zurueckkehrt, sonst −1.
    fn loc_resumed(&self, inst: MachineId) -> String {
        format!("s.{}.resumed", self.machine(inst).name)
    }

    /// `m.state` liest zum Blatt −1 das Blatt, das der naechste Eintritt
    /// betritt (5.11, `Sim::entry_leaf`): das gemerkte bei `resume`, sonst
    /// das unter `initial`.
    pub(super) fn entry_leaf(&mut self, m: MachineId, leaf: Term, cx: &Cx<'_>, env: &Env, span: Span) -> R<Term> {
        let resumed = self.psi(cx, env, m, &self.loc_resumed(m), span)?;
        let machine = self.machine(m);
        let Some(&initial) = self.descend(m, machine.initial).last() else {
            return Err(Unsupported { what: "Instanz ohne Zustand".into(), span });
        };
        let entry = Term::ite(Term::eq(resumed.clone(), Term::int(-1)), Term::int(self.code(m, initial)), resumed);
        Ok(Term::ite(Term::eq(leaf.clone(), Term::int(-1)), entry, leaf))
    }

    /// Vor dem ersten Tick ist keine Instanz aktiv.
    pub(super) fn scoped_initial(&self, env: &mut Env) {
        for (_, si) in self.scoped() {
            env.insert(self.loc_scope(si.machine), Term::bool(false));
            env.insert(self.loc_resumed(si.machine), Term::int(-1));
        }
    }

    /// Steht der Zustand `scope` in der Konfiguration von `owner`?
    fn scope_now(&self, owner: MachineId, scope: StateId, env: &Env) -> Term {
        let leaf = env[&self.loc_leaf(owner)].clone();
        let leaves = self.leaves(owner).into_iter().filter(|l| self.chain_to(owner, *l).contains(&scope));
        Term::or(leaves.map(|l| Term::eq(leaf.clone(), Term::int(self.code(owner, l)))).collect())
    }

    /// Ein- und Austritte nach den Schritten (`scoped_lifecycle`); `faults`
    /// sagt je Maschine, ob sie in diesem Tick einen Fault-Pfad nahm.
    pub(super) fn scoped_lifecycle(
        &mut self,
        env: &mut Env,
        actives: &BTreeMap<MachineId, Term>,
        faults: &BTreeMap<MachineId, Term>,
    ) -> R<()> {
        for (owner, si) in self.scoped() {
            let inst = si.machine;
            let was = env[&self.loc_scope(inst)].clone();
            let now = self.scope_now(owner, si.scope, env);
            let enter = Term::and(vec![now.clone(), was.clone().not()]);
            let leave = Term::and(vec![now.clone().not(), was]);
            let by_fault = Term::or(vec![
                env[&self.loc_faulted(owner)].clone(),
                faults.get(&owner).cloned().unwrap_or_else(|| Term::bool(false)),
            ]);
            let base = env.clone();
            if !enter.is_bool(false) {
                let mut fresh = base.clone();
                self.enter_scoped(&si, &enter, &base, actives, &mut fresh)?;
                *env = ite_env(&enter, &fresh, env);
            }
            if !leave.is_bool(false) {
                let mut gone = base.clone();
                self.leave_scoped(&si, &leave, &by_fault, actives, &mut gone)?;
                *env = ite_env(&leave, &gone, env);
            }
            env.insert(self.loc_scope(inst), now);
        }
        Ok(())
    }

    /// `enter_scoped`: frisch, `init_vars`, die `persist`-Variablen von
    /// davor, dann `initial` oder das gemerkte Blatt.
    fn enter_scoped(
        &mut self,
        si: &ScopedInstance,
        under: &Term,
        base: &Env,
        actives: &BTreeMap<MachineId, Term>,
        env: &mut Env,
    ) -> R<()> {
        let m = si.machine;
        let machine = self.machine(m).clone();
        self.machine_defaults(m, env, true)?;
        let pre = env.clone();
        let cx = Cx { m: Some(m), leaf: None, mode: Mode::Entry, pre: &pre, active: actives, locals: None };
        self.init_vars(m, &cx, env, under)?;
        for pv in &machine.persist {
            let (loc, ty) = (self.loc_var(m, pv.var), machine.vars[pv.var.index()].ty);
            let kept = self.load(base, &loc, &self.shape(ty, Span::default())?, Span::default())?;
            self.init_loc(env, &loc, ty, kept, Span::default())?;
        }
        self.unrolled = 0;
        if !si.resume {
            return self.switch(&cx, None, Target::State(machine.initial), env, 0, under, false, true);
        }
        // 5.12: das gemerkte Blatt mit der Geschichte seiner `resume`-Zustaende.
        for s in (0..machine.states.len()).map(|i| StateId(i as u32)).filter(|s| machine.states[s.index()].resume) {
            let at = self.loc_saved(m, s);
            env.insert(at.clone(), base[&at].clone());
        }
        let resumed = base[&self.loc_resumed(m)].clone();
        let start = env.clone();
        let none = Term::and(vec![under.clone(), Term::eq(resumed.clone(), Term::int(-1))]);
        self.switch(&cx, None, Target::State(machine.initial), env, 0, &none, false, true)?;
        for leaf in self.leaves(m) {
            let hit = Term::eq(resumed.clone(), Term::int(self.code(m, leaf)));
            let mut branch = start.clone();
            self.unrolled = 0;
            let under = Term::and(vec![under.clone(), hit.clone()]);
            self.switch(&cx, None, Target::State(leaf), &mut branch, 0, &under, false, true)?;
            *env = ite_env(&hit, &branch, env);
        }
        env.insert(self.loc_resumed(m), Term::int(-1));
        Ok(())
    }

    /// `leave_scoped`: die `exit:`-Bloecke, wenn der Besitzer nicht ueber
    /// einen Fault ging — scheitert einer, entfallen die restlichen ohne
    /// Fault-Pfad —, dann `safe`, die Warteschlangen leer, die Konfiguration
    /// leer, die gehobenen Signale bis zum Tick-Ende; mit `resume` bleibt das
    /// Blatt gemerkt.
    fn leave_scoped(
        &mut self,
        si: &ScopedInstance,
        under: &Term,
        by_fault: &Term,
        actives: &BTreeMap<MachineId, Term>,
        env: &mut Env,
    ) -> R<()> {
        let m = si.machine;
        let machine = self.machine(m).clone();
        let leaf_now = env[&self.loc_leaf(m)].clone();
        let pre = env.clone();
        for leaf in self.leaves(m) {
            let is = Term::eq(leaf_now.clone(), Term::int(self.code(m, leaf)));
            let cond = Term::and(vec![under.clone(), by_fault.clone().not(), is]);
            if cond.is_bool(false) {
                continue;
            }
            let cx = Cx { m: Some(m), leaf: Some(leaf), mode: Mode::Entry, pre: &pre, active: actives, locals: None };
            let mut flow = Flow::new(cond);
            for s in self.chain_to(m, leaf).iter().rev() {
                self.block(&machine.states[s.index()].exit, &cx, env, &mut flow)?;
            }
        }
        for (i, c) in self.p.channels.iter().enumerate() {
            if c.dir != Direction::Output || c.owner != Some(m) {
                continue;
            }
            if let Some(safe) = c.attrs.safe.clone() {
                let v = self.const_value(None, &safe)?;
                let (loc, ty) = (self.loc_out(takt_mir::ChannelId(i as u32)), c.ty);
                self.init_loc(env, &loc, ty, v, safe.span)?;
            }
        }
        self.clear_schedules(m, env)?;
        let saved: Vec<(String, Term)> = (0..machine.states.len())
            .map(|i| StateId(i as u32))
            .filter(|s| machine.states[s.index()].resume)
            .map(|s| (self.loc_saved(m, s), env[&self.loc_saved(m, s)].clone()))
            .collect();
        // 5.11: Was `exit:` an Signalen hob, gilt in diesem Tick (FB-495).
        let raised: Vec<(String, Term)> =
            (0..machine.signals.len()).map(|i| (self.loc_sig(m, i), env[&self.loc_sig(m, i)].clone())).collect();
        self.machine_defaults(m, env, false)?;
        for (at, v) in raised {
            env.insert(at, v);
        }
        if si.resume {
            // 5.12: nach einem Fault-Uebergang des Besitzers nicht.
            let keep = by_fault.clone().not();
            env.insert(self.loc_resumed(m), Term::ite(keep.clone(), leaf_now, Term::int(-1)));
            for (at, old) in saved {
                let reset = env[&at].clone();
                env.insert(at, Term::ite(keep.clone(), old, reset));
            }
        }
        Ok(())
    }
}
