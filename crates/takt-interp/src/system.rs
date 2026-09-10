//! System-Tick (Referenz 9.4): `tick(k)` mit Abtastung, Aktivierung,
//! Schrittphase, Abort-Phase, Veroeffentlichung und Commit; dazu die
//! Umgebung einer Maschine ueber dem Prozessabbild.

use takt_mir::expr::Builtin;
use takt_mir::machine::{FaultKind, Machine, MachineKind, VarScope};
use takt_mir::program::{OutputTiming, Program};
use takt_mir::types::Type;
use takt_mir::*;

use crate::env::{MachineEnv, Observation, Outer};
use crate::eval::Ctx;
use crate::image::Image;
use crate::loaded::Loaded;
use crate::machine::{self, MachineState};
use crate::value::{EvalResult, Fault, Sample, Trap, Value, bug};

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
        MachineEnv { loaded, id, state, image, out, tick_ns, tick: 0, aborted: false }
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
        for i in counters {
            self.state.every_next[i] = 0;
        }
        for i in sites {
            self.state.viol[i] = 0;
        }
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

    fn published(&self, m: MachineId, v: VarId) -> EvalResult<&Value> {
        self.image.published_var(m, v).ok_or_else(|| Trap::Bug(format!("`pub var` {} von Maschine {} fehlt", v.0, m.0)))
    }

    fn state_of(&self, m: MachineId) -> EvalResult<Value> {
        self.image.published_state(m).cloned().ok_or_else(|| Trap::Bug(format!("Zustand von Maschine {} fehlt", m.0)))
    }

    fn signal(&self, m: MachineId, s: SignalId) -> EvalResult<bool> {
        Ok(self.image.published_signal(m, s))
    }

    fn viol(&mut self, site: SiteId) -> EvalResult<&mut i64> {
        self.state.viol.get_mut(site.index()).ok_or_else(|| Trap::Bug(format!("Bestaetigungsstelle {} fehlt", site.0)))
    }

    fn every(&mut self, counter: CounterId) -> EvalResult<&mut i64> {
        self.state
            .every_next
            .get_mut(counter.index())
            .ok_or_else(|| Trap::Bug(format!("every-Zaehler {} fehlt", counter.0)))
    }

    fn builtin(&self, b: Builtin) -> EvalResult<Value> {
        let m = &self.loaded.program.machines[self.id.index()];
        machine::builtin_value(b, m, self.state, self.tick, self.tick_ns, self.last_fault_value(self.loaded))
    }

    fn period(&self) -> EvalResult<i64> {
        let m = &self.loaded.program.machines[self.id.index()];
        Ok(i64::from(m.period).saturating_mul(self.tick_ns))
    }

    fn var_type(&self, v: VarId) -> EvalResult<TypeId> {
        let m = &self.loaded.program.machines[self.id.index()];
        m.vars.get(v.index()).map(|d| d.ty).ok_or_else(|| Trap::Bug(format!("Variable {} fehlt", v.0)))
    }

    fn observe(&mut self, o: Observation) -> EvalResult<()> {
        self.out.push(o);
        Ok(())
    }

    fn abort(&mut self) -> EvalResult<()> {
        self.aborted = true;
        Ok(())
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
    /// Beobachtungen des laufenden Ticks je Maschine.
    pub observations: Vec<(MachineId, Observation)>,
}

/// Laufende Maschinen: Vorlagen und Szenarien laufen nicht mit (5.8, 13.6).
fn runnable(p: &Program) -> Vec<MachineId> {
    p.machines
        .iter()
        .enumerate()
        .filter(|(_, m)| !matches!(m.kind, MachineKind::Template | MachineKind::Scenario) && !m.states.is_empty())
        .map(|(i, _)| MachineId(i as u32))
        .collect()
}

