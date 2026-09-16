//! Prozessabbild (Referenz 9.1, 9.4): Inputs mit Qualitaet und Alter,
//! Commands als Pulse, Output-Latches, Ψ als Snapshot des Tick-Anfangs.

use std::collections::HashMap;

use takt_mir::machine::Machine;
use takt_mir::program::{Binding, Direction, Program};
use takt_mir::{ChannelId, CommandId, MachineId, SignalId, VarId};

use takt_mir::types::Type;

use crate::stream::{Buffer, Delivery};
use crate::value::{Quality, Reason, Sample, Value};
use takt_mir::expr::StreamRef;

/// Veroeffentlichte Groessen einer Maschine (Ψ, 9.1).
#[derive(Clone, Debug, Default)]
pub struct Published {
    /// `pub var` je `VarId`.
    pub vars: HashMap<VarId, Value>,
    /// Zustandspfad als Variante des Zustandstyps.
    pub state: Option<Value>,
    /// Signale, die im vorigen Tick erhoben wurden (5.8).
    pub signals: Vec<bool>,
}

/// Prozessabbild und Ψ.
#[derive(Debug)]
pub struct Image {
    /// Abtastung je Input.
    pub inputs: Vec<Sample>,
    /// Latch je Output: was die besitzende Maschine in diesem Tick schreibt.
    pub outputs: Vec<Value>,
    /// Committete Outputs des vorigen Ticks: was fremde Maschinen lesen
    /// (Unit-Delay, 8.3).
    pub committed: Vec<Value>,
    /// Commands dieses Ticks.
    pub commands: Vec<bool>,
    /// Ψ: Snapshot vom Tick-Anfang.
    pub published: Vec<Published>,
    /// Ψ des laufenden Ticks (wird am Tick-Ende zu `published`).
    pub next: Vec<Published>,
    /// `fresh[m]` (9.4): die Maschine ist in dieser Schrittphase schon
    /// gelaufen; Follower lesen sie aus `next` (7.2).
    fresh: Vec<bool>,
    /// Parameterwerte des Laufs (8.4).
    pub params: Vec<Value>,
    /// Aufgezeichnete Fertigstellungen aus dem Stimulus (4.5): Maschine,
    /// Slot, Tick — sie ersetzen das Modell `duration`.
    pub job_records: Vec<(MachineId, usize, u64)>,
    /// Adresse → `sim`-Output, der einen `hw`-Input speist (8.3).
    sim_sources: HashMap<String, ChannelId>,
    /// Adresse → `hw`-Input.
    hw_inputs: HashMap<String, ChannelId>,
    /// Inputs, die der Stimulus in diesem Tick gesetzt hat; ihre
    /// `sim`-Bindung ruht so lange (8.3).
    driven: Vec<bool>,
    /// `buf[s]` je Stream-Channel (9.1, 9.6).
    pub channel_bufs: HashMap<ChannelId, Buffer>,
    /// `buf[s]` je internem Stream; was in Tick k gesendet wird, ist ab k+1
    /// sichtbar (8.6, Unit-Delay wie Psi).
    pub stream_bufs: Vec<Buffer>,
    /// Was in diesem Tick per `send` in einen internen Stream ging.
    pub stream_next: Vec<Vec<(i64, Value, u32)>>,
    /// Sendepuffer je Ausgabestrom: freier Platz und Warteschlange (8.8).
    pub tx: HashMap<ChannelId, TxBuffer>,
    /// `sched[o]`: geplante Schreibvorgaenge, nach `T` sortiert (9.8). Nur
    /// fuer Outputs, die in einem `at` oder `pulse` vorkommen.
    pub sched: HashMap<ChannelId, Vec<(i64, Value)>>,
    /// Der defensive Treiberrand je Input (12.6): Range, `max_slew`,
    /// `debounce`. Er steht in `takt-hal`, damit Interpreter und erzeugter
    /// Code denselben Rand benutzen — sonst gaelte Satz 9.4.4 fuer die
    /// Randfaelle nicht.
    gates: Vec<takt_hal::quality::Gate>,
    /// Die ausgewerteten Grenzen je Channel; einmal beim Start gerechnet,
    /// nicht in jedem Tick (12.1).
    limits: Vec<takt_hal::quality::Limits>,
    /// Der zuletzt gut gelieferte Wert je Channel; ihn haelt `debounce`
    /// (3.5). Er steht hier und nicht im Rand, weil nur der Interpreter
    /// den Typ des Kanals kennt.
    last_good: Vec<Option<Value>>,
}

