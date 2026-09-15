//! Ein Lauf: Stimulus einspeisen, Ticks ausfuehren, Trace schreiben und das
//! Lauf-Verdikt bilden (Referenz 9.4, 13.5; `grammar/trace.md`).

use std::collections::HashMap;

use takt_diag::Span;
use takt_mir::program::{Direction, Overflow, Program};
use takt_mir::types::Type;
use takt_mir::{ChannelId, MachineId, VarId};

use crate::env::Observation;
use crate::stream::Delivery;
use crate::system::Sim;
use crate::trace::{LineKind, Trace, TraceLine, parse_value, sample_from_text, value_text};
use crate::value::{Trap, Value};

/// Lauf-Verdikt (13.5): FAIL absorbiert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Mindestens ein `verify` verletzt, ein `verdict fail`, oder ein Fault
    /// hat sein Ziel erreicht (bei `fault_is_fail`).
    Fail,
    /// Mindestens ein `verdict pass`, sonst kein FAIL.
    Pass,
    /// Keine Aussage.
    Inconclusive,
}

impl Verdict {
    /// Name im Trace.
    pub fn name(self) -> &'static str {
        match self {
            Verdict::Fail => "FAIL",
            Verdict::Pass => "PASS",
            Verdict::Inconclusive => "INCONCLUSIVE",
        }
    }
}

/// Einstellungen eines Laufs.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Zahl der Ticks.
    pub ticks: u64,
    /// Profil (8.4).
    pub profile: Option<String>,
    /// Reihenfolge der Schritte permutieren (Test zu Satz 9.4.1).
    pub order_seed: Option<u64>,
}

/// Ergebnis eines Laufs.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// Der Trace in kanonischer Ordnung.
    pub trace: Trace,
    /// Lauf-Verdikt (13.5).
    pub verdict: Verdict,
    /// Warum der Lauf endete (12.7).
    pub ended: Ended,
}

/// Warum ein Lauf endete (12.7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ended {
    /// Die gewuenschte Tickzahl ist erreicht.
    #[default]
    Ticks,
    /// `reboot = RESTART`: Neustart nach dem Commit.
    Restart,
    /// `reboot = DEEP_SLEEP`: kein virtueller Tick; der naechste Lauf
    /// beginnt mit `boot_reason = DEEP_SLEEP_WAKE`.
    DeepSleep,
}

impl Ended {
    /// Der Name fuer den Trace.
    pub fn name(self) -> &'static str {
        match self {
            Ended::Ticks => "ticks",
            Ended::Restart => "restart",
            Ended::DeepSleep => "deep_sleep",
        }
    }
}

/// Fuehrt ein Programm mit einem Stimulus aus.
pub fn run(program: &Program, stimulus: &Trace, options: &RunOptions) -> Result<RunResult, Trap> {
    let mut sim = Sim::new(program, options.profile.as_deref())?;
    if let Some(seed) = options.order_seed {
        permute(&mut sim.order, seed);
    }
    let mut writer = Writer::new(program);
    let mut verdict = Verdict::Inconclusive;
    let mut fail = false;

    // Tick 0: Stimulus, dann Anfangszustand und Anfangsausgaben (9.4)
    apply_stimulus(&mut sim, stimulus, 0)?;
    sim.init()?;
    collect(&mut writer, &sim, 0, &mut verdict, &mut fail);
    writer.initial(&sim);

    // Auch der Anfangszustand kann das Kommando setzen (12.7).
    let mut ended = reboot_of(&sim).unwrap_or(Ended::Ticks);
    let mut last = 0;
    if ended != Ended::Ticks {
        writer.lines.push(TraceLine { tick: 0, kind: LineKind::Reboot { reason: ended.name().to_string() } });
        sim.safe_all()?;
        writer.changes(&sim, 0);
    }
    for tick in 1..=options.ticks {
        if ended != Ended::Ticks {
            break;
        }
        sim.age();
        apply_stimulus(&mut sim, stimulus, tick)?;
        sim.step()?;
        collect(&mut writer, &sim, tick, &mut verdict, &mut fail);
        writer.changes(&sim, tick);
        last = tick;
        // 12.7: nach dem Commit, wenn alle Outputs stehen.
        if let Some(e) = reboot_of(&sim) {
            ended = e;
            writer.lines.push(TraceLine { tick, kind: LineKind::Reboot { reason: e.name().to_string() } });
            // Die Zeile markiert die Entscheidung, das `safe` danach die
            // Ausfuehrung (12.7) — so liest der Trace sich von oben nach
            // unten wie der Ablauf.
            sim.safe_all()?;
            writer.changes(&sim, tick);
            break;
        }
    }
    let final_verdict = if fail { Verdict::Fail } else { verdict };
    let at = if ended == Ended::Ticks { options.ticks } else { last };
    writer.lines.push(TraceLine { tick: at, kind: LineKind::Final { verdict: final_verdict.name().to_string() } });
    Ok(RunResult { trace: Trace { lines: writer.lines }, verdict: final_verdict, ended })
}

