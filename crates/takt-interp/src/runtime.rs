//! Der Interpreter in der Schleife aus 12.1: Ein [`Run`] ist ein
//! `takt_rt_core::Program`, und die Schleife darf ihn schlafen lassen (9.9).
//!
//! **Satz 9.9.1** verlangt, dass der Trace mit Schlaf derselbe ist wie
//! ohne. Die Bedingung ist die aus 9.9; die Frist ist die frueheste
//! `after`-Frist ueber alle Maschinen — und die naechste Stimuluszeile,
//! weil der Stimulus hier der Rand ist: Ein Ereignis vom Rand weckt, und
//! frueher zu wecken als noetig ist immer richtig, denn die
//! uebersprungenen Ticks waeren leere Schritte gewesen. Ein Sendepuffer,
//! den der Treiber noch leert, haelt wach: Sein Abholen steht im Trace.
//! Und eine Wake-Quelle, deren Abtastung veraltet (3.5), weckt ebenfalls:
//! Der Guard, der sie liest, faultet in genau diesem Tick — mit oder
//! ohne Schlaf.

use takt_mir::MachineId;
use takt_mir::expr::StreamRef;
use takt_mir::machine::TransTrigger;
use takt_mir::program::Direction;

use crate::env::MachineEnv;
use crate::run::{Ended, Run, observe_properties};
use crate::system::Sim;
use crate::value::Quality;

impl Run<'_> {
    /// `sleep_allowed()` aus 9.9, dazu ein leerer Sendepuffer (8.8).
    pub fn may_sleep(&self) -> bool {
        let sim = &self.sim;
        let p = sim.loaded.program;
        if self.ended != Ended::Ticks || self.trap.is_some() {
            return false;
        }
        let quiet = |id: MachineId| {
            let s = &sim.states[id.index()];
            let windows_empty = p.machines[id.index()].layout.cursors.iter().enumerate().all(|(i, r)| {
                let StreamRef::Channel(c) = *r else { return true };
                !p.channels[c.index()].attrs.wake
                    || sim.image.channel_bufs.get(&c).is_none_or(|b| b.window(s.cursors[i]).is_empty())
            });
            sim.is_idle(id)
                && s.pending.is_none()
                && s.raised.is_none()
                && s.jobs.iter().all(|j| j.due.is_none())
                && windows_empty
        };
        sim.order.iter().all(|&id| quiet(id))
            && sim.image.sched.values().all(Vec::is_empty)
            && sim.image.tx.values().all(|t| t.queued.is_empty() && t.sent.is_empty())
    }

    /// Die frueheste Frist (9.9) als logischer Zeitpunkt in Nanosekunden:
    /// `after` in den aktiven Zustaenden, die naechste Stimuluszeile, das
    /// Ende des Laufs. `None`, wenn eine Frist nicht auswertbar ist — dann
    /// schlaeft die Schleife nicht.
    pub(crate) fn earliest_deadline(&mut self) -> Option<i64> {
        let now = self.sim.tick;
        let tick_ns = self.sim.loaded.program.config.tick;
        let mut best = self.ticks;
        for t in self.stimulus.lines.iter().map(|l| l.tick) {
            if t > now {
                best = best.min(t);
            }
        }
        let program = self.sim.loaded.program;
        for (i, c) in program.channels.iter().enumerate() {
            let s = &self.sim.image.inputs[i];
            let (Some(max), true) = (c.attrs.max_age, c.dir == Direction::Input && c.attrs.wake) else { continue };
            if matches!(s.quality, Quality::Bad | Quality::Stale) {
                continue;
            }
            // `age_inputs` laeuft vor jedem Tick: veraltet ist die Abtastung
            // im ersten Tick mit `age > max_age`.
            let left = max.saturating_sub(s.age).max(-1);
            best = best.min(now.saturating_add(u64::try_from(left / tick_ns.max(1) + 1).unwrap_or(1)));
        }
        let Sim { loaded, states, image, order, .. } = &mut self.sim;
        let mut scratch = Vec::new();
        for &id in order.iter() {
            let m = &program.machines[id.index()];
            let state = &mut states[id.index()];
            let (countdown, chain, timers) = (u64::from(state.countdown), state.conf.clone(), state.timers.clone());
            let mut env = MachineEnv::new(loaded, id, state, image, &mut scratch, tick_ns);
            for s in &chain {
                for t in &m.states[s.index()].transitions {
                    let TransTrigger::After(d) = &t.trigger else { continue };
                    let ns = env.ctx(loaded, now).eval_duration(d).ok()?;
                    // Wie `take_transition`: die erste Aktivierung mit
                    // `time_in_state >= d`, nie der Entry-Tick (7.1).
                    let step = i64::from(m.period).saturating_mul(tick_ns).max(1);
                    let need = ((ns + step - 1) / step).max(1);
                    let have = i64::try_from(timers[s.index()]).unwrap_or(i64::MAX);
                    let j = u64::try_from(need - have).unwrap_or(0);
                    best = best.min(now + 1 + countdown + j * u64::from(m.period));
                }
            }
        }
        Some(i64::try_from(best).unwrap_or(i64::MAX).saturating_mul(tick_ns))
    }

    /// Traegt `n` uebersprungene Ticks nach (9.9): Alterung, Zaehler und
    /// Timer wie in leeren Schritten, die Monitore sehen jeden Tick (13.3);
    /// `every`-Zaehler bleiben, wie 9.9 es sagt.
    pub fn skip(&mut self, n: u64) {
        for _ in 0..n {
            self.sim.age();
            self.sim.skip_tick();
            let tick = self.sim.tick;
            observe_properties(&mut self.monitors, &self.sim, tick, &mut self.writer, &mut self.fail);
            self.last = tick;
        }
        self.deadline = self.earliest_deadline();
    }
}

impl takt_rt_core::Program for Run<'_> {
    /// Tick `k` der Schleife ist Tick `k + 1` des Laufs: Tick 0 hat `new`
    /// ausgefuehrt (9.4). `now` rechnet der Lauf selbst aus dem Tick.
    fn tick(&mut self, k: u64, _now: i64) {
        if self.trap.is_none() {
            if let Err(trap) = Run::tick(self, k.saturating_add(1)) {
                self.trap = Some(trap);
            }
        }
        self.deadline = self.earliest_deadline();
    }

    fn sleep_allowed(&self) -> bool {
        self.may_sleep()
    }

    fn next_deadline(&self) -> Option<i64> {
        self.deadline
    }

    fn advance(&mut self, ticks: u64) {
        self.skip(ticks);
    }
}