/// Sendepuffer eines Ausgabestroms (8.8): der Treiber leert ihn mit
/// `max_rate`, `tx.free` ist der freie Platz zu Tick-Beginn.
#[derive(Clone, Debug, Default)]
pub struct TxBuffer {
    /// Bytes, die auf das Senden warten.
    pub queued: Vec<u8>,
    /// Kapazitaet in Bytes.
    pub capacity: u32,
    /// Bytes, die der Treiber je Tick abholt.
    pub per_tick: u32,
    /// Im Tick gesendete Bytes (fuer den Trace).
    pub sent: Vec<u8>,
}

impl TxBuffer {
    /// Freier Platz (`tx.free`, 8.8).
    pub fn free(&self) -> u32 {
        self.capacity.saturating_sub(self.queued.len() as u32)
    }

    /// Der Treiber holt bis zu `per_tick` Bytes ab (8.8: „die Simulation
    /// leert exakt `max_rate * T0` Bytes pro Tick").
    pub fn drain(&mut self) {
        let n = (self.per_tick as usize).min(self.queued.len());
        self.sent = self.queued.drain(..n).collect();
    }
}

impl Image {
    /// Anfangsabbild: Outputs auf `safe` (1.5), Inputs ohne Wert.
    pub fn new(p: &Program, outputs: Vec<Value>, params: Vec<Value>) -> Image {
        let inputs = p
            .channels
            .iter()
            .map(|c| {
                if c.dir == Direction::Input {
                    Sample::bad(Reason::Driver)
                } else {
                    Sample { value: None, quality: Quality::Bad, age: 0, reason: None }
                }
            })
            .collect();
        let mut sim_sources = HashMap::new();
        let mut hw_inputs = HashMap::new();
        for (i, c) in p.channels.iter().enumerate() {
            let id = ChannelId(i as u32);
            match (&c.binding, c.dir) {
                (Binding::Sim(a), Direction::Output) => {
                    sim_sources.insert(address_key(a), id);
                }
                (Binding::Hw(a), Direction::Input) => {
                    hw_inputs.insert(address_key(a), id);
                }
                _ => {}
            }
        }
        let published = p
            .machines
            .iter()
            .map(|m| Published { signals: vec![false; m.signals.len()], ..Default::default() })
            .collect();
        let next = p
            .machines
            .iter()
            .map(|m| Published { signals: vec![false; m.signals.len()], ..Default::default() })
            .collect();
        let driven = vec![false; p.channels.len()];
        let committed = outputs.clone();
        // Puffer je Stream: Channels mit Stream-Typ und die internen Streams
        // (8.6). Die Schranken stehen im Programm.
        let mut channel_bufs = HashMap::new();
        let mut tx = HashMap::new();
        for (i, c) in p.channels.iter().enumerate() {
            let id = ChannelId(i as u32);
            if !matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))) {
                continue;
            }
            match c.dir {
                Direction::Input => {
                    let cap = c.attrs.capacity.unwrap_or(16);
                    let cap_bytes = c.attrs.capacity_bytes.unwrap_or(cap * 256);
                    channel_bufs.insert(id, Buffer::new(cap, cap_bytes));
                }
                Direction::Output => {
                    // Ein Ausgabestrom, den eine Maschine liest, hat ein
                    // Fenster wie ein Eingabestrom (8.3, Plant-Modelle).
                    if p.machines.iter().any(|m| m.layout.cursors.contains(&StreamRef::Channel(id))) {
                        let cap = c.attrs.capacity.unwrap_or(16);
                        channel_bufs.insert(id, Buffer::new(cap, cap.saturating_mul(256)));
                    }
                    // 8.8: „die Simulation leert exakt `max_rate * T0` Bytes
                    // pro Tick". Ohne `max_rate` holt der Treiber alles ab.
                    let per_tick = match rate_hz(c) {
                        Some(hz) => {
                            let bytes = hz.saturating_mul(p.config.tick as u64) / 1_000_000_000;
                            u32::try_from(bytes.max(1)).unwrap_or(u32::MAX)
                        }
                        None => u32::MAX,
                    };
                    tx.insert(
                        id,
                        TxBuffer { capacity: c.attrs.capacity.unwrap_or(256), per_tick, ..Default::default() },
                    );
                }
            }
        }
        let stream_bufs =
            p.streams.iter().map(|s| Buffer::new(s.capacity, s.capacity_bytes.unwrap_or(s.capacity * 256))).collect();
        let stream_next = p.streams.iter().map(|_| Vec::new()).collect();
        Image {
            inputs,
            outputs,
            committed,
            commands: vec![false; p.commands.len()],
            published,
            next,
            fresh: vec![false; p.machines.len()],
            params,
            sim_sources,
            hw_inputs,
            driven,
            channel_bufs,
            stream_bufs,
            stream_next,
            tx,
            sched: HashMap::new(),
            gates: vec![takt_hal::quality::Gate::default(); p.channels.len()],
            limits: p.channels.iter().map(|c| limits_of(c, p)).collect(),
            last_good: vec![None; p.channels.len()],
            job_records: Vec::new(),
        }
    }

    /// Abtastung eines Inputs.
    pub fn input(&self, c: ChannelId) -> &Sample {
        &self.inputs[c.index()]
    }

    /// Setzt eine Abtastung (Stimulus). Der Wert durchlaeuft den
    /// Treiberrand: Die Simulation ist eine Treiberimplementierung, kein
    /// Sonderweg (12.6, Prinzip 4).
    ///
    /// `now` ist der Zeitstempel der Lieferung in Nanosekunden; `max_slew`
    /// ist eine Rate und braucht ihn.
    pub fn set_input(&mut self, c: ChannelId, sample: Sample, now: i64, p: &Program) {
        self.inputs[c.index()] = self.through_edge(sample, c, now, p);
        self.driven[c.index()] = true;
    }

    /// Fuehrt eine Lieferung durch den Rand (12.6, Zeilen 3 und 4).
    ///
    /// Ein Wert ausserhalb der Range wird `Bad`/`OutOfRange`, ein Wert
    /// jenseits `max_slew` `Bad`/`Implausible` — mit `debounce` zunaechst
    /// `Suspect`, wobei der letzte gute Wert gehalten wird (3.5). Geklemmt
    /// wird nie: Das verbirgt den Fehler, statt ihn sichtbar zu machen.
    fn through_edge(&mut self, sample: Sample, c: ChannelId, now: i64, p: &Program) -> Sample {
        let Some(value) = &sample.value else { return sample };
        if sample.quality == Quality::Bad {
            self.gates[c.index()].driver_bad();
            self.last_good[c.index()] = None;
            return sample;
        }
        // 8.9: Bei einem oversampelten Kanal traegt das Element die Range;
        // ein einziges Sample ausserhalb macht das ganze Tick-Array `Bad`
        // (konservativ). Die Steigung misst der Rand am ersten Element.
        let channel = &p.channels[c.index()];
        let limits = self.limits[c.index()];
        let worst = match value {
            Value::Samples(items) => {
                let mut worst = takt_hal::quality::Verdict::good();
                for item in items {
                    let v = self.gates[c.index()].check(item, channel, now, &limits);
                    if severity(v.quality) > severity(worst.quality) {
                        worst = v;
                    }
                }
                worst
            }
            v => self.gates[c.index()].check(v, channel, now, &limits),
        };
        if worst.quality == takt_hal::Quality::Good {
            self.last_good[c.index()] = sample.value.clone();
        }
        let held = worst.held.then(|| self.last_good[c.index()].clone()).flatten();
        if worst.quality == takt_hal::Quality::Bad {
            self.last_good[c.index()] = None;
        }
        apply(sample, worst, held.as_ref())
    }

    /// Legt ein Element eines Eingabestroms ab (8.6). Es kommt vom Rand und
    /// ist darum sofort sichtbar; der Unit-Delay gilt nur fuer interne
    /// Stroeme, deren Schreiber im selben Tick laeuft (9.6).
    pub fn push_element(&mut self, c: ChannelId, t: i64, value: Value, drop_oldest: bool) -> Delivery {
        let bytes = crate::stream::byte_len(&value);
        match self.channel_bufs.get_mut(&c) {
            Some(buf) => buf.push(t, value, bytes, drop_oldest),
            None => Delivery::Ok,
        }
    }

    /// Command dieses Ticks.
    pub fn command(&self, c: CommandId) -> bool {
        self.commands[c.index()]
    }

    /// Setzt ein Command fuer diesen Tick (8.5).
    pub fn set_command(&mut self, c: CommandId) {
        self.commands[c.index()] = true;
    }

    /// Alle Commands zuruecksetzen (Puls gilt einen Tick).
    pub fn clear_commands(&mut self) {
        self.commands.iter_mut().for_each(|c| *c = false);
    }

    /// Latch eines Outputs.
    pub fn output(&self, c: ChannelId) -> &Value {
        &self.outputs[c.index()]
    }

    /// Committeter Wert eines Outputs: was eine fremde Maschine liest
    /// (Unit-Delay, 8.3).
    pub fn committed_output(&self, c: ChannelId) -> &Value {
        &self.committed[c.index()]
    }

    /// Uebernimmt die Latches als committete Werte (Ende des Ticks, 9.4).
    pub fn commit_outputs(&mut self) {
        self.committed.clone_from(&self.outputs);
    }

    /// Latch eines Outputs, schreibend.
    pub fn output_mut(&mut self, c: ChannelId) -> &mut Value {
        &mut self.outputs[c.index()]
    }

    /// Ψ: `pub var` einer Maschine; `fresh` liest `next` (7.2).
    pub fn published_var(&self, m: MachineId, v: VarId, fresh: bool) -> Option<&Value> {
        self.bank(fresh)[m.index()].vars.get(&v)
    }

    /// Ψ: Zustand einer Maschine.
    pub fn published_state(&self, m: MachineId, fresh: bool) -> Option<&Value> {
        self.bank(fresh)[m.index()].state.as_ref()
    }

    /// Ψ: Signal einer Maschine.
    pub fn published_signal(&self, m: MachineId, s: SignalId, fresh: bool) -> bool {
        self.bank(fresh)[m.index()].signals.get(s.index()).copied().unwrap_or(false)
    }

    fn bank(&self, fresh: bool) -> &[Published] {
        if fresh { &self.next } else { &self.published }
    }

    /// Ist die Maschine in dieser Schrittphase schon gelaufen (9.4)?
    pub fn fresh(&self, m: MachineId) -> bool {
        self.fresh.get(m.index()).copied().unwrap_or(false)
    }

    /// `fresh[m] = publish_m(v_m)`: ab jetzt lesen Follower `next` (7.2).
    pub fn set_fresh(&mut self, m: MachineId) {
        if let Some(f) = self.fresh.get_mut(m.index()) {
            *f = true;
        }
    }

    /// Ende der Schrittphase: in der Abort-Phase gilt Ψ_k (7.2).
    pub fn clear_fresh(&mut self) {
        self.fresh.iter_mut().for_each(|f| *f = false);
    }

    /// Traegt die Veroeffentlichung einer Maschine in Ψ_{k+1} ein (9.4).
    pub fn publish(&mut self, m: MachineId, machine: &Machine, vars: &[Value], state: Value, signals: &[bool]) {
        let entry = &mut self.next[m.index()];
        entry.vars.clear();
        for (i, def) in machine.vars.iter().enumerate() {
            if def.public {
                if let Some(v) = vars.get(i) {
                    entry.vars.insert(VarId(i as u32), v.clone());
                }
            }
        }
        entry.state = Some(state);
        entry.signals = signals.to_vec();
    }

    /// `apply_scheduled(k)` (9.8): faellige Schreibvorgaenge in den Latch.
    /// Die Simulation wendet einen Zeitpunkt `T` im Tick `ceil(T / T0)` an.
    pub fn apply_scheduled(&mut self, now: i64) {
        for (o, queue) in &mut self.sched {
            let mut due: Vec<(i64, Value)> = Vec::new();
            queue.retain(|(t, v)| {
                if *t <= now {
                    due.push((*t, v.clone()));
                    false
                } else {
                    true
                }
            });
            // Nach `T` sortiert; der spaeteste faellige Wert gewinnt.
            if let Some((_, v)) = due.into_iter().max_by_key(|(t, _)| *t) {
                self.outputs[o.index()] = v;
            }
        }
        self.sched.retain(|_, q| !q.is_empty());
    }

    /// Leert `sched` aller Outputs einer Maschine (5.3: ein Fault-Uebergang
    /// verwirft die geplanten Schreibvorgaenge).
    pub fn cancel_all_scheduled(&mut self, outputs: &[ChannelId]) {
        for o in outputs {
            self.sched.remove(o);
        }
    }

    /// Tauscht Ψ (Doppelpuffer, 11.2).
    pub fn commit_published(&mut self) {
        std::mem::swap(&mut self.published, &mut self.next);
    }

    /// Speist `sim`-Outputs in die zugehoerigen `hw`-Inputs (8.3, Unit-Delay).
    /// Ein Input, den der Stimulus in diesem Tick gesetzt hat, behaelt seinen
    /// Wert; ohne Stimuluszeile fuehrt die Bindung ihn im naechsten Tick fort.
    pub fn apply_sim_bindings(&mut self, p: &Program, now: i64) {
        let pairs: Vec<(ChannelId, ChannelId)> = self
            .sim_sources
            .iter()
            .filter_map(|(addr, out)| self.hw_inputs.get(addr).map(|inp| (*out, *inp)))
            .collect();
        for (out, inp) in pairs {
            if self.driven[inp.index()] {
                continue;
            }
            // 8.3: ein simulierter Stream-Input wird per `send` gespeist, nicht
            // aus einem Latch. Was der Treiber dem `sim`-Ausgabestrom in
            // diesem Tick abgenommen hat, wird zum Element des `hw`-Inputs;
            // seine `.t` ist die Commit-Zeit.
            if matches!(p.types.list.get(p.channels[inp.index()].ty.index()), Some(Type::Stream(_))) {
                let sent = self.tx.get_mut(&out).map(|t| std::mem::take(&mut t.sent)).unwrap_or_default();
                if !sent.is_empty() {
                    let value = element_of(&sent, p.channels[inp.index()].ty, p);
                    let drop_oldest =
                        matches!(p.channels[inp.index()].attrs.overflow, Some(takt_mir::program::Overflow::DropOldest));
                    self.push_element(inp, now, value, drop_oldest);
                }
                continue;
            }
            let value = self.committed[out.index()].clone();
            // 8.9: „fehlende Samples ergeben Qualitaet `Stale`". Ein leeres
            // Tick-Array heisst, dass der Treiber nichts geliefert hat.
            let sample = match &value {
                Value::Samples(items) if items.is_empty() => Sample::bad(Reason::Stale),
                _ => Sample::good(value),
            };
            self.inputs[inp.index()] = self.through_edge(sample, inp, now, p);
        }
        self.driven.iter_mut().for_each(|d| *d = false);
    }

    /// Laesst alle Inputs um einen Tick altern; ueberschreitet das Alter
    /// `max_age`, wird die Abtastung `Stale` (3.5).
    pub fn age_inputs(&mut self, p: &Program, tick_ns: i64) {
        for (i, c) in p.channels.iter().enumerate() {
            if c.dir != Direction::Input {
                continue;
            }
            let s = &mut self.inputs[i];
            s.age = s.age.saturating_add(tick_ns);
            if let Some(max) = c.attrs.max_age {
                // 3.5 formuliert Stale unbedingt ueber das Alter. Ein
                // entprellter Kanal (`Suspect`), dessen Treiber danach
                // ausfaellt, blieb sonst dauerhaft gueltig und hielt den
                // letzten Wert unbegrenzt. Nur `Bad` bleibt `Bad`, weil das
                // die schlechtere Qualitaet ist.
                if s.age > max && s.quality != Quality::Bad {
                    s.quality = Quality::Stale;
                    s.reason = Some(Reason::Stale);
                }
            }
        }
    }
}

