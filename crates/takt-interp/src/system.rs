//! System-Tick (Referenz 9.4): `tick(k)` mit Abtastung, Aktivierung,
//! Schrittphase, Abort-Phase, Veroeffentlichung und Commit; dazu die
//! Umgebung einer Maschine ueber dem Prozessabbild.

use std::collections::HashMap;

use takt_diag::Span;

use takt_mir::expr::{Accessor, Builtin, JobField, StreamRef};
use takt_mir::machine::{FaultKind, Machine, VarScope};
use takt_mir::program::{OutputTiming, Overflow, Program};
use takt_mir::types::Type;
use takt_mir::*;

use crate::env::{MachineEnv, Observation, Outer};
use crate::eval::Ctx;
use crate::image::Image;
use crate::loaded::Loaded;
use crate::machine::{self, MachineState};
use crate::nvm::{Load, Nvm};
use crate::stream::{Delivery, Element};
use crate::value::{EvalResult, Fault, Sample, Trap, Value, bug};
use takt_mir::TypeId;
use takt_mir::analysis::schedule;

impl<'a, 'p> MachineEnv<'a, 'p> {
    /// Umgebung einer Maschine fuer diesen Tick.
    pub fn new(
        loaded: &'a Loaded<'p>,
        id: MachineId,
        state: &'a mut MachineState,
        image: &'a mut Image,
        out: &'a mut Vec<Observation>,
        tick_ns: i64,
    ) -> Self {
        MachineEnv { loaded, id, state, image, out, tick_ns, tick: 0, aborted: false, steps: None }
    }

    /// Sammelt die ausgefuehrten Anweisungen (`takt sim --steps`).
    pub fn with_steps(mut self, steps: &'a mut Vec<crate::env::Step>) -> Self {
        self.steps = Some(steps);
        self
    }

    /// Die Maschine.
    pub fn machine<'q>(&self, loaded: &'q Loaded<'_>) -> &'q Machine {
        &loaded.program.machines[self.id.index()]
    }

    /// Auswertungskontext.
    pub fn ctx<'c, 'q>(&'c mut self, loaded: &'c Loaded<'q>, tick: u64) -> Ctx<'q, 'c> {
        self.tick = tick;
        Ctx::new(loaded, self, tick)
    }

    /// Initialisiert die Variablen der Maschine (Tick 0).
    pub fn init_vars(&mut self, loaded: &Loaded<'_>, tick: u64) -> Result<(), Trap> {
        let defs = self.machine(loaded).vars.clone();
        self.state.vars = vec![Value::Bool(false); defs.len()];
        for (i, def) in defs.iter().enumerate() {
            if !matches!(def.scope, VarScope::Machine | VarScope::Param) {
                self.state.vars[i] = Value::default_for(def.ty, loaded.program);
                continue;
            }
            let value = match &def.init {
                Some(e) => {
                    let mut ctx = self.ctx(loaded, tick);
                    ctx.eval(e)?
                }
                None => Value::default_for(def.ty, loaded.program),
            };
            self.state.vars[i] = value;
        }
        Ok(())
    }

    /// Betritt einen Zustand: zustandslokale und gehobene Variablen, Timer,
    /// `every`-Zaehler und `viol`-Zaehler zuruecksetzen (9.3, Schritt 3).
    pub fn enter_state(&mut self, loaded: &Loaded<'_>, s: StateId, tick: u64) -> Result<(), Trap> {
        let m = self.machine(loaded);
        let vars: Vec<VarId> = m.states[s.index()].vars.clone();
        let counters: Vec<usize> =
            m.layout.every_counters.iter().enumerate().filter(|(_, c)| c.state == Some(s)).map(|(i, _)| i).collect();
        let sites: Vec<usize> =
            m.layout.viol_sites.iter().enumerate().filter(|(_, c)| c.state == Some(s)).map(|(i, _)| i).collect();
        self.state.timers[s.index()] = 0;
        self.state.every_next.reset(&counters);
        self.state.viol.reset(&sites);
        for v in vars {
            let def = self.machine(loaded).vars[v.index()].clone();
            let value = match &def.init {
                Some(e) => {
                    let mut ctx = self.ctx(loaded, tick);
                    ctx.eval(e)?
                }
                None => Value::default_for(def.ty, loaded.program),
            };
            self.state.vars[v.index()] = value;
        }
        Ok(())
    }

    /// Setzt alle Outputs der Maschine auf ihren `safe`-Wert (5.3).
    pub fn safe_outputs(&mut self, loaded: &Loaded<'_>) {
        let owned: Vec<(ChannelId, Option<takt_mir::expr::Expr>)> = loaded
            .program
            .channels
            .iter()
            .enumerate()
            .filter(|(_, c)| c.owner == Some(self.id))
            .map(|(i, c)| (ChannelId(i as u32), c.attrs.safe.clone()))
            .collect();
        for (c, safe) in owned {
            let value = match safe {
                Some(e) => {
                    let mut ctx = self.ctx(loaded, self.tick);
                    match ctx.eval(&e) {
                        Ok(v) => v,
                        Err(_) => continue,
                    }
                }
                None => continue,
            };
            *self.image.output_mut(c) = value;
        }
    }

