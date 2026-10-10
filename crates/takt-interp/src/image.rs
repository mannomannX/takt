//! Prozessabbild (Referenz 9.1, 9.4): Inputs mit Qualitaet und Alter,
//! Commands als Pulse, Output-Latches, Ψ als Snapshot des Tick-Anfangs.

use std::collections::{HashMap, VecDeque};

use takt_mir::machine::Machine;
use takt_mir::program::{Binding, Direction, Program};
use takt_mir::{ChannelId, CommandId, MachineId, SignalId, VarId};

use takt_mir::types::Type;

use crate::stream::{Buffer, Delivery};
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
    /// `fresh[m]` (9.4): die Maschine ist in dieser Schrittphase schon
    /// gelaufen; Follower lesen sie aus `next` (7.2).
    fresh: Vec<bool>,
    /// Parameterwerte des Laufs (8.4).
    pub params: Vec<Value>,
    /// Aufgezeichnete Laeufe aus dem Stimulus (4.5): Maschine, Slot,
    /// Start-Tick und der Tick der Fertigstellung — `None`, wenn nur `late`
    /// aufgezeichnet ist. Sie ersetzen das Modell `duration`.
    pub job_records: Vec<(MachineId, usize, u64, Option<u64>)>,
    /// Je implizite Pruefstelle der Quelle (Anfang, Ende, Art), wie oft sie
    /// ausgewertet wurde, ohne selbst zu faulten (`coverage::sites`).
    pub site_evals: HashMap<(u32, u32, u8), u64>,
    /// Adresse → `sim`-Output, der einen `hw`-Input speist (8.3).
    sim_sources: HashMap<String, ChannelId>,
    /// Adresse → `hw`-Input.
    hw_inputs: HashMap<String, ChannelId>,
    /// Das zuletzt entnommene Element je Strom an `mmio/ADR/r` (12.10).
    port_last: HashMap<ChannelId, Value>,
    /// Schreibvorgaenge an Registerports in diesem Tick, zugestellt nach
    /// allen Schritten (12.10).
    port_writes: Vec<(ChannelId, i64, Value, bool)>,
    /// Inputs, die der Stimulus in diesem Tick gesetzt hat; ihre
    /// `sim`-Bindung ruht so lange (8.3).
    driven: Vec<bool>,
    /// Die Lieferung dieses Ticks je Input, wie sie am Rand ankam: vom
    /// Stimulus, einer `sim`-Bindung oder als Degradierung. Ohne sie haelt
    /// der Input seine vorige Abtastung, die altert (3.5).
    pub delivered: Vec<Option<Sample>>,
    /// `N` eines Inputs `samples<T, N>`, sonst null (8.9).
    samples_len: Vec<usize>,
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
    /// Der defensive Treiberrand (12.6): Vertrag je Treiber, Range,
    /// `max_slew`, `debounce`. Er steht in `takt-hal`, damit Interpreter
    /// und erzeugter Code denselben Rand benutzen — sonst gaelte Satz
    /// 9.4.4 fuer die Randfaelle nicht. Die Grenzen sind einmal beim Start
    /// gerechnet, nicht in jedem Tick (12.1).
    pub(crate) edge: takt_hal::Edge,
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
    /// `tx.idle` (8.8): zu Tickbeginn gesampelt, in der Simulation genau
    /// dann wahr, wenn der Puffer leer ist (`free == capacity`). Ein `send`
    /// im Tick aendert es erst im naechsten.
    pub idle: bool,
    /// `free` und `idle`, wie eine aufgezeichnete Zeile `tx` sie meldet
    /// (12.5); sie gelten statt des Modells bis zur naechsten Zeile.
    pub reported: Option<(u32, bool)>,
    /// Bytes, die dieser Tick gesendet hat: Um sie sinkt `free` gegenueber
    /// dem gemeldeten Stand.
    pub fresh: u32,
    /// Ist der Strom das Modell eines Ports (`mmio/ADR/r`), leert ihn das
    /// Lesen des Ports statt `max_rate` (12.10).
    pub port: Option<PortRead>,
    /// Behaelt der Puffer die Grenzen seiner Elemente
    /// (`Program::keeps_elements`): die Laengen der wartenden Elemente, die
    /// schon abgeholten Bytes des ersten und die Elemente, die der letzte
    /// Commit vollendet hat.
    pub elements: Option<Elements>,
}