/// Steht auf `sys/reboot` ein Kommando (12.7)?
fn reboot_of(sim: &Sim<'_>) -> Option<Ended> {
    let program = sim.loaded.program;
    // Nur `hw`: Ein `sim`-Output speist den gleichnamigen `hw`-Input (8.3),
    // und `sys/reboot` hat keinen — die Plattform fuehrt das Kommando aus.
    let (i, c) = program.channels.iter().enumerate().find(|(_, c)| {
        c.dir == Direction::Output
            && matches!(&c.binding, takt_mir::program::Binding::Hw(a) if a.text() == "sys/reboot")
    })?;
    let takt_mir::types::Type::Enum(e) = program.types.list.get(c.ty.index())? else { return None };
    let def = program.enums.get(e.index())?;
    // 12.7: `RebootCmd` ist vordefiniert. Ein fremdes Enum mit einer
    // zufaellig `RESTART` heissenden Variante ist kein Reboot-Kommando.
    if def.name != "RebootCmd" {
        return None;
    }
    let Value::Enum { variant, .. } = sim.image.outputs.get(i)? else { return None };
    match def.variants.get(*variant as usize)?.name.as_str() {
        "RESTART" => Some(Ended::Restart),
        "DEEP_SLEEP" => Some(Ended::DeepSleep),
        _ => None,
    }
}