    /// Fenster eines Stroms fuer diese Aktivierung (9.6, `windows`). Im
    /// Entry-Modus ist es leer — dort laufen keine Handler (8.7).
    pub fn window(&self, loaded: &Loaded<'_>, stream: StreamRef) -> Vec<Element> {
        let cursor = self.cursor_of(loaded, stream);
        match stream {
            StreamRef::Channel(c) => self.image.channel_bufs.get(&c).map(|b| b.window(cursor)).unwrap_or_default(),
            StreamRef::Internal(s) => {
                self.image.stream_bufs.get(s.index()).map(|b| b.window(cursor)).unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// Cursor dieser Maschine auf einem Strom (`cur[s, m]`, 9.6).
    pub fn cursor_of(&self, loaded: &Loaded<'_>, stream: StreamRef) -> i64 {
        let m = &loaded.program.machines[self.id.index()];
        match m.layout.cursors.iter().position(|r| *r == stream) {
            Some(i) => self.state.cursors.get(i).copied().unwrap_or(0),
            None => 0,
        }
    }

    /// `dropped[s, m]`: was diese Maschine im Schlaf missed hat (5.10).
    pub fn dropped_of(&self, loaded: &Loaded<'_>, stream: StreamRef) -> u32 {
        let m = &loaded.program.machines[self.id.index()];
        match m.layout.cursors.iter().position(|r| *r == stream) {
            Some(i) => self.state.dropped.get(i).copied().unwrap_or(0),
            None => 0,
        }
    }

    /// Merkt ein Element als untersucht (9.6, „untersucht heisst
    /// konsumiert"): `examined` ist das Maximum ueber alle Konstrukte der
    /// Aktivierung.
    pub fn mark_examined(&mut self, loaded: &Loaded<'_>, stream: StreamRef, seq: i64) {
        let m = &loaded.program.machines[self.id.index()];
        if let Some(i) = m.layout.cursors.iter().position(|r| *r == stream) {
            if let Some(e) = self.state.examined.get_mut(i) {
                *e = (*e).max(seq);
            }
        }
    }

    /// Baut den Bindungsrecord eines Elements: die Captures, gefolgt von
    /// `.t`, `.seq` und `.text`/`.data` (8.7).
    /// Wie [`Self::element_record`], aber mit dem Elementtyp eines Stroms
    /// statt dem einer Bindung (7.5: `event` und `fired` teilen ihn).
    pub fn element_record_for(
        &mut self,
        loaded: &Loaded<'_>,
        stream: StreamId,
        element: &Element,
        caps: Vec<Value>,
    ) -> Result<Value, Trap> {
        let ty = loaded.program.streams[stream.index()].elem;
        let Type::Record(r) = loaded.ty(ty) else {
            return bug("`fired` ohne Recordtyp");
        };
        let defs = loaded.program.records[r.index()].fields.clone();
        let mut fields = caps;
        for def in defs.iter().skip(fields.len()) {
            let v = match def.name.as_str() {
                "t" => Value::Duration(element.t),
                "seq" => Value::Int(element.seq),
                "text" | "data" => element.value.clone(),
                _ => Value::default_for(def.ty, loaded.program),
            };
            fields.push(v);
        }
        fields.truncate(defs.len());
        Ok(Value::Record(fields))
    }

    /// Baut den Bindungsrecord eines Elements aus dem Typ einer Bindung.
    pub fn element_record(
        &mut self,
        loaded: &Loaded<'_>,
        var: VarId,
        element: &Element,
        caps: Vec<Value>,
    ) -> Result<Value, Trap> {
        let m = &loaded.program.machines[self.id.index()];
        let ty = m.vars.get(var.index()).map(|v| v.ty).ok_or_else(|| Trap::Bug("Bindung fehlt".into()))?;
        let Type::Record(r) = loaded.ty(ty) else {
            return bug(format!("Bindung {} ist kein Record", var.0));
        };
        let defs = loaded.program.records[r.index()].fields.clone();
        // Die Bindung ist ein Wrapper (8.7): hinter den Captures stehen
        // `t`, `seq` und der Inhalt unter einem Namen. Felder eines
        // Record-Elements sind darunter erreichbar, nicht daneben.
        let mut fields = caps;
        for def in defs.iter().skip(fields.len()) {
            let v = match def.name.as_str() {
                "t" => Value::Duration(element.t),
                "seq" => Value::Int(element.seq),
                "text" | "data" => element.value.clone(),
                _ => Value::default_for(def.ty, loaded.program),
            };
            fields.push(v);
        }
        fields.truncate(defs.len());
        Ok(Value::Record(fields))
    }

    /// Der Slot eines Job-Handles (`Layout::job_slots`).
    fn job_slot(&self, handle: VarId) -> EvalResult<usize> {
        self.machine(self.loaded)
            .layout
            .job_slots
            .iter()
            .position(|s| s.handle == handle)
            .ok_or_else(|| Trap::Bug(format!("Job-Handle {} ohne Slot", handle.0)))
    }

    /// Meldet einen Fault als Beobachtung.
    pub fn observe_fault(&mut self, f: &Fault, target: String) {
        self.out.push(Observation::Fault { kind: f.kind, message: f.message.clone(), target });
    }

    /// `last_fault` als Recordwert (5.3).
    pub fn last_fault_value(&self, loaded: &Loaded<'_>) -> Value {
        let record = loaded.program.records.iter().position(|r| r.name == "LastFault");
        let Some(r) = record else { return Value::Bool(false) };
        let def = &loaded.program.records[r];
        let f = self.state.last_fault.clone();
        let kind = match &f {
            Some(f) => fault_kind_value(loaded, f.kind),
            None => Value::Enum { variant: 0, fields: Vec::new() },
        };
        let message = f.as_ref().map(|f| f.message.clone()).unwrap_or_default();
        let line = f.as_ref().map_or(0, |f| i64::from(f.span.start));
        let tick = f.as_ref().map_or(0, |f| i64::try_from(f.tick).unwrap_or(0));
        let mut fields = vec![kind, Value::Str(message), Value::Int(line), Value::Int(tick)];
        fields.truncate(def.fields.len());
        while fields.len() < def.fields.len() {
            fields.push(Value::default_for(def.fields[fields.len()].ty, loaded.program));
        }
        Value::Record(fields)
    }

    /// Zustand als Variante des Zustandstyps (Festlegung 4).
    pub fn state_value(&self, loaded: &Loaded<'_>) -> Value {
        let m = self.machine(loaded);
        let name = self.state.path(m);
        let leaf = name.rsplit('.').next().unwrap_or(&name);
        let variant = loaded
            .program
            .enums
            .iter()
            .find(|e| e.name == format!("{}.State", m.name))
            .and_then(|e| e.variants.iter().position(|v| v.name == leaf))
            .unwrap_or(0);
        Value::Enum { variant: variant as u32, fields: Vec::new() }
    }
}

fn fault_kind_value(loaded: &Loaded<'_>, kind: FaultKind) -> Value {
    let Some(e) = loaded.program.enums.iter().find(|e| e.name == "FaultKind") else {
        return Value::Enum { variant: 0, fields: Vec::new() };
    };
    let name = match kind {
        FaultKind::CheckFailed => "CHECK_FAILED",
        FaultKind::Expect => "EXPECT",
        FaultKind::Timeout => "TIMEOUT",
        FaultKind::SensorFault => "SENSOR_FAULT",
        FaultKind::MissingValue => "MISSING_VALUE",
        FaultKind::Arithmetic(_) => "ARITHMETIC",
        FaultKind::Range => "RANGE",
        FaultKind::StreamOverflow => "STREAM_OVERFLOW",
        FaultKind::Timing => "TIMING",
        FaultKind::ScheduleOverflow => "SCHEDULE_OVERFLOW",
        FaultKind::JobOverflow => "JOB_OVERFLOW",
        FaultKind::Abort => "ABORT",
        FaultKind::Runtime(_) => "RUNTIME",
    };
    let variant = e.variants.iter().position(|v| v.name == name).unwrap_or(0);
    let fields = match kind {
        FaultKind::Arithmetic(k) => vec![Value::Enum { variant: k as u32, fields: Vec::new() }],
        FaultKind::Runtime(k) => vec![Value::Enum { variant: k as u32, fields: Vec::new() }],
        _ => Vec::new(),
    };
    Value::Enum { variant: variant as u32, fields }
}

impl Outer for MachineEnv<'_, '_> {
    fn var(&self, v: VarId) -> EvalResult<&Value> {
        self.state.vars.get(v.index()).ok_or_else(|| Trap::Bug(format!("Variable {} fehlt", v.0)))
    }

    fn var_mut(&mut self, v: VarId) -> EvalResult<&mut Value> {
        self.state.vars.get_mut(v.index()).ok_or_else(|| Trap::Bug(format!("Variable {} fehlt", v.0)))
    }

    fn param(&self, p: ParamId) -> EvalResult<&Value> {
        self.image.params.get(p.index()).ok_or_else(|| Trap::Bug(format!("Parameter {} fehlt", p.0)))
    }

    fn command(&self, c: CommandId) -> EvalResult<bool> {
        Ok(self.image.command(c))
    }

    fn input(&self, c: ChannelId) -> EvalResult<&Sample> {
        Ok(self.image.input(c))
    }

    fn output(&self, c: ChannelId) -> EvalResult<&Value> {
        // Die besitzende Maschine liest ihren Latch, jede andere den
        // committeten Wert des vorigen Ticks (Unit-Delay, 8.3).
        let owner = self.loaded.program.channels[c.index()].owner;
        if owner == Some(self.id) { Ok(self.image.output(c)) } else { Ok(self.image.committed_output(c)) }
    }

    fn output_mut(&mut self, c: ChannelId) -> EvalResult<&mut Value> {
        Ok(self.image.output_mut(c))
    }

    // 7.2: Ein Follower liest eine gefolgte Maschine frisch, wenn sie in
    // diesem Tick schon gelaufen ist; sonst — und in der Abort-Phase — Ψ_k.
    fn published(&self, m: MachineId, v: VarId) -> EvalResult<&Value> {
        let fresh = self.image.fresh(m) && self.machine(self.loaded).follows.contains(&m);
        self.image
            .published_var(m, v, fresh)
            .ok_or_else(|| Trap::Bug(format!("`pub var` {} von Maschine {} fehlt", v.0, m.0)))
    }

    fn job(&self, handle: VarId, field: JobField) -> EvalResult<Value> {
        let slot = self.job_slot(handle)?;
        let job = self.state.jobs.get(slot).ok_or_else(|| Trap::Bug(format!("Job-Slot {slot} fehlt")))?;
        Ok(match field {
            JobField::Done => Value::Bool(job.done),
            JobField::Result => job.result.clone(),
        })
    }

    fn job_start(&mut self, handle: VarId, value: Value, due: u64) -> EvalResult<()> {
        let slot = self.job_slot(handle)?;
        // 4.5: Eine Aufzeichnung ersetzt den Tick des Modells.
        let (id, tick) = (self.id, self.tick);
        let recorded =
            self.image.job_records.iter().filter(|(m, s, t)| *m == id && *s == slot && *t >= tick).map(|r| r.2).min();
        let due = recorded.unwrap_or(due);
        self.state.jobs.get_mut(slot).ok_or_else(|| Trap::Bug(format!("Job-Slot {slot} fehlt")))?.start(value, due);
        Ok(())
    }

    fn state_of(&self, m: MachineId) -> EvalResult<Value> {
        let fresh = self.image.fresh(m) && self.machine(self.loaded).follows.contains(&m);
        self.image
            .published_state(m, fresh)
            .cloned()
            .ok_or_else(|| Trap::Bug(format!("Zustand von Maschine {} fehlt", m.0)))
    }

    fn signal(&self, m: MachineId, s: SignalId) -> EvalResult<bool> {
        let fresh = self.image.fresh(m) && self.machine(self.loaded).follows.contains(&m);
        Ok(self.image.published_signal(m, s, fresh))
    }

    fn viol(&mut self, site: SiteId, index: &[i64]) -> EvalResult<&mut i64> {
        Ok(self.state.viol.at(site.0, index))
    }

    fn every(&mut self, counter: CounterId, index: &[i64], start: i64) -> EvalResult<&mut i64> {
        Ok(self.state.every_next.at_or(counter.0, index, start))
    }

    fn every_clock(&self, counter: CounterId) -> EvalResult<Value> {
        let m = &self.loaded.program.machines[self.id.index()];
        let site = m
            .layout
            .every_counters
            .get(counter.index())
            .ok_or_else(|| Trap::Bug(format!("every-Zaehler {} fehlt", counter.0)))?;
        match site.state {
            Some(_) => Ok(machine::time_in_state(m, self.state, self.tick_ns)),
            None => self.builtin(Builtin::Now),
        }
    }

    fn builtin(&self, b: Builtin) -> EvalResult<Value> {
        let m = &self.loaded.program.machines[self.id.index()];
        machine::builtin_value(b, m, self.state, self.tick, self.tick_ns, self.last_fault_value(self.loaded))
    }

    /// 12.9: Ein Port ist im Sim-Build ein Channel-Paar. Gelesen wird der
    /// `sim`-Output `mmio/ADR/r`, den ein Modell stellt — mit Unit-Delay
    /// wie jeder Modellwert (8.3).
    fn port_read(&mut self, p: PortId) -> EvalResult<Value> {
        let port = &self.loaded.program.ports[p.index()];
        let want = format!("mmio/{:#x}/r", port.address);
        let ty = port.ty;
        match self.loaded.program.channels.iter().position(|c| sim_address(c) == Some(want.clone())) {
            Some(i) => Ok(self.image.output(ChannelId(i as u32)).clone()),
            None => Ok(Value::default_for(ty, self.loaded.program)),
        }
    }

    /// 12.9: Ein Schreibvorgang wird ein Element des Eingangsstroms
    /// `mmio/ADR/w` — in Reihenfolge, auch mehrere je Tick.
    fn port_write(&mut self, p: PortId, v: Value) -> EvalResult<()> {
        let port = &self.loaded.program.ports[p.index()];
        let want = format!("mmio/{:#x}/w", port.address);
        let Some(i) = self.loaded.program.channels.iter().position(|c| sim_address(c) == Some(want.clone())) else {
            return Ok(());
        };
        let t = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(self.tick_ns);
        self.image.push_element(ChannelId(i as u32), t, v, false);
        Ok(())
    }

    fn armed(&self, t: TriggerId) -> EvalResult<Value> {
        let m = &self.loaded.program.machines[self.id.index()];
        let Some(i) = m.layout.trigger_flags.iter().position(|x| *x == t) else {
            return bug("Trigger nicht in dieser Maschine armiert (7.5)");
        };
        Ok(Value::Bool(self.state.armed.get(i).copied().unwrap_or(false)))
    }

    fn set_armed(&mut self, t: TriggerId, on: bool) -> EvalResult<()> {
        let m = &self.loaded.program.machines[self.id.index()];
        let Some(i) = m.layout.trigger_flags.iter().position(|x| *x == t) else {
            return bug("Trigger nicht in dieser Maschine armiert (7.5)");
        };
        if let Some(f) = self.state.armed.get_mut(i) {
            *f = on;
        }
        Ok(())
    }

    fn period(&self) -> EvalResult<i64> {
        let m = &self.loaded.program.machines[self.id.index()];
        Ok(i64::from(m.period).saturating_mul(self.tick_ns))
    }

    fn var_type(&self, v: VarId) -> EvalResult<TypeId> {
        let m = &self.loaded.program.machines[self.id.index()];
        m.vars.get(v.index()).map(|d| d.ty).ok_or_else(|| Trap::Bug(format!("Variable {} fehlt", v.0)))
    }

    fn step_taken(&mut self, span: takt_diag::Span, result: Option<String>) {
        if let Some(steps) = &mut self.steps {
            steps.push(crate::env::Step { span, result });
        }
    }

    fn steps_wanted(&self) -> bool {
        self.steps.is_some()
    }

    fn cover(&mut self, kind: crate::env::CoverKind, name: String) {
        self.out.push(Observation::Cover { kind, name });
    }

    fn observe(&mut self, o: Observation) -> EvalResult<()> {
        self.out.push(o);
        Ok(())
    }

    fn abort(&mut self) -> EvalResult<()> {
        self.aborted = true;
        Ok(())
    }

    fn send(&mut self, stream: StreamRef, v: Value, len_max: u32, span: Span) -> EvalResult<()> {
        match stream {
            // Ausgabestrom: die Bytes gehen in den Sendepuffer; reicht der
            // freie Platz nicht, ist das ein `StreamOverflow` (8.8).
            StreamRef::Channel(c) => {
                let elem = match self.loaded.ty(self.loaded.program.channels[c.index()].ty) {
                    Type::Stream(e) => *e,
                    _ => return bug(format!("Channel {} ist kein Strom", c.0)),
                };
                let bytes = element_bytes(self.loaded, elem, &v);
                let Some(tx) = self.image.tx.get_mut(&c) else {
                    return bug(format!("Channel {} ist kein Ausgabestrom", c.0));
                };
                if bytes.len() as u32 > tx.free() {
                    let name = self.loaded.program.channels[c.index()].name.clone();
                    let drop = matches!(self.loaded.program.channels[c.index()].attrs.overflow, Some(Overflow::Drop));
                    if drop {
                        self.out.push(Observation::Alert {
                            span,
                            index: Vec::new(),
                            active: true,
                            message: format!("Sendepuffer `{name}` voll, {} Byte verworfen", bytes.len()),
                            invalid: false,
                        });
                        return Ok(());
                    }
                    return Err(Trap::Fault(Fault::new(
                        FaultKind::StreamOverflow,
                        format!("Sendepuffer `{name}` hat {} Byte frei, {} verlangt", tx.free(), bytes.len()),
                        span,
                        self.tick,
                    )));
                }
                tx.queued.extend(bytes);
                Ok(())
            }
            // Interner Stream: das Element wird im naechsten Tick sichtbar
            // (8.6, Unit-Delay).
            StreamRef::Internal(sid) => {
                let v = as_element(self.loaded, sid, v);
                let t = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(self.tick_ns);
                let bytes = crate::stream::byte_len(&v).max(len_max.min(crate::stream::byte_len(&v)));
                let Some(slot) = self.image.stream_next.get_mut(sid.index()) else {
                    return bug(format!("interner Stream {} fehlt", sid.0));
                };
                slot.push((t, v, bytes));
                Ok(())
            }
            _ => bug("`send` auf einem Nicht-Stream"),
        }
    }

    fn schedule(&mut self, o: ChannelId, t: i64, v: Value, span: Span) -> EvalResult<()> {
        let now = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(self.tick_ns);
        // 9.8: `T <= now + guard(o)` ist ein `TimingFault`; in der Simulation
        // ist `guard` null.
        if t <= now {
            let name = self.loaded.program.channels[o.index()].name.clone();
            return Err(Trap::Fault(Fault::new(
                FaultKind::Timing,
                format!("`at` fuer `{name}` liegt nicht in der Zukunft"),
                span,
                self.tick,
            )));
        }
        let queue = self.image.sched.entry(o).or_default();
        if queue.len() as u32 >= MAX_SCHED {
            let name = self.loaded.program.channels[o.index()].name.clone();
            return Err(Trap::Fault(Fault::new(
                FaultKind::ScheduleOverflow,
                format!("`sched` von `{name}` ist voll ({MAX_SCHED})"),
                span,
                self.tick,
            )));
        }
        // Sortiert nach T; gleiche T: die spaetere Anweisung gewinnt (9.8).
        queue.retain(|(at, _)| *at != t);
        queue.push((t, v));
        queue.sort_by_key(|(at, _)| *at);
        Ok(())
    }

    fn cancel(&mut self, o: ChannelId) -> EvalResult<()> {
        self.image.sched.remove(&o);
        Ok(())
    }

    fn stream_window(&self, s: StreamRef) -> EvalResult<Vec<Element>> {
        Ok(self.window(self.loaded, s))
    }

    fn element_value(&mut self, v: VarId, e: &Element, caps: Vec<Value>) -> EvalResult<Value> {
        self.element_record(self.loaded, v, e, caps)
    }

    fn stream_peek(&mut self, s: StreamRef) -> EvalResult<Value> {
        let Some(first) = self.window(self.loaded, s).into_iter().next() else {
            return Ok(Value::Optional(None));
        };
        self.mark_examined(self.loaded, s, first.seq);
        Ok(Value::Optional(Some(Box::new(first.value))))
    }

    fn stream_examined(&mut self, s: StreamRef, seq: i64) -> EvalResult<()> {
        self.mark_examined(self.loaded, s, seq);
        Ok(())
    }

    fn stream_stat(&self, r: StreamRef, acc: Accessor) -> EvalResult<Option<Value>> {
        // `count` zaehlt das Fenster dieser Maschine, die uebrigen Zaehler
        // gehoeren dem Strom (8.6).
        // `o.sent` (8.8): was der Treiber beim letzten Commit abgeholt hat.
        if acc == Accessor::Sent {
            let sent = match r {
                StreamRef::Channel(c) => self.image.tx.get(&c).map(|t| t.sent.clone()).unwrap_or_default(),
                _ => Vec::new(),
            };
            return Ok(Some(Value::Optional((!sent.is_empty()).then(|| Box::new(Value::Bytes(sent))))));
        }
        if acc == Accessor::Free {
            let free = match r {
                StreamRef::Channel(c) => self.image.tx.get(&c).map_or(0, |t| t.free()),
                _ => 0,
            };
            return Ok(Some(Value::Int(i64::from(free))));
        }
        let buf = match r {
            StreamRef::Channel(c) => self.image.channel_bufs.get(&c),
            StreamRef::Internal(s) => self.image.stream_bufs.get(s.index()),
            _ => None,
        };
        let Some(buf) = buf else { return Ok(None) };
        let v = match acc {
            Accessor::Count => {
                let cursor = self.cursor_of(self.loaded, r);
                Value::Int(buf.count(cursor) as i64)
            }
            // `dropped[s]` am Puffer plus `dropped[s, m]` dieser Maschine (9.6).
            Accessor::Dropped => Value::Int(i64::from(buf.dropped) + i64::from(self.dropped_of(self.loaded, r))),
            Accessor::Overflowed => Value::Int(i64::from(buf.overflowed)),
            Accessor::Malformed => Value::Int(i64::from(buf.malformed)),
            _ => return Ok(None),
        };
        Ok(Some(v))
    }

    fn raise(&mut self, s: SignalId) -> EvalResult<()> {
        if let Some(slot) = self.state.raised_signals.get_mut(s.index()) {
            *slot = true;
            return Ok(());
        }
        bug(format!("Signal {} fehlt", s.0))
    }
}

/// Ein Lauf: Programm, Zustaende, Abbild.
pub struct Sim<'p> {
    /// Geladenes Programm.
    pub loaded: Loaded<'p>,
    /// Zustand je Maschine.
    pub states: Vec<MachineState>,
    /// Prozessabbild und Ψ.
    pub image: Image,
    /// Tick-Nummer.
    pub tick: u64,
    /// Reihenfolge der Schritte (7.2; permutierbar fuer Satz 9.4.1).
    pub order: Vec<MachineId>,
    /// Maschinen-Replay (12.5): die Maschinen, die nicht laufen; ihre
    /// Anfangswerte kommen aus `init_vars`, alles Weitere aus der Scheibe.
    pub foreign: Vec<MachineId>,
    /// Beobachtungen des laufenden Ticks je Maschine.
    pub observations: Vec<(MachineId, Observation)>,
    /// Ausgefuehrte Anweisungen des laufenden Ticks je Maschine, wenn die
    /// Schrittsicht laeuft (`takt sim --steps`).
    pub steps: Option<Vec<(MachineId, crate::env::Step)>>,
    /// Nichtfluechtiger Speicher fuer `persist var` (5.9); vor `init()` zu
    /// fuellen, danach unveraendert — das Schreiben liegt ausserhalb der
    /// Semantik.
    pub nvm: Nvm,
    /// Gescopte Instanzen mit ihrem Besitzer (5.11).
    pub scoped: Vec<(MachineId, takt_mir::machine::ScopedInstance)>,
    /// Cursor je Trigger auf seinem Quellstrom (7.5).
    ///
    /// Ein Trigger wird mit Ereignisrate ausgewertet, nicht mit dem Tick,
    /// und gehoert keiner Maschine — er kann darum nicht den Cursor
    /// seines Besitzers benutzen, der den Strom gar nicht liest.
    pub trigger_cursors: Vec<i64>,
    /// Welche gescopte Instanz war zu Beginn des vorigen Ticks aktiv?
    /// Die Aktivitaet steht vor jedem Schritt fest (5.11), der Vergleich
    /// mit diesem Stand liefert Ein- und Austritt.
    pub active_scoped: Vec<bool>,
}

/// Laufende Maschinen in Deklarationsreihenfolge (5.8, 13.6).
fn runnable(p: &Program) -> Vec<MachineId> {
    schedule::runnable(p)
}

impl<'p> Sim<'p> {
    /// Setzt alle Outputs auf ihren `safe`-Wert (12.7).
    ///
    /// Vor `reboot` und Deep Sleep verlangt 12.7 genau das — die Anlage
    /// bleibt in dem Zustand stehen, den sie ohne Programm haette.
    pub fn safe_all(&mut self) -> Result<(), Trap> {
        self.image.outputs = eval_safe_outputs(&self.loaded, &self.image.params)?;
        Ok(())
    }

    /// Ueberschreibt die Defaults der `persist`-Variablen mit den geladenen,
    /// gueltigen Werten (5.9, 9.10).
    ///
    /// Ein verworfener Eintrag meldet `PersistReset`; ein fehlender nicht —
    /// der erste Start eines Geraets ist kein Fehler.
    fn load_persist(&mut self) {
        let program = self.loaded.program;
        for id in self.order.clone() {
            let m = &program.machines[id.index()];
            for pv in &m.persist {
                let def = &m.vars[pv.var.index()];
                let (result, value) = self.nvm.load(program, pv.type_hash, def.ty);
                if let Some(v) = value {
                    self.states[id.index()].vars[pv.var.index()] = v;
                }
                if let Load::Reset(reason) = result {
                    self.observations.push((
                        id,
                        Observation::Alert {
                            span: def.span,
                            index: Vec::new(),
                            active: true,
                            message: format!("PersistReset: `{}` verworfen ({})", def.name, reason.text()),
                            invalid: false,
                        },
                    ));
                }
            }
        }
    }

    /// Die kanonische Form aller `persist`-Variablen (5.9): Maschinen in
    /// Deklarationsreihenfolge, darin nach Typ-Hash — dieselbe Ordnung,
    /// die der erzeugte Code schreibt. Nicht `self.order`: Die ist unter
    /// Satz 9.4.1 permutierbar, und die Zeile gehoert zum Trace.
    pub fn persist_payload(&self) -> Option<Vec<u8>> {
        let program = self.loaded.program;
        let mut out = Vec::new();
        for id in &runnable(program) {
            let m = &program.machines[id.index()];
            let values: Vec<(u64, &Value, TypeId)> = m
                .persist
                .iter()
                .map(|pv| (pv.type_hash, &self.states[id.index()].vars[pv.var.index()], m.vars[pv.var.index()].ty))
                .collect();
            out.extend(Nvm::payload(program, &values)?);
        }
        Some(out)
    }

    /// Neuer Lauf: Outputs auf `safe`, Parameter aus Defaults und Profil;
    /// `scenario` waehlt das Szenario, das mitlaeuft (13.6).
    pub fn new(
        program: &'p Program,
        profile: Option<&str>,
        overrides: &[(String, String)],
        scenario: Option<MachineId>,
    ) -> Result<Sim<'p>, Trap> {
        let loaded = Loaded::load(program).map_err(|d| Trap::Bug(format!("{d}")))?;
        let params = eval_params(&loaded, profile, overrides)?;
        let outputs = eval_safe_outputs(&loaded, &params)?;
        let image = Image::new(program, outputs, params);
        let states = program.machines.iter().map(MachineState::new).collect();
        // 7.2: topologisch nach `follows`, sonst Prioritaet; ein Zyklus ist
        // ein Fehler der Pruefung 33 und kommt hier nicht an.
        let order =
            schedule::order_with(program, scenario).unwrap_or_else(|_| schedule::runnable_with(program, scenario));
        let scoped = takt_mir::machine::scoped_instances(program);
        let active_scoped = vec![false; scoped.len()];
        Ok(Sim {
            loaded,
            states,
            image,
            tick: 0,
            order,
            foreign: Vec::new(),
            observations: Vec::new(),
            steps: None,
            nvm: Nvm::new(),
            scoped,
            active_scoped,
            trigger_cursors: vec![0; program.triggers.len()],
        })
    }

    /// Tick 0: jede Maschine betritt ihren Anfangszustand (1.5). Wird nach
    /// dem Stimulus des Ticks 0 gerufen, damit `sample()` auch dort vor der
    /// Schrittphase liegt (9.4).
    pub fn init(&mut self) -> Result<(), Trap> {
        self.init_with(|_| Ok(()))
    }

    /// Maschinen-Replay (12.5): nur `only` laeuft; die anderen behalten
    /// ihre Anfangswerte und bekommen alles Weitere aus der Scheibe.
    pub fn restrict(&mut self, only: MachineId) {
        self.foreign = self.order.iter().copied().filter(|x| *x != only).collect();
        self.order.retain(|x| *x == only);
    }

    /// Tick 0 mit einem Schritt zwischen den Anfangswerten und dem Eintritt:
    /// Dort setzt die Scheibe, was die fremden Maschinen im Tick 0
    /// veroeffentlichten (12.5).
    pub fn init_with(&mut self, between: impl FnOnce(&mut Self) -> Result<(), Trap>) -> Result<(), Trap> {
        let program = self.loaded.program;
        let tick_ns = program.config.tick;
        self.observations.clear();
        self.image.apply_sim_bindings(program, 0);
        // Ψ traegt im Tick 0 die Anfangswerte der Variablen, damit die
        // `enter`- und Entry-`loop`-Bloecke sie schon lesen koennen (1.4).
        for id in [self.order.clone(), self.foreign.clone()].concat() {
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            env.init_vars(&self.loaded, 0)?;
            self.observations.extend(out.into_iter().map(|o| (id, o)));
        }
        self.load_persist();
        self.publish_all();
        for id in self.foreign.clone() {
            self.publish_one(id);
        }
        self.image.commit_published();
        between(self)?;
        // Die fremden Maschinen gelten als eingetreten (7.2): Was die
        // Scheibe brachte, lesen Follower frisch.
        for id in self.foreign.clone() {
            self.image.set_fresh(id);
        }
        // 5.11: Eine gescopte Instanz betritt nichts, solange ihr Scope
        // steht nicht — der Besitzer sagt es erst mit seinem `init`.
        let scoped_ids: Vec<MachineId> = self.scoped.iter().map(|(_, si)| si.machine).collect();
        for id in self.order.clone() {
            if scoped_ids.contains(&id) {
                continue;
            }
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            machine::init(&self.loaded, &mut env, 0)?;
            self.observations.extend(out.into_iter().map(|o| (id, o)));
            self.publish_one(id);
            self.image.set_fresh(id);
        }
        self.scoped_lifecycle(tick_ns)?;
        self.image.clear_fresh();
        // Tick 0 aktiviert jede Maschine mit `phase = 0` (7.2); der Zaehler
        // wird wie am Ende jedes Ticks fortgeschrieben.
        let active: Vec<MachineId> =
            self.order.iter().copied().filter(|id| self.states[id.index()].countdown == 0).collect();
        self.advance_counters(&active);
        self.publish_all();
        self.image.commit_published();
        self.drain_tx(0);
        self.image.commit_outputs();
        Ok(())
    }

    /// Der Treiber holt die gesendeten Bytes ab (8.8) — in jedem Tick, auch
    /// im Tick 0. Ein Modell liest den Ausgabestrom mit Unit-Delay (8.3):
    /// was der Treiber jetzt abgeholt hat, steht im naechsten Tick im Fenster.
    fn drain_tx(&mut self, now: i64) {
        let program = self.loaded.program;
        for tx in self.image.tx.values_mut() {
            tx.drain();
        }
        let taken: Vec<(ChannelId, Vec<u8>)> = self
            .image
            .tx
            .iter()
            .filter(|(c, t)| !t.sent.is_empty() && self.image.channel_bufs.contains_key(c))
            .map(|(c, t)| (*c, t.sent.clone()))
            .collect();
        for (c, bytes) in taken {
            let value = crate::image::element_of(&bytes, program.channels[c.index()].ty, program);
            let drop_oldest = matches!(program.channels[c.index()].attrs.overflow, Some(Overflow::DropOldest));
            self.image.push_element(c, now, value, drop_oldest);
        }
    }

    /// `deliver(D_k)` (9.6): die im vorigen Tick gesendeten Elemente eines
    /// internen Stroms werden sichtbar. Ein Ueberlauf merkt den Fault fuer
    /// jeden Konsumenten vor; bei `drop_oldest` faellt ein Alert an.
    /// Ein uebergelaufener Eingabestrom faultet jede Maschine, die ihn liest
    /// (8.6): der Ueberlauf ist ein Fehler des Systems, kein stiller Verlust.
    pub fn overflow_channel(&mut self, c: ChannelId) {
        let program = self.loaded.program;
        let name = program.channels[c.index()].name.clone();
        let span = program.channels[c.index()].span;
        let f = Fault::new(FaultKind::StreamOverflow, format!("Stream `{name}` uebergelaufen"), span, self.tick);
        let wakes = program.channels[c.index()].attrs.wake;
        for id in self.order.clone() {
            // 9.6: Einer schlafenden Maschine wird der Ueberlauf nicht
            // zugestellt, solange der Strom sie nicht weckt (5.10).
            if !wakes && self.is_idle(id) {
                continue;
            }
            let reads = program.machines[id.index()].layout.cursors.contains(&StreamRef::Channel(c));
            if reads && self.states[id.index()].pending.is_none() {
                self.states[id.index()].pending = Some(f.clone());
            }
        }
    }

    fn deliver(&mut self) -> Result<(), Trap> {
        let program = self.loaded.program;
        for (i, def) in program.streams.iter().enumerate() {
            let pending = std::mem::take(&mut self.image.stream_next[i]);
            if pending.is_empty() {
                continue;
            }
            let drop_oldest = matches!(def.overflow, Overflow::DropOldest);
            let mut overflowed = false;
            let mut dropped = 0;
            for (t, value, bytes) in pending {
                match self.image.stream_bufs[i].push(t, value, bytes, drop_oldest) {
                    Delivery::Ok => {}
                    Delivery::Overflow => overflowed = true,
                    Delivery::Dropped(n) => dropped += n,
                }
            }
            if dropped > 0 {
                let name = def.name.clone();
                for id in self.order.clone() {
                    if def.readers.contains(&id) {
                        self.observations.push((
                            id,
                            Observation::Alert {
                                span: def.span,
                                index: Vec::new(),
                                active: true,
                                message: format!("Stream `{name}` hat {dropped} Elemente verworfen"),
                                invalid: false,
                            },
                        ));
                    }
                }
            }
            if overflowed {
                let f = Fault::new(
                    FaultKind::StreamOverflow,
                    format!("Stream `{}` uebergelaufen", def.name),
                    def.span,
                    self.tick,
                );
                for id in self.order.clone() {
                    // 9.6: Einer schlafenden Maschine wird der Ueberlauf
                    // nicht zugestellt — sie hoert nicht hin (5.10).
                    if self.is_idle(id) {
                        continue;
                    }
                    if def.readers.contains(&id) && self.states[id.index()].pending.is_none() {
                        self.states[id.index()].pending = Some(f.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Ist die Maschine in einem `idle`-Zustand (5.10)?
    pub fn is_idle(&self, id: MachineId) -> bool {
        let m = &self.loaded.program.machines[id.index()];
        self.states[id.index()].conf.iter().any(|c| m.states[c.index()].idle)
    }

    /// Weckt dieser Strom aus dem Schlaf (5.10)?
    fn wakes(&self, r: StreamRef) -> bool {
        match r {
            StreamRef::Channel(c) => self.loaded.program.channels[c.index()].attrs.wake,
            // Interne Stroeme entstehen aus `send` und wecken nie. `fired`
            // und Handles sind v1.2; bis dahin gilt dasselbe.
            _ => false,
        }
    }

    /// Hinter das letzte Element eines Stroms.
    fn stream_end(&self, r: StreamRef) -> Option<i64> {
        match r {
            StreamRef::Channel(c) => self.image.channel_bufs.get(&c).map(crate::stream::Buffer::end),
            StreamRef::Internal(i) => self.image.stream_bufs.get(i.index()).map(crate::stream::Buffer::end),
            _ => None,
        }
    }

    /// `advance_cursors()` (9.6): der Cursor rueckt hinter das zuletzt
    /// untersuchte Element; danach faellt alles weg, was kein Konsument mehr
    /// sehen kann.
    fn advance_cursors(&mut self) {
        let program = self.loaded.program;
        for id in self.order.clone() {
            let m = &program.machines[id.index()];
            // 5.10: Eine Maschine in `idle` hoert nicht; ihre
            // Nicht-Wake-Stroeme werden verworfen statt zu ueberlaufen.
            let idle = self.is_idle(id);
            let ends: Vec<Option<i64>> = m
                .layout
                .cursors
                .iter()
                .map(|r| if idle && !self.wakes(*r) { self.stream_end(*r) } else { None })
                .collect();
            let state = &mut self.states[id.index()];
            // 5.10: Der Alert kommt beim *Verlassen* — wer aufwacht, soll
            // erfahren, was er missed hat.
            let woke_up = state.was_idle && !idle;
            state.was_idle = idle;
            let mut missed = 0u32;
            for (i, _) in m.layout.cursors.iter().enumerate() {
                let examined = state.examined.get(i).copied().unwrap_or(-1);
                if let Some(end) = ends[i] {
                    let before = state.cursors.get(i).copied().unwrap_or(0);
                    if let Some(d) = state.dropped.get_mut(i) {
                        *d = d.saturating_add(u32::try_from(end - before).unwrap_or(u32::MAX));
                    }
                    if let Some(c) = state.cursors.get_mut(i) {
                        *c = end;
                    }
                } else if examined >= 0 {
                    if let Some(c) = state.cursors.get_mut(i) {
                        *c = examined + 1;
                    }
                }
                if let Some(e) = state.examined.get_mut(i) {
                    *e = -1;
                }
            }
            if woke_up {
                missed = state.dropped.iter().copied().fold(0u32, u32::saturating_add);
            }
            if woke_up && missed > 0 {
                let span = m.states.first().map_or_else(takt_diag::Span::default, |s| s.span);
                self.observations.push((
                    id,
                    Observation::Alert {
                        span,
                        index: Vec::new(),
                        active: true,
                        message: format!("StreamPaused: {missed} Elemente im Schlaf verworfen"),
                        invalid: false,
                    },
                ));
            }
        }
        // Eviction: das Minimum ueber alle Konsumenten je Stream.
        let mut min_channel: HashMap<ChannelId, i64> = HashMap::new();
        let mut min_stream: Vec<Option<i64>> = vec![None; program.streams.len()];
        for id in &self.order {
            let m = &program.machines[id.index()];
            let state = &self.states[id.index()];
            for (i, r) in m.layout.cursors.iter().enumerate() {
                let cur = state.cursors.get(i).copied().unwrap_or(0);
                match r {
                    StreamRef::Channel(c) => {
                        let slot = min_channel.entry(*c).or_insert(cur);
                        *slot = (*slot).min(cur);
                    }
                    StreamRef::Internal(sid) => {
                        let slot = &mut min_stream[sid.index()];
                        *slot = Some(slot.map_or(cur, |v: i64| v.min(cur)));
                    }
                    _ => {}
                }
            }
        }
        for (c, min) in min_channel {
            if let Some(buf) = self.image.channel_bufs.get_mut(&c) {
                buf.evict(min);
            }
        }
        for (i, min) in min_stream.iter().enumerate() {
            if let Some(min) = min {
                self.image.stream_bufs[i].evict(*min);
            }
        }
    }

    /// Schreibt Aktivierungszaehler und Zustandstimer fort (9.4).
    fn advance_counters(&mut self, active: &[MachineId]) {
        let program = self.loaded.program;
        for id in self.order.clone() {
            let m = &program.machines[id.index()];
            let state = &mut self.states[id.index()];
            if active.contains(&id) {
                state.countdown = m.period.saturating_sub(1);
                if let Some(leaf) = state.leaf() {
                    for s in machine::chain_to(m, leaf) {
                        state.timers[s.index()] = state.timers[s.index()].saturating_add(1);
                    }
                }
            } else {
                state.countdown = state.countdown.saturating_sub(1);
            }
        }
    }

    /// Laesst die Abtastungen um einen Tick altern (3.5). Getrennt von
    /// `step`, weil der Stimulus des Ticks dazwischen liegt: eine frische
    /// Lieferung hat das Alter ihres Treibers, nicht schon einen Tick.
    pub fn age(&mut self) {
        let program = self.loaded.program;
        self.image.age_inputs(program, program.config.tick);
        // Ein Command gilt einen Tick (8.5): bis zum naechsten Stimulus, damit
        // der Tick-Rand-Snapshot der Eigenschaften ihn noch sieht (13.3).
        self.image.clear_commands();
    }

    /// Ein uebersprungener Tick (9.9): nur Tick und Zaehler laufen weiter —
    /// der Schritt waere die Identitaet gewesen (Satz 9.9.1).
    pub fn skip_tick(&mut self) {
        self.tick += 1;
        let active: Vec<MachineId> =
            self.order.iter().copied().filter(|id| self.states[id.index()].countdown == 0).collect();
        self.advance_counters(&active);
    }

    /// Ein System-Tick (9.4).
    /// Trigger-Phase (7.5): die armierten Trigger ueber die Elemente, die
    /// in diesem Tick eingetroffen sind.
    ///
    /// Sie liegt zwischen Zustellung und Schrittphase, damit die geplante
    /// Ausgabe im selben Tick in `sched` steht. Ein Trigger ist
    /// einschuessig: Das erste passende Element feuert, `armed` faellt,
    /// und `fired` bekommt ein Element — im naechsten Tick sichtbar, wie
    /// jeder interne Strom (8.6).
    ///
    /// Ausgefuehrt wird er in der Umgebung seines Besitzers: 7.5 nennt
    /// `armed` dessen Zustand, und `schedule` braucht ohnehin eine
    /// Maschine.
    fn trigger_phase(&mut self, tick_ns: i64) -> Result<(), Trap> {
        for i in 0..self.loaded.program.triggers.len() {
            let t = &self.loaded.program.triggers[i];
            let (Some(owner), fired) = (t.owner, t.fired) else { continue };
            let flags = &self.loaded.program.machines[owner.index()].layout.trigger_flags;
            let Some(slot) = flags.iter().position(|x| x.index() == i) else { continue };
            if !self.states[owner.index()].armed.get(slot).copied().unwrap_or(false) {
                continue;
            }
            let Some(hit) = self.trigger_fires(TriggerId(i as u32), owner, tick_ns)? else { continue };
            self.states[owner.index()].armed[slot] = false;
            let t = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(tick_ns);
            let bytes = crate::stream::byte_len(&hit);
            if let Some(q) = self.image.stream_next.get_mut(fired.index()) {
                q.push((t, hit, bytes));
            }
        }
        Ok(())
    }

    /// Prueft einen armierten Trigger gegen die Elemente dieses Ticks und
    /// plant seine Ausgaben, wenn er feuert (7.5). Liefert das Element von
    /// `fired`: die Captures und `.t` des Ausloesers.
    fn trigger_fires(&mut self, id: TriggerId, owner: MachineId, tick_ns: i64) -> Result<Option<Value>, Trap> {
        let trigger = self.loaded.program.triggers[id.index()].clone();
        let takt_mir::machine::Guard::Match { subject, kind, pattern, .. } = &trigger.guard else { return Ok(None) };
        let Some(stream) = stream_of_expr(subject) else { return Ok(None) };
        // 7.5: mit dem eigenen Cursor des Triggers, nicht dem des
        // Besitzers — der liest den Quellstrom in der Regel gar nicht.
        let cursor = self.trigger_cursors[id.index()];
        let window = match stream {
            StreamRef::Channel(c) => self.image.channel_bufs.get(&c).map(|b| b.window(cursor)).unwrap_or_default(),
            StreamRef::Internal(s) => {
                self.image.stream_bufs.get(s.index()).map(|b| b.window(cursor)).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        let mut env =
            MachineEnv::new(&self.loaded, owner, &mut self.states[owner.index()], &mut self.image, &mut out, tick_ns);
        let mut found = None;
        let mut seen = None;
        for element in window {
            seen = Some(element.seq);
            let consts = trigger_consts(&self.loaded, &mut env, pattern, self.tick)?;
            if let Some(caps) = crate::pattern::match_value(pattern, *kind, &element.value, &consts) {
                found = Some((element, caps));
                break;
            }
        }
        let Some((element, caps)) = found else {
            // Kein Treffer: alles Gesehene ist untersucht (9.6).
            if let Some(last) = seen {
                self.trigger_cursors[id.index()] = last + 1;
            }
            self.observations.extend(out.into_iter().map(|o| (owner, o)));
            return Ok(None);
        };
        self.trigger_cursors[id.index()] = element.seq + 1;
        // 7.5: `event` ist das Element mit seinen Captures; `then` plant
        // seine Ausgaben fuer `event.t + d` mit `guard = bound`.
        let event = env.element_record_for(&self.loaded, trigger.fired, &element, caps)?;
        let mut ctx = env.ctx(&self.loaded, self.tick).with_event(event.clone());
        let at = ctx.eval_duration(&trigger.time)?;
        let mut writes = Vec::new();
        for stmt in &trigger.then.stmts {
            let takt_mir::stmt::StmtKind::Assign { target: takt_mir::stmt::Place::Output(c), value } = &stmt.kind
            else {
                return bug("`then` eines Triggers enthaelt mehr als Output-Zuweisungen");
            };
            writes.push((*c, ctx.eval(value)?, stmt.span));
        }
        for (c, v, span) in writes {
            env.schedule(c, at, v, span)?;
        }
        self.observations.extend(out.into_iter().map(|o| (owner, o)));
        Ok(Some(event))
    }

    /// Die gescopten Instanzen, die gerade nicht laufen (5.11).
    fn inactive_scoped(&self) -> Vec<MachineId> {
        (0..self.scoped.len()).filter(|i| !self.active_scoped[*i]).map(|i| self.scoped[i].1.machine).collect()
    }

    /// Ist die gescopte Instanz `i` aktiv? (5.11) Sie ist es, wenn ihr
    /// Scope-Zustand in der Konfiguration des Besitzers liegt. Die Frage
    /// wird zu Tick-Beginn gestellt und gilt fuer den ganzen Tick; damit
    /// haengt der Trace nicht von der Schrittordnung ab (Satz 9.4.1).
    fn scope_active(&self, i: usize) -> bool {
        let (owner, si) = &self.scoped[i];
        self.states[owner.index()].conf.contains(&si.scope)
    }

    /// Ein- und Austritte der gescopten Instanzen nach einem Schritt des
    /// Besitzers (5.11).
    ///
    /// Austritt: `exit` von innen nach aussen bei einem regulaeren
    /// Uebergang, ohne `exit` nach einem Fault (2.2 in plan/m8.md, wie
    /// 5.4 es fuer den Besitzer sagt); danach gehen die Outputs der
    /// Instanz auf `safe` und ihr Zustand wird verworfen. Eintritt: frisch
    /// initialisiert, `initial` betreten.
    fn scoped_lifecycle(&mut self, tick_ns: i64) -> Result<(), Trap> {
        for i in 0..self.scoped.len() {
            let now = self.scope_active(i);
            if now == self.active_scoped[i] {
                continue;
            }
            let (owner, si) = self.scoped[i].clone();
            let inst = si.machine;
            if now {
                self.enter_scoped(inst, tick_ns)?;
            } else {
                let faulted = self.states[owner.index()].faulted
                    || self.states[owner.index()].last_fault.as_ref().is_some_and(|f| f.tick == self.tick);
                self.leave_scoped(inst, faulted, tick_ns)?;
            }
            self.active_scoped[i] = now;
        }
        Ok(())
    }

    /// Eine gescopte Instanz betreten (5.11): frischer Zustand, Variablen,
    /// `initial`. Sie schreitet ab dem naechsten Tick.
    fn enter_scoped(&mut self, inst: MachineId, tick_ns: i64) -> Result<(), Trap> {
        let program = self.loaded.program;
        self.states[inst.index()] = MachineState::new(&program.machines[inst.index()]);
        let mut out = Vec::new();
        let mut env =
            MachineEnv::new(&self.loaded, inst, &mut self.states[inst.index()], &mut self.image, &mut out, tick_ns);
        env.init_vars(&self.loaded, self.tick)?;
        machine::init(&self.loaded, &mut env, self.tick)?;
        self.observations.extend(out.into_iter().map(|o| (inst, o)));
        self.publish_one(inst);
        Ok(())
    }

    /// Eine gescopte Instanz verlassen (5.11).
    fn leave_scoped(&mut self, inst: MachineId, by_fault: bool, tick_ns: i64) -> Result<(), Trap> {
        let mut out = Vec::new();
        if !by_fault {
            let mut env =
                MachineEnv::new(&self.loaded, inst, &mut self.states[inst.index()], &mut self.image, &mut out, tick_ns);
            machine::exit_all(&self.loaded, &mut env, self.tick)?;
        }
        // 5.11: Was die Instanz stellte, geht auf `safe` — wie bei
        // FAULTED, nur dass hier die ganze Maschine verschwindet.
        let mut safe = Vec::new();
        let mut env =
            MachineEnv::new(&self.loaded, inst, &mut self.states[inst.index()], &mut self.image, &mut safe, tick_ns);
        env.safe_outputs(&self.loaded);
        out.extend(safe);
        self.observations.extend(out.into_iter().map(|o| (inst, o)));
        self.publish_one(inst);
        self.states[inst.index()] = MachineState::new(&self.loaded.program.machines[inst.index()]);
        Ok(())
    }

    /// Ein System-Tick (9.4).
    pub fn step(&mut self) -> Result<(), Trap> {
        let program = self.loaded.program;
        let tick_ns = program.config.tick;
        self.observations.clear();
        self.tick += 1;
        // sample(): sim-Bindungen (8.3). Die Alterung liegt in `age()` und
        // laeuft vor dem Stimulus dieses Ticks, damit eine frische Lieferung
        // mit dem Alter 0 gelesen wird (9.4: `I_k = sample()`).
        let now = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(tick_ns);
        self.image.apply_sim_bindings(program, now);
        // deliver(D_k): interne Streams werden sichtbar, Ueberlauf merkt den
        // Fault fuer jeden Konsumenten vor (9.6).
        self.deliver()?;
        // 7.5: die Trigger-Phase liegt zwischen Zustellung und Schritt —
        // die geplante Ausgabe steht damit im selben Tick in `sched`.
        self.trigger_phase(tick_ns)?;
        // active(k): countdown == 0 (7.2); eine gescopte Instanz zusaetzlich
        // nur, wenn ihr Scope-Zustand zu Tick-Beginn steht (5.11).
        let inactive = self.inactive_scoped();
        let active: Vec<MachineId> = self
            .order
            .iter()
            .copied()
            .filter(|id| self.states[id.index()].countdown == 0 && !inactive.contains(id))
            .collect();
        // Schrittphase; `fresh[m] = publish_m(v_m)` nach jedem Schritt, nur
        // fuer Follower in dieser Phase sichtbar (9.4).
        let mut aborted = false;
        for id in &active {
            let mut out = Vec::new();
            let mut taken = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, *id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            if self.steps.is_some() {
                env = env.with_steps(&mut taken);
            }
            let result = machine::step_m(&self.loaded, &mut env, self.tick);
            aborted |= env.aborted;
            self.observations.extend(out.into_iter().map(|o| (*id, o)));
            if let Some(steps) = &mut self.steps {
                steps.extend(taken.into_iter().map(|s| (*id, s)));
            }
            result?;
            self.publish_one(*id);
            self.image.set_fresh(*id);
        }
        // 5.11: Der Besitzer ist geschritten, seine Konfiguration steht —
        // jetzt treten die gescopten Instanzen ein oder aus.
        self.scoped_lifecycle(tick_ns)?;
        // abort_phase(): Abort und Runtime-Faults wirken im selben Tick fuer
        // alle Maschinen, ob aktiv oder nicht (5.4, 9.4); dort gilt Ψ_k.
        self.image.clear_fresh();
        if aborted {
            self.raise_all();
        }
        self.abort_phase(tick_ns, &active)?;
        // advance_cursors(): `cur[s, m] = examined + 1`, danach Eviction
        // unterhalb des kleinsten Cursors (9.6).
        self.advance_cursors();
        // Zaehler fortschreiben (7.2)
        self.advance_counters(&active);
        // Erhobene Signale als Beobachtung (5.8), bevor sie zurueckgesetzt werden
        for id in self.order.clone() {
            let machine = &program.machines[id.index()];
            for (i, raised) in self.states[id.index()].raised_signals.iter().enumerate() {
                if *raised {
                    self.observations.push((id, Observation::Signal { name: machine.signals[i].name.clone() }));
                }
            }
        }
        // publish(sigma), commit(L)
        self.publish_all();
        self.image.commit_published();
        // apply_scheduled(k): faellige geplante Schreibvorgaenge vor dem
        // Commit (9.4, 9.8).
        let now = i64::try_from(self.tick).unwrap_or(i64::MAX).saturating_mul(tick_ns);
        self.image.apply_scheduled(now);
        self.drain_tx(now);
        self.image.commit_outputs();
        for state in &mut self.states {
            state.raised_signals.iter_mut().for_each(|s| *s = false);
        }
        Ok(())
    }

    /// Eine `abort`-Anweisung merkt den Fault fuer alle anderen Maschinen vor
    /// (`raised[m']`, 5.4). Gelesen wird erst nach allen Schritten, damit die
    /// Ausfuehrungsreihenfolge irrelevant bleibt (Satz 9.4.1).
    fn raise_all(&mut self) {
        for id in self.order.clone() {
            let state = &mut self.states[id.index()];
            if !state.faulted && state.raised.is_none() {
                state.raised = Some(Fault::new(FaultKind::Abort, "abort", takt_diag::Span::default(), self.tick));
            }
        }
    }

    /// Abort-Phase (5.4, 9.4): jede Maschine mit einem erhobenen Fault, und
    /// jede inaktive mit einem vorgemerkten Abort- oder Runtime-Fault, nimmt
    /// ihren Fault-Pfad — im selben Tick, unabhaengig von ihrer Periode.
    fn abort_phase(&mut self, tick_ns: i64, active: &[MachineId]) -> Result<(), Trap> {
        for id in self.order.clone() {
            let state = &mut self.states[id.index()];
            if state.faulted {
                state.raised = None;
                continue;
            }
            let raised = state.raised.take();
            let pending = match (&raised, active.contains(&id), &state.pending) {
                // Eine aktive Maschine hat ihren vorgemerkten Fault schon im
                // Schritt zugestellt bekommen (9.6).
                (None, false, Some(f)) if matches!(f.kind, FaultKind::Abort | FaultKind::Runtime(_)) => {
                    state.pending.take()
                }
                _ => None,
            };
            let Some(f) = raised.or(pending) else { continue };
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            let result = machine::resolve_m(&self.loaded, &mut env, Err(Trap::Fault(f)), self.tick);
            self.observations.extend(out.into_iter().map(|o| (id, o)));
            result?;
        }
        Ok(())
    }

    /// Veroeffentlicht `pub var`, Zustand und Signale aller Maschinen (9.4).
    fn publish_all(&mut self) {
        for id in self.order.clone() {
            self.publish_one(id);
        }
    }

    /// `publish_m(v_m)`: traegt eine Maschine in Ψ_{k+1} ein.
    fn publish_one(&mut self, id: MachineId) {
        let program = self.loaded.program;
        let machine = &program.machines[id.index()];
        let vars = self.states[id.index()].vars.clone();
        let signals = self.states[id.index()].raised_signals.clone();
        let mut out = Vec::new();
        let env = MachineEnv::new(
            &self.loaded,
            id,
            &mut self.states[id.index()],
            &mut self.image,
            &mut out,
            program.config.tick,
        );
        let state = env.state_value(&self.loaded);
        self.image.publish(id, machine, &vars, state, &signals);
    }

    /// Zustandspfad einer Maschine.
    pub fn path(&self, id: MachineId) -> String {
        self.states[id.index()].path(&self.loaded.program.machines[id.index()])
    }

    /// Committete Ausgabe eines Outputs.
    pub fn output(&self, c: ChannelId) -> &Value {
        self.image.output(c)
    }

    /// Commit-Zeitpunkt (2.4): `asap` oder `boundary`; in der Simulation
    /// unterscheidet sich nur die Aufzeichnung.
    pub fn output_timing(&self) -> OutputTiming {
        self.loaded.program.config.output_timing
    }
}

/// Parameterwerte: Defaults, dann Profil (8.4).
/// Defaults, dann das Profil, dann die Ueberlagerung (8.4, 13.7).
fn eval_params(loaded: &Loaded<'_>, profile: Option<&str>, overrides: &[(String, String)]) -> Result<Vec<Value>, Trap> {
    let p = loaded.program;
    let mut env = crate::ConstEnv::new(p.config.tick);
    let mut ctx = Ctx::new(loaded, &mut env, 0);
    let mut out = Vec::with_capacity(p.params.len());
    for param in &p.params {
        out.push(ctx.eval(&param.default)?);
    }
    if let Some(name) = profile {
        let Some(prof) = p.profiles.iter().find(|x| x.name == name) else {
            return bug(format!("Profil `{name}` gibt es nicht"));
        };
        for (id, value) in &prof.assignments {
            let v = ctx.eval(value)?;
            let ty = p.params[id.index()].ty;
            if let Some(range) = range_of(loaded, ty) {
                if !crate::eval::in_range(&v, &range) {
                    return bug(format!(
                        "Profil `{name}`: Wert von `{}` ausserhalb der Range (8.4)",
                        p.params[id.index()].name
                    ));
                }
            }
            out[id.index()] = v;
        }
    }
    for (name, text) in overrides {
        let Some(i) = p.params.iter().position(|q| q.name == *name) else {
            return bug(format!("Parameter `{name}` gibt es nicht"));
        };
        let v = crate::trace::parse_value(text, p.params[i].ty, p).map_err(Trap::Bug)?;
        if let Some(range) = range_of(loaded, p.params[i].ty) {
            if !crate::eval::in_range(&v, &range) {
                return bug(format!("`{name} = {text}` liegt ausserhalb der Range (8.4)"));
            }
        }
        out[i] = v;
    }
    Ok(out)
}

fn range_of(loaded: &Loaded<'_>, ty: TypeId) -> Option<takt_mir::types::Range> {
    match loaded.ty(ty) {
        Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range } => *range,
        _ => None,
    }
}

/// Anfangswerte der Outputs: `safe` (1.5, 8.1).
fn eval_safe_outputs(loaded: &Loaded<'_>, params: &[Value]) -> Result<Vec<Value>, Trap> {
    let p = loaded.program;
    let mut out = Vec::with_capacity(p.channels.len());
    for c in &p.channels {
        let value = match (&c.attrs.safe, c.dir) {
            (Some(e), _) => {
                let mut env = ParamEnv { params: params.to_vec(), tick: p.config.tick };
                let mut ctx = Ctx::new(loaded, &mut env, 0);
                ctx.eval(e)?
            }
            (None, _) => Value::default_for(c.ty, p),
        };
        out.push(value);
    }
    Ok(out)
}

/// Umgebung mit Parametern, aber ohne Maschine (Anfangswerte der Outputs).
struct ParamEnv {
    params: Vec<Value>,
    tick: i64,
}

impl Outer for ParamEnv {
    fn param(&self, p: ParamId) -> EvalResult<&Value> {
        self.params.get(p.index()).ok_or_else(|| Trap::Bug(format!("Parameter {} fehlt", p.0)))
    }

    fn builtin(&self, b: Builtin) -> EvalResult<Value> {
        match b {
            Builtin::Tick => Ok(Value::Duration(self.tick)),
            other => bug(format!("`{other:?}` ist im Anfangswert nicht lesbar")),
        }
    }
}

/// `K_o` (7.5): hoechstens so viele geplante Schreibvorgaenge je Output.
/// Der Default aus 7.5 ist vier.
pub const MAX_SCHED: u32 = 4;

/// Die Bytes eines Elements fuer einen Ausgabestrom (8.8): Text und
/// `bytes<N>` als Inhalt, alles andere in der kanonischen Byteform
/// (plan/m6.md 2.2) — so liest es der `sim`-gekoppelte Eingang, und so
/// legt es der Rahmen ab.
fn element_bytes(loaded: &Loaded<'_>, elem: TypeId, v: &Value) -> Vec<u8> {
    match v {
        Value::Bytes(b) => b.clone(),
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Line { text, .. } => text.as_bytes().to_vec(),
        Value::Array(items) => items.iter().flat_map(|x| element_bytes(loaded, elem, x)).collect(),
        // Feste Elementform (plan/m6.md 2.2): die kanonische Form, mit
        // Nullen auf `max_size` gefuellt — ein `bytes<N>`-Feld macht sie
        // sonst variabel lang, und der Leser faende die Grenzen nicht (FB-189).
        other => {
            let mut bytes = crate::bytes::encode(loaded.program, other, elem).unwrap_or_default();
            if let Ok(size) = takt_mir::bytes::max_size(loaded.program, elem) {
                bytes.resize(size as usize, 0);
            }
            bytes
        }
    }
}

/// Text in einem `bytes`-Strom ist ein Element aus seinen Bytes (8.8): Die
/// Sema laesst das Literal zu, der Wert muss die Form des Elements haben,
/// sonst faultet der Leser bei `data[i]` (FB-176).
fn as_element(loaded: &Loaded<'_>, sid: takt_mir::StreamId, v: Value) -> Value {
    let elem = loaded.program.streams[sid.index()].elem;
    match (loaded.ty(elem), v) {
        (Type::Bytes { .. }, Value::Str(s)) => Value::Bytes(s.into_bytes()),
        (Type::Bytes { .. }, Value::Line { text, .. }) => Value::Bytes(text.into_bytes()),
        (_, v) => v,
    }
}

/// Der Strom, ueber den ein Trigger-Guard laeuft (7.5).
fn stream_of_expr(subject: &takt_mir::expr::Expr) -> Option<StreamRef> {
    match &subject.kind {
        takt_mir::expr::ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
        takt_mir::expr::ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
        _ => None,
    }
}

/// Die Konstanten der Feldbedingungen eines Trigger-Musters (8.7).
fn trigger_consts(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    pattern: &takt_mir::pattern::Pattern,
    tick: u64,
) -> Result<Vec<Value>, Trap> {
    let takt_mir::pattern::Pattern::Record { fields, .. } = pattern else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for (_, e) in fields {
        let mut ctx = env.ctx(loaded, tick);
        out.push(ctx.eval(e)?);
    }
    Ok(out)
}

/// Die `sim`-Adresse eines Channels als Text (8.3, 12.9).
fn sim_address(c: &takt_mir::program::Channel) -> Option<String> {
    let takt_mir::program::Binding::Sim(a) = &c.binding else { return None };
    Some(a.segments.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join("/"))
}