/// Die Elemente eines Sendepuffers, der ihre Grenzen behaelt (8.3, 8.8): je
/// `send` eines, hoechstens `capacity` zugleich.
#[derive(Clone, Debug, Default)]
pub struct Elements {
    /// Die Laengen der Elemente im Puffer, das erste vorn.
    pub lengths: VecDeque<usize>,
    /// Was der Treiber vom ersten schon abgeholt hat.
    pub carry: Vec<u8>,
    /// Was der letzte Commit vollendet hat, in Sendereihenfolge.
    pub done: Vec<Vec<u8>>,
}

/// Der Port als Treiber eines Stroms an `mmio/ADR/r` (12.10): Jedes Lesen
/// entnimmt ein Element, das vor dem Tick im Puffer stand. Frei werden die
/// Bytes erst am Tick-Ende, damit `free` des Modells nicht davon abhaengt,
/// ob es vor oder nach dem Treiber schreitet (Satz 9.4.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct PortRead {
    /// Bytes je Element in kanonischer Form.
    pub size: usize,
    /// Bytes am Anfang des Puffers, die schon vor diesem Tick darin standen.
    pub visible: usize,
    /// Bytes, die das Lesen dieses Ticks entnommen hat.
    pub taken: usize,
}

impl TxBuffer {
    /// Freier Platz (`tx.free`, 8.8): aus dem Modell, oder der gemeldete
    /// Stand des Tickbeginns abzueglich der Sendungen des Ticks.
    pub fn free(&self) -> u32 {
        match self.reported {
            Some((free, _)) => free.saturating_sub(self.fresh),
            None => self.capacity.saturating_sub(self.queued.len() as u32),
        }
    }

    /// Faengt der Puffer kein weiteres Element mehr? Nur wo er Grenzen
    /// behaelt: dort fasst er hoechstens `capacity` Elemente, auch leere.
    pub fn full_of_elements(&self) -> bool {
        self.elements.as_ref().is_some_and(|e| e.lengths.len() >= self.capacity as usize)
    }