/// Uebertraegt das Urteil des Randes auf die Abtastung (3.5).
///
/// Bei `Suspect` wird der letzte gute Wert gehalten und `.valid` bleibt
/// wahr; bei `Bad` faellt der Wert weg, und das Programm entscheidet ueber
/// `.or()`, was das heisst. `held` ist der Wert, den der Kanal zuletzt gut
/// geliefert hat — der Rand kennt ihn nicht, weil er den Typ nicht kennt.
fn apply(sample: Sample, v: takt_hal::quality::Verdict, held: Option<&Value>) -> Sample {
    match v.quality {
        takt_hal::Quality::Good => sample,
        takt_hal::Quality::Suspect => {
            Sample { value: held.cloned(), quality: Quality::Suspect, age: sample.age, reason: v.reason.map(reason_of) }
        }
        _ => Sample { value: None, quality: Quality::Bad, age: sample.age, reason: v.reason.map(reason_of) },
    }
}

/// Wie schlecht ist eine Qualitaet? Bei oversampelten Kanaelen zaehlt die
/// schlechteste (8.9).
fn severity(q: takt_hal::Quality) -> u8 {
    match q {
        takt_hal::Quality::Good => 0,
        takt_hal::Quality::Suspect => 1,
        takt_hal::Quality::Stale => 2,
        takt_hal::Quality::Bad => 3,
    }
}