impl<'p> Sim<'p> {
    /// Neuer Lauf: Outputs auf `safe`, Parameter aus Defaults und Profil.
    pub fn new(program: &'p Program, profile: Option<&str>) -> Result<Sim<'p>, Trap> {
        let loaded = Loaded::load(program).map_err(|d| Trap::Bug(format!("{d}")))?;
        let params = eval_params(&loaded, profile)?;
        let outputs = eval_safe_outputs(&loaded, &params)?;
        let image = Image::new(program, outputs, params);
        let states = program.machines.iter().map(MachineState::new).collect();
        let order = runnable(program);
        Ok(Sim { loaded, states, image, tick: 0, order, observations: Vec::new() })
    }

    /// Tick 0: jede Maschine betritt ihren Anfangszustand (1.5). Wird nach
    /// dem Stimulus des Ticks 0 gerufen, damit `sample()` auch dort vor der
    /// Schrittphase liegt (9.4).
    pub fn init(&mut self) -> Result<(), Trap> {
        let program = self.loaded.program;
        let tick_ns = program.config.tick;
        self.observations.clear();
        self.image.apply_sim_bindings();
        // Ψ traegt im Tick 0 die Anfangswerte der Variablen, damit die
        // `enter`- und Entry-`loop`-Bloecke sie schon lesen koennen (1.4).
        for id in self.order.clone() {
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            env.init_vars(&self.loaded, 0)?;
            self.observations.extend(out.into_iter().map(|o| (id, o)));
        }
        self.publish_all();
        self.image.commit_published();
        for id in self.order.clone() {
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            machine::init(&self.loaded, &mut env, 0)?;
            self.observations.extend(out.into_iter().map(|o| (id, o)));
        }
        // Tick 0 aktiviert jede Maschine mit `phase = 0` (7.2); der Zaehler
        // wird wie am Ende jedes Ticks fortgeschrieben.
        let active: Vec<MachineId> =
            self.order.iter().copied().filter(|id| self.states[id.index()].countdown == 0).collect();
        self.advance_counters(&active);
        self.publish_all();
        self.image.commit_published();
        self.image.commit_outputs();
        Ok(())
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

    /// Ein System-Tick (9.4).
    pub fn step(&mut self) -> Result<(), Trap> {
        let program = self.loaded.program;
        let tick_ns = program.config.tick;
        self.observations.clear();
        self.tick += 1;
        // sample(): Alterung, sim-Bindungen (8.3)
        self.image.age_inputs(program, tick_ns);
        self.image.apply_sim_bindings();
        // active(k): countdown == 0 (7.2)
        let active: Vec<MachineId> =
            self.order.iter().copied().filter(|id| self.states[id.index()].countdown == 0).collect();
        // Schrittphase
        let mut aborted = false;
        for id in &active {
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, *id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            let result = machine::step_m(&self.loaded, &mut env, self.tick);
            aborted |= env.aborted;
            self.observations.extend(out.into_iter().map(|o| (*id, o)));
            result?;
        }
        // abort_phase(): Abort und Runtime-Faults wirken im selben Tick (5.4)
        if aborted {
            self.abort_phase(tick_ns)?;
        }
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
        self.image.commit_outputs();
        self.image.clear_commands();
        for state in &mut self.states {
            state.raised_signals.iter_mut().for_each(|s| *s = false);
        }
        Ok(())
    }

    /// Abort-Phase (5.4, 9.4): jede Maschine nimmt ihren Fault-Pfad.
    fn abort_phase(&mut self, tick_ns: i64) -> Result<(), Trap> {
        for id in self.order.clone() {
            if self.states[id.index()].faulted {
                continue;
            }
            let f = Fault::new(FaultKind::Abort, "abort", takt_diag::Span::default(), self.tick);
            let mut out = Vec::new();
            let mut env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            let result = machine::resolve_m(&self.loaded, &mut env, Err(Trap::Fault(f)), self.tick);
            self.observations.extend(out.into_iter().map(|o| (id, o)));
            result?;
        }
        Ok(())
    }

    /// Veroeffentlicht `pub var`, Zustand und Signale (9.4).
    fn publish_all(&mut self) {
        let program = self.loaded.program;
        let tick_ns = program.config.tick;
        for id in self.order.clone() {
            let machine = &program.machines[id.index()];
            let vars = self.states[id.index()].vars.clone();
            let signals = self.states[id.index()].raised_signals.clone();
            let mut out = Vec::new();
            let env =
                MachineEnv::new(&self.loaded, id, &mut self.states[id.index()], &mut self.image, &mut out, tick_ns);
            let state = env.state_value(&self.loaded);
            self.image.publish(id, machine, &vars, state, &signals);
        }
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
fn eval_params(loaded: &Loaded<'_>, profile: Option<&str>) -> Result<Vec<Value>, Trap> {
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