    /// Legt ein gesendetes Element an (`send`, 8.8).
    pub fn push(&mut self, bytes: Vec<u8>) {
        self.fresh = self.fresh.saturating_add(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        if let Some(e) = &mut self.elements {
            e.lengths.push_back(bytes.len());
        }
        self.queued.extend(bytes);
    }

    /// Eine Zeile `tx` zu Tickbeginn (12.5).
    pub fn report(&mut self, free: u32, idle: bool) {
        self.reported = Some((free, idle));
        self.idle = idle;
    }

    /// Der Treiber holt bis zu `per_tick` Bytes ab (8.8: „die Simulation
    /// leert exakt `max_rate * T0` Bytes pro Tick"); an einem Port, was
    /// sein Lesen entnommen hat (12.10).
    pub fn drain(&mut self) {
        let n = match &mut self.port {
            Some(port) => std::mem::take(&mut port.taken),
            None => (self.per_tick as usize).min(self.queued.len()),
        };
        self.sent = self.queued.drain(..n).collect();
        if let Some(port) = &mut self.port {
            port.visible = self.queued.len();
        }
        if let Some(e) = &mut self.elements {
            e.done.clear();
            let mut rest = self.sent.as_slice();
            // Ein Element ist da, wenn sein letztes Byte abgeholt ist; ein
            // leeres, sobald es vorn steht.
            while let Some(&len) = e.lengths.front() {
                let need = len - e.carry.len();
                if need > rest.len() {
                    e.carry.extend_from_slice(rest);
                    break;
                }
                e.carry.extend_from_slice(&rest[..need]);
                rest = &rest[need..];
                e.done.push(std::mem::take(&mut e.carry));
                e.lengths.pop_front();
            }
        }
        self.fresh = 0;
        // Was nach dem Commit im Puffer steht, steht zu Beginn des naechsten
        // Ticks darin: dort wird `idle` gesampelt.
        self.idle = self.reported.map_or(self.queued.is_empty(), |(_, idle)| idle);
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
                    // Fenster wie ein Eingabestrom (8.3, Plant-Modelle) mit
                    // so vielen Plaetzen, wie sein Sendepuffer Bytes fasst;
                    // jedes Element belegt mindestens eins.
                    if p.is_read_output(id) {
                        let cap = c.attrs.capacity.unwrap_or(256);
                        let size = match p.types.list.get(c.ty.index()) {
                            Some(Type::Stream(e)) => takt_mir::bytes::max_size(p, *e).unwrap_or(1).max(1),
                            _ => 1,
                        };
                        channel_bufs.insert(id, Buffer::new(cap, cap.saturating_mul(size)));
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
                        TxBuffer {
                            capacity: c.attrs.capacity.unwrap_or(256),
                            per_tick,
                            idle: true,
                            port: port_read(p, c),
                            elements: p.keeps_elements(id).then(Elements::default),
                            ..Default::default()
                        },
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
            port_last: HashMap::new(),
            port_writes: Vec::new(),
            delivered: vec![None; p.channels.len()],
            samples_len: p
                .channels
                .iter()
                .map(|c| match p.types.get(c.ty) {
                    Type::Samples { len, .. } => *len as usize,
                    _ => 0,
                })
                .collect(),
            driven,
            channel_bufs,
            stream_bufs,
            stream_next,
            tx,
            sched: HashMap::new(),
            // 12.6 Zeile 1: Die Klemmtoleranz ist ein Tick, solange die
            // Konfiguration keine andere nennt.
            edge: takt_hal::Edge::new(p, p.channels.iter().map(|c| limits_of(c, p)).collect(), p.config.tick),
            last_good: vec![None; p.channels.len()],
            job_records: Vec::new(),
            site_evals: HashMap::new(),
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
    pub fn set_input(&mut self, c: ChannelId, sample: Sample, now: i64) {
        self.delivered[c.index()] = Some(sample.clone());
        self.inputs[c.index()] = self.through_edge(sample, c, now);
        self.driven[c.index()] = true;
    }

    /// Der Treiber eines Inputs haelt seinen Vertrag nicht (12.6, Zeile 2):
    /// `Bad` mit Grund `Driver`, der Bezugspunkt faellt weg.
    pub fn degrade(&mut self, c: ChannelId) {
        self.edge.driver_bad(c);
        self.last_good[c.index()] = None;
        self.inputs[c.index()] = Sample::bad(Reason::Driver);
        self.driven[c.index()] = true;
        self.delivered[c.index()] = Some(Sample::bad(Reason::Driver));
    }

    /// Ein Element liess sich nicht decodieren (12.6, Zeile 5): verworfen,
    /// `s.malformed` zaehlt.
    pub fn malformed(&mut self, c: ChannelId) {
        self.edge.malformed(c);
        if let Some(buf) = self.channel_bufs.get_mut(&c) {
            buf.malformed = buf.malformed.saturating_add(1);
        }
    }

    /// Fuehrt eine Lieferung durch den Rand (12.6, Zeilen 3 und 4).
    ///
    /// Ein Wert ausserhalb der Range wird `Bad`/`OutOfRange`, ein Wert
    /// jenseits `max_slew` `Bad`/`Implausible` — mit `debounce` zunaechst
    /// `Suspect`, wobei der letzte gute Wert gehalten wird (3.5). Geklemmt
    /// wird nie: Das verbirgt den Fehler, statt ihn sichtbar zu machen.
    fn through_edge(&mut self, sample: Sample, c: ChannelId, now: i64) -> Sample {
        let Some(value) = &sample.value else { return sample };
        if sample.quality == Quality::Bad {
            self.edge.driver_bad(c);
            self.last_good[c.index()] = None;
            return sample;
        }
        // 8.9: „fehlende Samples ergeben Qualitaet `Stale`" — ein Tick-Array
        // unter `N` gilt nicht, wie im erzeugten Code, der `N` zaehlt.
        if let Value::Samples(items) = value
            && items.len() < self.samples_len[c.index()]
        {
            return Sample::stale();
        }
        // 8.9: Bei einem oversampelten Kanal traegt das Element die Range;
        // ein einziges Sample ausserhalb macht das ganze Tick-Array `Bad`
        // (konservativ). Die Steigung misst der Rand am ersten Element.
        let worst = match value {
            Value::Samples(items) => {
                let mut worst = takt_hal::quality::Verdict::good();
                for item in items {
                    let v = self.edge.gate(c, item, now);
                    if severity(v.quality) > severity(worst.quality) {
                        worst = v;
                    }
                }
                worst
            }
            v => self.edge.gate(c, v, now),
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

    /// Ein Schreibvorgang an einem Registerport (12.10) ist ein `send` in
    /// seinen Schreibstrom, das nicht faultet: Er passt, wenn der Ring samt
    /// den Vorgaengen des Ticks unter Kapazitaet und Byteschranke bleibt
    /// (9.6); mit `drop_oldest` verdraengt er erst beim Zustellen und
    /// scheitert nur, wenn er allein die Byteschranke sprengt. Was nicht
    /// passt, zaehlt als `overflowed` (FB-475).
    pub fn queue_port_write(&mut self, c: ChannelId, t: i64, value: Value, drop_oldest: bool) {
        let Some(buf) = self.channel_bufs.get_mut(&c) else { return };
        let bytes = crate::stream::byte_len(&value);
        let rejected = if drop_oldest {
            bytes > buf.cap_bytes || buf.cap == 0
        } else {
            let queued: Vec<u64> = self
                .port_writes
                .iter()
                .filter(|w| w.0 == c)
                .map(|w| u64::from(crate::stream::byte_len(&w.2)))
                .collect();
            let count = buf.items.len() + queued.len() + 1;
            let used = u64::from(buf.bytes) + queued.iter().sum::<u64>() + u64::from(bytes);
            count as u64 > u64::from(buf.cap) || used > u64::from(buf.cap_bytes)
        };
        if rejected {
            buf.overflowed += 1;
        } else {
            self.port_writes.push((c, t, value, drop_oldest));
        }
    }

    /// Stellt die Schreibvorgaenge des Ticks zu, in Reihenfolge, nach allen
    /// Schritten und dem Verwerfen: Ein Modell sieht sie im naechsten Tick,
    /// ob es vor oder nach dem Treiber schreitet (Satz 9.4.1), und vor den
    /// Elementen, die der Rand dann liefert.
    pub fn deliver_port_writes(&mut self) {
        for (c, t, value, drop_oldest) in std::mem::take(&mut self.port_writes) {
            self.push_element(c, t, value, drop_oldest);
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

    /// Maschinen-Replay (12.5): Ψ einer fremden Maschine aus der
    /// Aufzeichnung, nach `next` wie ein Schritt der Maschine.
    pub fn force_published(&mut self, m: MachineId, f: impl FnOnce(&mut Published)) {
        f(&mut self.next[m.index()]);
    }

    /// Maschinen-Replay: Der Schritt der fremden Maschinen kommt aus der
    /// Scheibe — Werte und Zustand bleiben, Signale erloeschen (5.8), und
    /// sie gelten als gelaufen, damit Follower sie frisch lesen (7.2).
    pub fn carry_foreign(&mut self, only: MachineId) {
        for m in 0..self.published.len() {
            if m != only.index() {
                let mut entry = self.published[m].clone();
                entry.signals.iter_mut().for_each(|s| *s = false);
                self.next[m] = entry;
                self.set_fresh(MachineId(m as u32));
            }
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
                self.deliver_sent(p, out, inp, now);
                continue;
            }
            // 8.9: Ein Modell speist einen oversampelten Kanal mit seinem
            // Tick-Array, auch aus einer Konstante vom Typ `[N] T`; der Rand
            // prueft es Abtastwert fuer Abtastwert.
            let value = match (p.types.list.get(p.channels[inp.index()].ty.index()), &self.committed[out.index()]) {
                (Some(Type::Samples { .. }), Value::Array(items)) => Value::Samples(items.clone()),
                (_, v) => v.clone(),
            };
            let sample = Sample::good(value);
            self.delivered[inp.index()] = Some(sample.clone());
            self.inputs[inp.index()] = self.through_edge(sample, inp, now);
        }
        self.driven.iter_mut().for_each(|d| *d = false);
    }

    /// Was der Treiber dem Ausgabestrom `out` beim letzten Commit abnahm,
    /// als Elemente des Stroms `to` mit der Zeit `t` (8.3): je vollendetes
    /// `send` eines, genau wie gesendet, wenn der Puffer die Grenzen behaelt;
    /// sonst aus den Bytes (`elements_of`). Was sich nicht lesen laesst,
    /// zaehlt als `malformed`.
    pub fn deliver_sent(&mut self, p: &Program, out: ChannelId, to: ChannelId, t: i64) {
        let (blocks, kept) = match self.tx.get(&out) {
            Some(tx) => match &tx.elements {
                Some(e) => (e.done.clone(), true),
                None if tx.sent.is_empty() => (Vec::new(), false),
                None => (vec![tx.sent.clone()], false),
            },
            None => (Vec::new(), false),
        };
        let drop_oldest =
            matches!(p.channels[to.index()].attrs.overflow, Some(takt_mir::program::Overflow::DropOldest));
        let ty = p.channels[to.index()].ty;
        for block in blocks {
            let values = if kept { vec![Some(kept_element(&block, ty, p))] } else { elements_of(&block, ty, p) };
            for value in values {
                match value {
                    Some(v) => {
                        self.push_element(to, t, v, drop_oldest);
                    }
                    None => self.malformed(to),
                }
            }
        }
    }

    /// Das naechste Element des Stroms `out` an `mmio/ADR/r` (12.10), das
    /// vor diesem Tick im Puffer stand; ist keins mehr da, das zuletzt
    /// entnommene, vor dem ersten nichts. `Err`, wenn die Bytes kein Element
    /// sind — sie stammen aus `send`, das waere ein Fehler des Interpreters.
    pub fn port_next(&mut self, out: ChannelId, p: &Program) -> Result<Option<Value>, String> {
        let Some(Type::Stream(elem)) = p.types.list.get(p.channels[out.index()].ty.index()) else {
            return Err(format!("Channel {} ist kein Strom", out.0));
        };
        let Some(tx) = self.tx.get_mut(&out) else { return Err(format!("Channel {} ohne Sendepuffer", out.0)) };
        let Some(port) = tx.port.as_mut() else { return Err(format!("Channel {} ist kein Portmodell", out.0)) };
        if port.taken + port.size <= port.visible {
            let chunk = &tx.queued[port.taken..port.taken + port.size];
            let value = crate::bytes::decode_slot(p, chunk, *elem).map_err(|e| format!("{e:?}"))?;
            port.taken += port.size;
            self.port_last.insert(out, value);
        }
        Ok(self.port_last.get(&out).cloned())
    }

    /// Laesst alle Inputs um einen Tick altern; ueberschreitet das Alter
    /// `max_age`, wird die Abtastung `Stale` (3.5).
    pub fn age_inputs(&mut self, p: &Program, tick_ns: i64) {
        self.delivered.fill(None);
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
pub fn limits_of(c: &takt_mir::program::Channel, p: &Program) -> takt_hal::quality::Limits {
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
    takt_hal::quality::Limits { range, max_slew, debounce: c.attrs.debounce.unwrap_or(0) }
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
    match p.types.list.get(ty.index()) {
        Some(Type::Stream(elem)) => wire_element(bytes, *elem, p),
        _ => Value::Bytes(bytes.to_vec()),
    }
}

/// Die Drahtform eines Elements vom Typ `elem` als Wert: Text fuer `line`
/// und `str`, die kanonische Form fuer ein Capture-Fenster, sonst die Bytes.
pub fn wire_element(bytes: &[u8], elem: takt_mir::TypeId, p: &Program) -> Value {
    match p.types.list.get(elem.index()) {
        Some(Type::Line { .. }) => {
            Value::Line { text: String::from_utf8_lossy(bytes).trim_end().to_string(), truncated: false }
        }
        Some(Type::Str { .. }) => Value::Str(String::from_utf8_lossy(bytes).to_string()),
        // 8.9: Ein Capture-Fenster kommt in seiner kanonischen Byteform.
        Some(Type::Capture { .. }) => crate::bytes::decode(p, bytes, elem).unwrap_or(Value::Bytes(bytes.to_vec())),
        _ => Value::Bytes(bytes.to_vec()),
    }
}

/// Ein Element, wie `send` es in einen Puffer mit Grenzen legte (8.3): Text
/// und Bytes genau so, ohne die Rahmung des Rands.
fn kept_element(bytes: &[u8], ty: takt_mir::TypeId, p: &Program) -> Value {
    let elem = match p.types.list.get(ty.index()) {
        Some(Type::Stream(e)) => p.types.list.get(e.index()),
        _ => None,
    };
    match elem {
        Some(Type::Line { .. }) => Value::Line { text: String::from_utf8_lossy(bytes).into_owned(), truncated: false },
        Some(Type::Str { .. }) => Value::Str(String::from_utf8_lossy(bytes).into_owned()),
        _ => Value::Bytes(bytes.to_vec()),
    }
}

/// Die Elemente, die ein Byteblock in einem Strom ergibt: je Byte eines in
/// einem `stream<u8>` — `send` eines `bytes<N>` schickt N Elemente (8.8) —,
/// sonst ein Element. `None` steht fuer ein Element, dessen `decode`
/// misslingt (12.6, Zeile 5).
pub fn elements_of(bytes: &[u8], ty: takt_mir::TypeId, p: &Program) -> Vec<Option<Value>> {
    let elem = match p.types.list.get(ty.index()) {
        Some(Type::Stream(e)) => *e,
        _ => return vec![Some(Value::Bytes(bytes.to_vec()))],
    };
    match p.types.list.get(elem.index()) {
        Some(Type::Int { width: takt_mir::types::IntWidth::U8, .. }) => {
            bytes.iter().map(|b| Some(Value::UInt(u64::from(*b)))).collect()
        }
        Some(Type::Line { .. } | Type::Str { .. } | Type::Bytes { .. }) => vec![Some(element_of(bytes, ty, p))],
        // Feste Elementform (plan/m6.md 2.2): der Block traegt so viele
        // Elemente in kanonischer Byteform, wie hineinpassen.
        _ => match takt_mir::bytes::max_size(p, elem) {
            Ok(size) if size > 0 => {
                // 8.6: auch ein Feld ausserhalb seiner Range ist `malformed` (FB-470).
                let decoded = |chunk: &[u8]| crate::bytes::decode_slot(p, chunk, elem).ok();
                bytes
                    .chunks_exact(size as usize)
                    .map(|chunk| decoded(chunk).filter(|v| crate::bytes::in_type(p, v, elem)))
                    .collect()
            }
            _ => vec![Some(element_of(bytes, ty, p))],
        },
    }
}

/// Der Port, dessen Lesekanal der Ausgabestrom `c` ist (12.10): die Bytes
/// eines Elements.
fn port_read(p: &Program, c: &takt_mir::program::Channel) -> Option<PortRead> {
    let Binding::Sim(a) = &c.binding else { return None };
    let Some(Type::Stream(elem)) = p.types.list.get(c.ty.index()) else { return None };
    let key = address_key(a);
    p.ports.iter().any(|port| key == format!("mmio/{:#x}/r", port.address)).then_some(())?;
    let size = takt_mir::bytes::max_size(p, *elem).ok()?;
    Some(PortRead { size: usize::try_from(size).ok()?, ..PortRead::default() })
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