/// Der Grund des Randes als Grund der Abtastung.
fn reason_of(r: takt_hal::Reason) -> Reason {
    match r {
        takt_hal::Reason::Stale => Reason::Stale,
        takt_hal::Reason::OutOfRange => Reason::OutOfRange,
        takt_hal::Reason::Implausible => Reason::Implausible,
        takt_hal::Reason::Driver => Reason::Driver,
    }
}

/// Die ausgewerteten Grenzen eines Channels fuer den Rand (3.4, 3.5).
///
/// `max_slew` steht als `const_expr` in der MIR; nur ein Literal ist
/// sinnvoll, wie bei `max_rate` (8.6). Die Einheit ist „je Sekunde" —
/// `50 bar/s` steht als 50 da, weil der Wert in der Basiseinheit des
/// Channels gerechnet wird (3.2).
fn limits_of(c: &takt_mir::program::Channel, p: &Program) -> takt_hal::quality::Limits {
    let ty = &p.types.list[c.ty.index()];
    let ty = match ty {
        Type::Samples { elem, .. } => &p.types.list[elem.index()],
        other => other,
    };
    let range = range_of(ty).and_then(|r| Some((const_f64(&r.lo)?, const_f64(&r.hi)?)));
    let max_slew = match c.attrs.max_slew.as_ref().map(|e| &e.kind) {
        Some(takt_mir::expr::ExprKind::Float(f)) => Some(*f),
        Some(takt_mir::expr::ExprKind::Int(n)) => Some(*n as f64),
        _ => None,
    };
    takt_hal::quality::Limits { range, max_slew }
}