/// Speist die Stimuluszeilen eines Ticks ein.
fn apply_stimulus(sim: &mut Sim<'_>, stimulus: &Trace, tick: u64) -> Result<(), Trap> {
    let program = sim.loaded.program;
    for line in stimulus.at(tick) {
        match &line.kind {
            LineKind::Input { channel, sample } => {
                let Some(id) = channel_by_name(program, channel) else {
                    return Err(Trap::Bug(format!("Stimulus: Channel `{channel}` gibt es nicht")));
                };
                if program.channels[id.index()].dir != Direction::Input {
                    return Err(Trap::Bug(format!("Stimulus: `{channel}` ist kein Input")));
                }
                let ty = program.channels[id.index()].ty;
                // 8.6: ein Stream-Input traegt kein Latch, sondern ein Element
                // je Zeile; mehrere Zeilen eines Ticks liefern mehrere
                // Elemente in ihrer Reihenfolge.
                if let Some(Type::Stream(elem)) = program.types.list.get(ty.index()).cloned() {
                    let text = sample.value.clone().unwrap_or_default();
                    let value = parse_value(&text, elem, program).map_err(Trap::Bug)?;
                    let attrs = &program.channels[id.index()].attrs;
                    let drop_oldest = matches!(attrs.overflow, Some(Overflow::DropOldest));
                    let t = i64::try_from(tick).unwrap_or(i64::MAX).saturating_mul(program.config.tick);
                    if sim.image.push_element(id, t, value, drop_oldest) == Delivery::Overflow {
                        sim.overflow_channel(id);
                    }
                    continue;
                }
                let s = sample_from_text(sample, ty, program).map_err(Trap::Bug)?;
                let now = i64::try_from(tick).unwrap_or(i64::MAX).saturating_mul(program.config.tick);
                sim.image.set_input(id, s, now, program);
            }
            LineKind::Command { name } => {
                let Some(i) = program.commands.iter().position(|c| c.name == *name) else {
                    return Err(Trap::Bug(format!("Stimulus: Command `{name}` gibt es nicht")));
                };
                sim.image.set_command(takt_mir::CommandId(i as u32));
            }
            LineKind::Abort => {
                for state in &mut sim.states {
                    if !state.faulted {
                        state.pending = Some(crate::value::Fault::new(
                            takt_mir::machine::FaultKind::Abort,
                            "operator abort",
                            takt_diag::Span::default(),
                            tick,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn channel_by_name(p: &Program, name: &str) -> Option<ChannelId> {
    p.channels.iter().position(|c| c.name == name).map(|i| ChannelId(i as u32))
}

/// Sammelt die Beobachtungen eines Ticks in kanonischer Ordnung (T5).
fn collect(writer: &mut Writer, sim: &Sim<'_>, tick: u64, verdict: &mut Verdict, fail: &mut bool) {
    let program = sim.loaded.program;
    let fault_is_fail = program.config.fault_is_fail;
    let mut by_machine: Vec<(MachineId, &Observation)> = sim.observations.iter().map(|(m, o)| (*m, o)).collect();
    by_machine.sort_by_key(|(m, _)| m.0);
    for (id, obs) in by_machine {
        let machine = program.machines[id.index()].name.clone();
        let kind = match obs {
            Observation::Log(text) => LineKind::Log { machine, text: text.clone() },
            Observation::Alert { span, index, active, message, invalid } => {
                if !writer.alert_edge(id, *span, index, *active) {
                    continue;
                }
                // Ein ungueltiger Input laesst den Alert mit Zusatz feuern (3.5).
                let text = if *invalid { format!("{message} (sensor invalid)") } else { message.clone() };
                LineKind::Alert { machine, on: *active, text }
            }
            Observation::Measure { name, value, ty } => {
                let text = match value {
                    Some(v) => value_text(v, *ty, program),
                    None => "<invalid>".to_string(),
                };
                LineKind::Measure { machine, name: name.clone(), value: text }
            }
            Observation::Verify { ok, message, .. } => {
                if !ok {
                    *fail = true;
                }
                LineKind::Verify { machine, ok: *ok, text: message.clone() }
            }
            Observation::Verdict { pass, message } => {
                if *pass {
                    if *verdict == Verdict::Inconclusive {
                        *verdict = Verdict::Pass;
                    }
                } else {
                    *fail = true;
                }
                LineKind::Verdict { machine, pass: *pass, text: message.clone() }
            }
            Observation::Fault { kind, message, target } => {
                if fault_is_fail {
                    *fail = true;
                }
                LineKind::Fault { machine, kind: fault_name(*kind), message: message.clone(), target: target.clone() }
            }
            Observation::Signal { name } => LineKind::Signal { machine, name: name.clone() },
        };
        writer.lines.push(TraceLine { tick, kind });
    }
}

fn fault_name(kind: takt_mir::machine::FaultKind) -> String {
    use takt_mir::machine::FaultKind as F;
    match kind {
        F::CheckFailed => "CheckFailed".into(),
        F::Expect => "Expect".into(),
        F::Timeout => "Timeout".into(),
        F::SensorFault => "SensorFault".into(),
        F::MissingValue => "MissingValue".into(),
        F::Arithmetic(k) => format!("Arithmetic({k:?})"),
        F::Range => "RangeFault".into(),
        F::StreamOverflow => "StreamOverflow".into(),
        F::Timing => "TimingFault".into(),
        F::ScheduleOverflow => "ScheduleOverflow".into(),
        F::JobOverflow => "JobOverflow".into(),
        F::Abort => "Abort".into(),
        F::Runtime(k) => format!("Runtime({k:?})"),
    }
}

/// Schreibt Ausgaben, Zustaende und `pub var`, jeweils nur bei Aenderung (T4).
struct Writer<'p> {
    program: &'p Program,
    lines: Vec<TraceLine>,
    outputs: Vec<Option<String>>,
    states: HashMap<MachineId, String>,
    published: HashMap<(MachineId, VarId), String>,
    alerts: HashMap<(MachineId, Span, Vec<i64>), bool>,
    /// Letzter gemeldeter Stand von `dropped`, `overflowed`, `malformed`.
    counters: HashMap<String, (u32, u32, u32)>,
}

impl<'p> Writer<'p> {
    fn new(program: &'p Program) -> Writer<'p> {
        Writer {
            program,
            lines: Vec::new(),
            outputs: vec![None; program.channels.len()],
            states: HashMap::new(),
            published: HashMap::new(),
            alerts: HashMap::new(),
            counters: HashMap::new(),
        }
    }

    /// Nur die Flanken eines Alerts melden (5.6). Der Schluessel ist die
    /// Stelle samt den Indizes der umgebenden `for`-Schleifen: eine Stelle in
    /// einer Schleife hat je Durchlauf eine eigene Flanke, und der Text taugt
    /// nicht als Schluessel, weil Platzhalter ihn mit jedem Wert aendern. Der
    /// Anfangszustand jeder Stelle ist inaktiv, eine erste inaktive
    /// Auswertung also keine Flanke.
    fn alert_edge(&mut self, m: MachineId, span: Span, index: &[i64], active: bool) -> bool {
        let key = (m, span, index.to_vec());
        let previous = self.alerts.insert(key, active).unwrap_or(false);
        previous != active
    }

    /// Tick 0: alle Anfangswerte.
    fn initial(&mut self, sim: &Sim<'_>) {
        for id in &running(sim) {
            let path = sim.path(*id);
            self.states.insert(*id, path.clone());
            self.lines.push(TraceLine {
                tick: 0,
                kind: LineKind::State { machine: self.program.machines[id.index()].name.clone(), path },
            });
        }
        for (i, c) in self.program.channels.iter().enumerate() {
            if c.dir != Direction::Output || self.is_stream(c.ty) {
                continue;
            }
            let id = ChannelId(i as u32);
            let text = value_text(sim.output(id), c.ty, self.program);
            self.outputs[i] = Some(text.clone());
            self.lines.push(TraceLine { tick: 0, kind: LineKind::Output { channel: c.name.clone(), value: text } });
        }
        self.publish(sim, 0);
        // Die Zaehler stehen in Tick 0 auf null; nur ihre Aenderungen sind
        // eine Beobachtung, darum den Anfangsstand nur merken.
        self.stream_counters(sim, 0);
        self.lines.retain(|l| !matches!(l.kind, LineKind::Stream { .. }));
    }

    /// Aenderungen nach einem Tick.
    fn changes(&mut self, sim: &Sim<'_>, tick: u64) {
        for id in &running(sim) {
            let path = sim.path(*id);
            if self.states.get(id) != Some(&path) {
                self.states.insert(*id, path.clone());
                self.lines.push(TraceLine {
                    tick,
                    kind: LineKind::State { machine: self.program.machines[id.index()].name.clone(), path },
                });
            }
        }
        self.publish(sim, tick);
        for (i, c) in self.program.channels.iter().enumerate() {
            if c.dir != Direction::Output {
                continue;
            }
            let id = ChannelId(i as u32);
            if self.is_stream(c.ty) {
                continue;
            }
            let text = value_text(sim.output(id), c.ty, self.program);
            if self.outputs[i].as_deref() != Some(text.as_str()) {
                self.outputs[i] = Some(text.clone());
                self.lines.push(TraceLine { tick, kind: LineKind::Output { channel: c.name.clone(), value: text } });
            }
        }
        self.sent(sim, tick);
        self.stream_counters(sim, tick);
    }

    /// Ist der Typ ein Strom? Ein Ausgabestrom traegt kein Latch (8.8).
    fn is_stream(&self, ty: takt_mir::TypeId) -> bool {
        matches!(self.program.types.list.get(ty.index()), Some(takt_mir::types::Type::Stream(_)))
    }

    /// `out <stream> <element>`: was der Treiber in diesem Tick abgeholt hat
    /// (8.8). Anders als ein Latch erscheint jedes Element, auch ein
    /// wiederholtes.
    fn sent(&mut self, sim: &Sim<'_>, tick: u64) {
        for (i, c) in self.program.channels.iter().enumerate() {
            if c.dir != Direction::Output || !self.is_stream(c.ty) {
                continue;
            }
            let Some(tx) = sim.image.tx.get(&ChannelId(i as u32)) else { continue };
            if tx.sent.is_empty() {
                continue;
            }
            let value = crate::value::Value::Bytes(tx.sent.clone());
            let text = value_text(&value, c.ty, self.program);
            self.lines.push(TraceLine { tick, kind: LineKind::Output { channel: c.name.clone(), value: text } });
        }
    }

    /// `stream <name> dropped=… overflowed=… malformed=…` bei Aenderung
    /// (8.6): die Zaehler sind beobachtbar und gehoeren in den Golden-Trace.
    fn stream_counters(&mut self, sim: &Sim<'_>, tick: u64) {
        let mut seen: Vec<(String, (u32, u32, u32))> = Vec::new();
        for (i, c) in self.program.channels.iter().enumerate() {
            if c.dir != Direction::Input || !self.is_stream(c.ty) {
                continue;
            }
            if let Some(b) = sim.image.channel_bufs.get(&ChannelId(i as u32)) {
                seen.push((c.name.clone(), (b.dropped, b.overflowed, b.malformed)));
            }
        }
        for (i, def) in self.program.streams.iter().enumerate() {
            if let Some(b) = sim.image.stream_bufs.get(i) {
                seen.push((def.name.clone(), (b.dropped, b.overflowed, b.malformed)));
            }
        }
        for (name, now) in seen {
            if self.counters.get(&name) == Some(&now) {
                continue;
            }
            self.counters.insert(name.clone(), now);
            self.lines.push(TraceLine {
                tick,
                kind: LineKind::Stream { name, dropped: now.0, overflowed: now.1, malformed: now.2 },
            });
        }
    }

    /// `pub var` bei Aenderung (T3).
    fn publish(&mut self, sim: &Sim<'_>, tick: u64) {
        for id in &running(sim) {
            let machine = &self.program.machines[id.index()];
            for (i, def) in machine.vars.iter().enumerate() {
                if !def.public {
                    continue;
                }
                let v = VarId(i as u32);
                let Some(value) = sim.states[id.index()].vars.get(i) else { continue };
                let text = value_text(value, def.ty, self.program);
                let key = (*id, v);
                if self.published.get(&key) != Some(&text) {
                    self.published.insert(key, text.clone());
                    self.lines.push(TraceLine {
                        tick,
                        kind: LineKind::Published { machine: machine.name.clone(), var: def.name.clone(), value: text },
                    });
                }
            }
        }
    }
}

/// Laufende Maschinen in Indexreihenfolge (T5): die Aufzeichnung ist von der
/// Schrittreihenfolge unabhaengig (Satz 9.4.1).
fn running(sim: &Sim<'_>) -> Vec<MachineId> {
    let mut ids = sim.order.clone();
    ids.sort_by_key(|m| m.0);
    ids
}

/// Permutiert die Schrittreihenfolge deterministisch (Test zu Satz 9.4.1).
fn permute(order: &mut [MachineId], seed: u64) {
    let mut state = seed | 1;
    for i in (1..order.len()).rev() {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let j = (state >> 33) as usize % (i + 1);
        order.swap(i, j);
    }
}

/// Ein Wert als Text ohne Typ (Messwerte, Diagnosen).
pub fn value_untyped(v: &Value) -> String {
    match v {
        Value::Duration(d) => takt_mir::dump::duration(*d),
        Value::F32(f) => crate::trace::float32_text(*f),
        Value::F64(f) => crate::trace::float_text(*f),
        Value::Int(i) => i.to_string(),
        Value::UInt(u) => u.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => format!("{s:?}"),
        other => format!("{other:?}"),
    }
}
