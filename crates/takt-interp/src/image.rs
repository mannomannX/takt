//! Prozessabbild (Referenz 9.1, 9.4): Inputs mit Qualitaet und Alter,
//! Commands als Pulse, Output-Latches, Ψ als Snapshot des Tick-Anfangs.

use std::collections::HashMap;

use takt_mir::machine::Machine;
use takt_mir::program::{Binding, Direction, Program};
use takt_mir::{ChannelId, CommandId, MachineId, SignalId, VarId};

use takt_mir::types::Type;

use crate::stream::Buffer;
use crate::value::{Quality, Reason, Sample, Value};

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
    /// Parameterwerte des Laufs (8.4).
    pub params: Vec<Value>,
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
                    tx.insert(id, TxBuffer { capacity: c.attrs.capacity.unwrap_or(256), ..Default::default() });
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
            params,
            sim_sources,
            hw_inputs,
            driven,
            channel_bufs,
            stream_bufs,
            stream_next,
            tx,
        }
    }

    /// Abtastung eines Inputs.
    pub fn input(&self, c: ChannelId) -> &Sample {
        &self.inputs[c.index()]
    }

    /// Setzt eine Abtastung (Stimulus). Der Wert durchlaeuft die
    /// Rand-Durchsetzung: die Simulation ist der Treiberrand (12.6).
    pub fn set_input(&mut self, c: ChannelId, sample: Sample, p: &Program) {
        self.inputs[c.index()] = enforce_range(sample, c, p);
        self.driven[c.index()] = true;
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

    /// Ψ: `pub var` einer Maschine.
    pub fn published_var(&self, m: MachineId, v: VarId) -> Option<&Value> {
        self.published[m.index()].vars.get(&v)
    }

    /// Ψ: Zustand einer Maschine.
    pub fn published_state(&self, m: MachineId) -> Option<&Value> {
        self.published[m.index()].state.as_ref()
    }

    /// Ψ: Signal einer Maschine.
    pub fn published_signal(&self, m: MachineId, s: SignalId) -> bool {
        self.published[m.index()].signals.get(s.index()).copied().unwrap_or(false)
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

    /// Tauscht Ψ (Doppelpuffer, 11.2).
    pub fn commit_published(&mut self) {
        std::mem::swap(&mut self.published, &mut self.next);
    }

    /// Speist `sim`-Outputs in die zugehoerigen `hw`-Inputs (8.3, Unit-Delay).
    /// Ein Input, den der Stimulus in diesem Tick gesetzt hat, behaelt seinen
    /// Wert; ohne Stimuluszeile fuehrt die Bindung ihn im naechsten Tick fort.
    pub fn apply_sim_bindings(&mut self, p: &Program) {
        let pairs: Vec<(ChannelId, ChannelId)> = self
            .sim_sources
            .iter()
            .filter_map(|(addr, out)| self.hw_inputs.get(addr).map(|inp| (*out, *inp)))
            .collect();
        for (out, inp) in pairs {
            if self.driven[inp.index()] {
                continue;
            }
            let value = self.committed[out.index()].clone();
            self.inputs[inp.index()] = enforce_range(Sample::good(value), inp, p);
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

/// Rand-Durchsetzung (3.5, 12.6): ein Wert ausserhalb der deklarierten Range
/// des Channels wird `Bad` mit Grund `OutOfRange`, nicht geklemmt und nicht
/// zu einem Fault. Ohne sie waeren die deklarierten Ranges keine gueltigen
/// Annahmen der Intervallanalyse (3.4).
fn enforce_range(sample: Sample, c: ChannelId, p: &Program) -> Sample {
    let Some(value) = &sample.value else { return sample };
    if sample.quality == Quality::Bad {
        return sample;
    }
    let ty = &p.types.list[p.channels[c.index()].ty.index()];
    let range = match ty {
        takt_mir::types::Type::Int { range, .. }
        | takt_mir::types::Type::Float { range, .. }
        | takt_mir::types::Type::Duration { range } => *range,
        _ => None,
    };
    let Some(range) = range else { return sample };
    if crate::eval::in_range(value, &range) {
        return sample;
    }
    Sample { value: None, quality: Quality::Bad, age: sample.age, reason: Some(Reason::OutOfRange) }
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