/// Eine Grenze als `f64`.
fn const_f64(c: &takt_mir::types::Const) -> Option<f64> {
    match c {
        takt_mir::types::Const::Int(i) => Some(*i as f64),
        takt_mir::types::Const::Float(f) => Some(*f),
        takt_mir::types::Const::Duration(d) => Some(*d as f64),
        takt_mir::types::Const::Bool(_) => None,
    }
}

/// Deutet die gesendeten Bytes als Element des Zieltyps (8.3). Ein
/// `line`-Strom traegt Text, jeder andere die Bytes selbst.
pub fn element_of(bytes: &[u8], ty: takt_mir::TypeId, p: &Program) -> Value {
    let elem = match p.types.list.get(ty.index()) {
        Some(Type::Stream(e)) => *e,
        _ => return Value::Bytes(bytes.to_vec()),
    };
    match p.types.list.get(elem.index()) {
        Some(Type::Line { .. }) => {
            Value::Line { text: String::from_utf8_lossy(bytes).trim_end().to_string(), truncated: false }
        }
        Some(Type::Str { .. }) => Value::Str(String::from_utf8_lossy(bytes).to_string()),
        _ => Value::Bytes(bytes.to_vec()),
    }
}

/// `max_rate` eines Streams in Hz; nur ein Literal, wie im Sema (8.6).
fn rate_hz(c: &takt_mir::program::Channel) -> Option<u64> {
    match &c.attrs.max_rate.as_ref()?.kind {
        takt_mir::expr::ExprKind::Int(n) => u64::try_from(*n).ok(),
        takt_mir::expr::ExprKind::Float(f) if *f >= 0.0 => Some(*f as u64),
        _ => None,
    }
}

/// Die deklarierte Range eines skalaren Typs (3.4).
fn range_of(ty: &takt_mir::types::Type) -> Option<takt_mir::types::Range> {
    match ty {
        takt_mir::types::Type::Int { range, .. }
        | takt_mir::types::Type::Float { range, .. }
        | takt_mir::types::Type::Duration { range } => *range,
        _ => None,
    }
}

/// Adresse als Schluessel (`daq1/ai0`).
pub fn address_key(a: &takt_mir::pattern::Address) -> String {
    a.segments
        .iter()
        .map(|s| match s.range {
            Some((lo, hi)) => format!("{}[{lo}:{hi}]", s.name),
            None => s.name.clone(),
        })
        .collect::<Vec<_>>()
        .join("/")
}
