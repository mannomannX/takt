//! Schritt einer Maschine (Referenz 9.3): `step_m`, `resolve_m`, `switch`,
//! `entered`, `exec_chain`. Die Namen folgen der Referenz, damit die
//! Inventurzeilen `FN-9-*` eine Entsprechung im Code haben.

use std::collections::HashMap;

use takt_diag::Span;
use takt_mir::expr::{Builtin, Expr, ExprKind, StreamRef};
use takt_mir::machine::*;
use takt_mir::stmt::Block;
use takt_mir::*;

use crate::env::{MachineEnv, Observation};
use crate::exec::{Mode, Out};
use crate::loaded::Loaded;
use crate::stream::Element;
use crate::value::{EvalResult, Fault, Trap, Value, bug};

/// Laufzeitzustand einer Maschine (Σ_m, 9.1).
#[derive(Clone, Debug)]
pub struct MachineState {
    /// Aktive Kette von der Wurzel zum Blatt.
    pub conf: Vec<StateId>,
    /// Variablen je `VarId`.
    pub vars: Vec<Value>,
    /// `time_in_state` je Zustand in Ticks der Maschine.
    pub timers: Vec<u64>,
    /// `next`-Zaehler je `every` in Nanosekunden, geschluesselt nach Stelle
    /// und den Indizes der umgebenden `for`-Schleifen (5.6).
    pub every_next: Counters,
    /// Bestaetigungszaehler `viol[site, index]` in Nanosekunden (5.6): eine
    /// Stelle in einer Schleife zaehlt je Durchlauf getrennt, sonst setzten
    /// die erfuellten Durchlaeufe den Zaehler der verletzten zurueck.
    pub viol: Counters,
    /// Vorgemerkter Fault (Operator-Abort, Runtime, Stream-Ueberlauf).
    pub pending: Option<Fault>,
    /// Von einer anderen Maschine erhobener Fault (`abort`, 5.4).
    pub raised: Option<Fault>,
    /// Abort-Latch (5.4): ein Abort wirkt bis zur naechsten normalen Transition.
    pub abort_latched: bool,
    /// `last_fault` (5.3).
    pub last_fault: Option<Fault>,
    /// Abwaertszaehler der Aktivierung (7.2).
    pub countdown: u32,
    /// Signale, die in diesem Tick erhoben wurden.
    pub raised_signals: Vec<bool>,
    /// `cur[s, m]` je gelesenem Stream (9.6), in der Reihenfolge von
    /// `Layout::cursors`.
    pub cursors: Vec<i64>,
    /// `examined[s, m]` der laufenden Aktivierung: die groesste untersuchte
    /// Nummer je Stream, oder -1. Wird in `advance_cursors` verbraucht.
    pub examined: Vec<i64>,
    /// `dropped[s, m]` (5.10): was diese Maschine im Schlaf verpasst hat.
    ///
    /// Getrennt von `Buffer::dropped`, weil nur sie die Elemente verliert
    /// — eine wache Schwester liest sie weiter (9.6).
    pub dropped: Vec<u32>,
    /// War die Maschine im vorigen Tick in einem `idle`-Zustand?
    ///
    /// Fuer den Alert `StreamPaused`, den 5.10 beim *Verlassen* verlangt:
    /// Wer schlaeft, soll beim Aufwachen erfahren, was er verpasst hat.
    pub was_idle: bool,
    /// Ist die Maschine in `FAULTED`?
    pub faulted: bool,
}

/// Zaehler einer Maschine, geschluesselt nach Stelle und den Indizes der
/// umgebenden `for`-Schleifen. Eine Stelle in einer Schleife hat je Durchlauf
/// einen eigenen Zaehler (5.6); die Zahl bleibt statisch beschraenkt, weil
/// jede Schleifenschranke statisch ist (4.3).
#[derive(Clone, Debug, Default)]
pub struct Counters {
    /// (Stelle, Schleifenindizes) -> Zaehlerstand in Nanosekunden.
    slots: HashMap<(u32, Vec<i64>), i64>,
}

impl Counters {
    /// Zaehler einer Stelle, schreibend; unbekannte Schluessel beginnen bei 0.
    pub fn at(&mut self, site: u32, index: &[i64]) -> &mut i64 {
        self.slots.entry((site, index.to_vec())).or_insert(0)
    }

    /// Zaehler einer Stelle, schreibend, mit einem Anfangswert fuer den
    /// ersten Zugriff. `every` beginnt bei `d` statt bei 0 (5.8); `d` ist ein
    /// Ausdruck und beim Zustandseintritt noch nicht bekannt.
    pub fn at_or(&mut self, site: u32, index: &[i64], start: i64) -> &mut i64 {
        self.slots.entry((site, index.to_vec())).or_insert(start)
    }

    /// Setzt alle Zaehler der genannten Stellen zurueck (Zustandseintritt).
    pub fn reset(&mut self, sites: &[usize]) {
        self.slots.retain(|(site, _), _| !sites.contains(&(*site as usize)));
    }
}

impl MachineState {
    /// Anfangszustand: leere Konfiguration; `init` betritt sie im Tick 0.
    pub fn new(m: &Machine) -> MachineState {
        MachineState {
            conf: Vec::new(),
            vars: Vec::new(),
            timers: vec![0; m.states.len()],
            every_next: Counters::default(),
            viol: Counters::default(),
            pending: None,
            raised: None,
            abort_latched: false,
            last_fault: None,
            countdown: m.phase,
            raised_signals: vec![false; m.signals.len()],
            cursors: vec![0; m.layout.cursors.len()],
            examined: vec![-1; m.layout.cursors.len()],
            dropped: vec![0; m.layout.cursors.len()],
            was_idle: false,
            faulted: false,
        }
    }

    /// Blattzustand der Konfiguration.
    pub fn leaf(&self) -> Option<StateId> {
        self.conf.last().copied()
    }

    /// Pfad der Konfiguration als Text (`ARMED.IDLE`).
    pub fn path(&self, m: &Machine) -> String {
        if self.faulted {
            return "FAULTED".to_string();
        }
        // Segmente einer Sequenz tragen im MIR den qualifizierten Namen
        // (`IGNITION.S0`, plan/mir.md); der Pfad haengt nur das Blatt an.
        self.conf
            .iter()
            .map(|s| {
                let name = m.states[s.index()].name.as_str();
                name.rsplit('.').next().unwrap_or(name)
            })
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// Kette von der Wurzel zu einem Zustand.
pub fn chain_to(m: &Machine, s: StateId) -> Vec<StateId> {
    let mut out = Vec::new();
    let mut cur = Some(s);
    while let Some(id) = cur {
        out.push(id);
        cur = m.states[id.index()].parent;
    }
    out.reverse();
    out
}

/// Blattpfad ab einem Zustand ueber `initial` (9.3, `switch`).
pub fn descend(m: &Machine, s: StateId) -> Vec<StateId> {
    // Die Konfiguration ist die volle Kette von der Wurzel (9.1); ein Ziel in
    // einem zusammengesetzten Zustand behaelt dessen Vorfahren.
    let mut out = chain_to(m, s);
    let mut cur = s;
    while let Some(init) = m.states[cur.index()].initial {
        out.push(init);
        cur = init;
    }
    out
}

/// Fault-Ziel φ(s) nach 5.3; der als Fault-Ziel der Maschine deklarierte
/// Zustand erbt nicht von ihr.
pub fn fault_target(m: &Machine, s: Option<StateId>) -> FaultTarget {
    let Some(s) = s else { return m.fault_target };
    let mut cur = Some(s);
    while let Some(id) = cur {
        if let Some(t) = m.states[id.index()].fault_target {
            return t;
        }
        cur = m.states[id.index()].parent;
    }
    if m.fault_target == FaultTarget::State(s) { FaultTarget::Faulted } else { m.fault_target }
}

/// Tiefe des Fault-Waldes ab einem Zustand (Lemma 9.3.1).
pub fn fault_depth(m: &Machine) -> u32 {
    let mut worst = 0;
    for i in 0..m.states.len() {
        let mut depth = 0;
        let mut cur = Some(StateId(i as u32));
        let mut seen = 0;
        while let Some(s) = cur {
            match fault_target(m, Some(s)) {
                FaultTarget::Faulted => break,
                FaultTarget::State(next) => {
                    depth += 1;
                    seen += 1;
                    if seen > m.states.len() {
                        break;
                    }
                    cur = Some(next);
                }
            }
        }
        worst = worst.max(depth);
    }
    worst as u32
}

/// Ein Schritt der Maschine (9.3, `step_m`).
pub fn step_m(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, tick: u64) -> Result<(), Trap> {
    let m = env.machine(loaded);
    // Jede Blockinstanz darf je Aktivierung einmal `step` ausfuehren (5.1).
    clear_stepped(&mut env.state.vars);
    let mut out = Ok(Out::Normal);
    if !env.state.faulted {
        // Vorgemerkte Faults zustellen (Operator-Abort, Runtime, 9.6)
        if let Some(f) = env.state.pending.take() {
            // Ein unterdrueckter Abort wird verworfen, nicht aufbewahrt: 5.4
            // sagt „ignoriert weitere Aborts". Bewahrt man ihn auf, feuert er
            // nach der naechsten normalen Transition ohne neue Eingabe erneut.
            if deliverable(&f, env) {
                out = Err(Trap::Fault(f));
            }
        }
        if matches!(out, Ok(Out::Normal)) {
            let chain = env.state.conf.clone();
            out = exec_chain(loaded, env, &chain, Mode::Run, tick);
        }
        // Uebergaenge outer-first, je Zustand in Quelltextreihenfolge (5.2)
        if matches!(out, Ok(Out::Normal)) {
            out = take_transition(loaded, env, tick);
        }
    } else {
        // In FAULTED laufen nur die dort deklarierten Uebergaenge (5.3)
        out = faulted_transition(loaded, env, tick);
    }
    let _ = m;
    resolve_m(loaded, env, out, tick)
}

/// Kann ein vorgemerkter Fault jetzt zugestellt werden (9.6)?
fn deliverable(f: &Fault, env: &MachineEnv<'_, '_>) -> bool {
    match f.kind {
        FaultKind::Abort => !env.state.abort_latched,
        _ => true,
    }
}

/// Erste zutreffende Transition der Kette (5.2, outer-first).
fn take_transition(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, tick: u64) -> Result<Out, Trap> {
    let m = env.machine(loaded);
    let chain = env.state.conf.clone();
    let period = i64::from(m.period) * loaded.program.config.tick;
    for state in chain {
        let transitions = m.states[state.index()].transitions.clone();
        for t in &transitions {
            let fired = match &t.trigger {
                TransTrigger::When(g) => guard_value(loaded, env, g, tick)?,
                TransTrigger::After(d) => {
                    let mut ctx = env.ctx(loaded, tick);
                    let ns = ctx.eval_duration(d)?;
                    // `after d` feuert im ersten Aktivierungs-Tick mit
                    // time_in_state >= d, nie im Entry-Tick (7.1).
                    let elapsed =
                        i64::try_from(env.state.timers[state.index()]).unwrap_or(i64::MAX).saturating_mul(period);
                    elapsed > 0 && elapsed >= ns
                }
            };
            if !fired {
                continue;
            }
            // Aktionen im Modus ENTRY (9.3)
            let mut ctx = env.ctx(loaded, tick);
            match ctx.exec_block(&t.actions, Mode::Entry)? {
                Out::Normal => {}
                other => return Ok(other),
            }
            return Ok(Out::Goto(t.target));
        }
    }
    Ok(Out::Normal)
}

/// Uebergaenge aus `FAULTED` (5.3): Guards ohne implizite Pruefungen.
fn faulted_transition(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, tick: u64) -> Result<Out, Trap> {
    let transitions = env.machine(loaded).faulted.transitions.clone();
    for t in &transitions {
        let fired = match &t.trigger {
            TransTrigger::When(g) => guard_value(loaded, env, g, tick)?,
            TransTrigger::After(_) => false,
        };
        if fired {
            let mut ctx = env.ctx(loaded, tick);
            match ctx.exec_block(&t.actions, Mode::Entry)? {
                Out::Normal => {}
                other => return Ok(other),
            }
            return Ok(Out::Goto(t.target));
        }
    }
    Ok(Out::Normal)
}

fn guard_value(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, g: &Guard, tick: u64) -> Result<bool, Trap> {
    match g {
        Guard::Expr(e) => {
            let mut ctx = env.ctx(loaded, tick);
            ctx.eval_bool(e)
        }
        Guard::Match { subject, kind, pattern, binding } => match stream_of(subject) {
            // Ein Stream-Guard sucht das erste passende Element in W und
            // setzt `examined` auf dessen `seq`; was danach kommt, bleibt
            // unkonsumiert (8.7).
            Some(stream) => first_match(loaded, env, stream, tick, *binding, |loaded, env, element| {
                let consts = pattern_consts(loaded, env, pattern, tick)?;
                Ok(crate::pattern::match_value(pattern, *kind, &element.value, &consts))
            }),
            // Auf einem gewoehnlichen Wert ist der Guard der Musterabgleich
            // selbst (8.7).
            None => {
                let mut ctx = env.ctx(loaded, tick);
                match ctx.matches(subject, *kind, pattern, *binding, subject.span)? {
                    Value::Bool(b) => Ok(b),
                    other => bug(format!("Guard ergab {}", other.kind_name())),
                }
            }
        },
        // `when s as e:` trifft das naechste Element des Fensters (8.7).
        Guard::Next { stream, binding } => {
            first_match(loaded, env, *stream, tick, Some(*binding), |_, _, _| Ok(Some(Vec::new())))
        }
    }
}

/// Der Strom, ueber den ein Guard laeuft; `None`, wenn das Subjekt ein
/// gewoehnlicher Wert ist.
fn stream_of(subject: &Expr) -> Option<StreamRef> {
    match &subject.kind {
        ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
        ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
        _ => None,
    }
}

/// Die Konstanten der Feldbedingungen eines Musters (8.7).
fn pattern_consts(
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

/// Sucht das erste Element des Fensters, das `hit` annimmt, bindet es und
/// meldet `examined` (8.7). Elemente davor gelten als untersucht, Elemente
/// danach bleiben im Puffer.
fn first_match(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    stream: StreamRef,
    tick: u64,
    binding: Option<VarId>,
    mut hit: impl FnMut(&Loaded<'_>, &mut MachineEnv<'_, '_>, &Element) -> Result<Option<Vec<Value>>, Trap>,
) -> Result<bool, Trap> {
    let _ = tick;
    for element in env.window(loaded, stream) {
        let Some(caps) = hit(loaded, env, &element)? else { continue };
        if let Some(var) = binding {
            let value = env.element_record(loaded, var, &element, caps)?;
            *env.state.vars.get_mut(var.index()).ok_or_else(|| Trap::Bug("Bindung fehlt".into()))? = value;
        }
        env.mark_examined(loaded, stream, element.seq);
        return Ok(true);
    }
    Ok(false)
}

/// `loop:`-Bloecke der Kette von aussen nach innen (9.3, `exec_chain`).
pub fn exec_chain(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    states: &[StateId],
    mode: Mode,
    tick: u64,
) -> Result<Out, Trap> {
    // Der maschinenweite `loop:` gilt in jedem Zustand (5.6, Interlocks).
    if mode == Mode::Run {
        let block = env.machine(loaded).loop_block.clone();
        let mut ctx = env.ctx(loaded, tick);
        match ctx.exec_block(&block, mode)? {
            Out::Normal => {}
            other => return Ok(other),
        }
    }
    // Handler der Maschinenebene laufen nach ihrem `loop:` (8.7).
    if mode == Mode::Run {
        let handlers = env.machine(loaded).handlers.clone();
        match dispatch(loaded, env, &handlers, tick)? {
            Out::Normal => {}
            other => return Ok(other),
        }
    }
    for s in states {
        let block = env.machine(loaded).states[s.index()].loop_block.clone();
        let mut ctx = env.ctx(loaded, tick);
        match ctx.exec_block(&block, mode)? {
            Out::Normal => {}
            other => return Ok(other),
        }
        // Handler laufen unmittelbar nach dem `loop:` ihrer Ebene, Vorfahren
        // vor Nachfahren (8.7); im Entry-Modus ist das Fenster leer (9.6).
        if mode == Mode::Run {
            let handlers = env.machine(loaded).states[s.index()].handlers.clone();
            match dispatch(loaded, env, &handlers, tick)? {
                Out::Normal => {}
                other => return Ok(other),
            }
        }
    }
    Ok(Out::Normal)
}

/// Handler-Dispatch (9.7): fuer jeden Stream in Deklarationsreihenfolge, je
/// Element in seq-Reihenfolge, je Handler in Quelltextreihenfolge. Der erste
/// passende gewinnt; ein Uebergang stoppt die Verarbeitung und laesst den
/// Rest des Fensters fuer den Folgezustand stehen. Auch ein Element, auf das
/// kein Handler passt, gilt als untersucht.
pub fn dispatch(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    handlers: &[Handler],
    tick: u64,
) -> Result<Out, Trap> {
    if handlers.is_empty() {
        return Ok(Out::Normal);
    }
    // Streams in Deklarationsreihenfolge; jede Ebene sieht dasselbe Fenster.
    let mut streams: Vec<StreamRef> = Vec::new();
    for h in handlers {
        if !streams.contains(&h.stream) {
            streams.push(h.stream);
        }
    }
    for stream in streams {
        let window = env.window(loaded, stream);
        for element in window {
            let mut hit = false;
            for h in handlers.iter().filter(|h| h.stream == stream) {
                let Some(caps) = match_handler(loaded, env, h, &element)? else { continue };
                if let Some(var) = h.binding {
                    let value = env.element_record(loaded, var, &element, caps)?;
                    *env.state.vars.get_mut(var.index()).ok_or_else(|| Trap::Bug("Bindung fehlt".into()))? = value;
                }
                env.mark_examined(loaded, stream, element.seq);
                let body = h.body.clone();
                let mut ctx = env.ctx(loaded, tick);
                let out = ctx.exec_block(&body, Mode::Run)?;
                hit = true;
                if out != Out::Normal {
                    // Der Rest des Fensters bleibt im Puffer (8.7).
                    return Ok(out);
                }
                break;
            }
            if !hit {
                env.mark_examined(loaded, stream, element.seq);
            }
        }
    }
    Ok(Out::Normal)
}

/// Gleicht das Muster eines Handlers gegen ein Element ab; `None` heisst
/// „passt nicht", `Some(caps)` die Werte seiner Captures.
fn match_handler(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    h: &Handler,
    element: &Element,
) -> Result<Option<Vec<Value>>, Trap> {
    let Some((kind, pattern)) = &h.pattern else {
        // Catch-all: jedes Element trifft (8.7).
        return Ok(Some(Vec::new()));
    };
    let consts = match pattern {
        takt_mir::pattern::Pattern::Record { fields, .. } => {
            let mut out = Vec::new();
            for (_, e) in fields {
                let mut ctx = env.ctx(loaded, env.tick);
                out.push(ctx.eval(e)?);
            }
            out
        }
        takt_mir::pattern::Pattern::Text { .. } => Vec::new(),
    };
    Ok(crate::pattern::match_value(pattern, *kind, &element.value, &consts))
}

/// Uebergaenge und Fault-Wald aufloesen (9.3, `resolve_m`).
pub fn resolve_m(
    loaded: &Loaded<'_>,
    env: &mut MachineEnv<'_, '_>,
    first: Result<Out, Trap>,
    tick: u64,
) -> Result<(), Trap> {
    let limit = 1 + fault_depth(env.machine(loaded)) + 1;
    let mut steps = 0;
    let mut out = first;
    loop {
        steps += 1;
        if steps > limit + 1 {
            return bug(format!("resolve_m terminiert nicht (Grenze {limit}, Lemma 9.3.1)"));
        }
        match out {
            Ok(Out::Normal) => return Ok(()),
            Ok(Out::Goto(target)) => {
                env.state.abort_latched = false;
                out = switch(loaded, env, target, tick);
            }
            Ok(Out::Break) | Ok(Out::Return(_)) => return bug("Sprung ausserhalb einer Funktion"),
            Err(Trap::Bug(msg)) => return Err(Trap::Bug(msg)),
            Err(Trap::Fault(f)) => {
                if f.kind == FaultKind::Abort {
                    if env.state.abort_latched {
                        return Ok(());
                    }
                    env.state.abort_latched = true;
                }
                env.state.last_fault = Some(f.clone());
                let leaf = env.state.leaf();
                let target = f.target.unwrap_or(match fault_target(env.machine(loaded), leaf) {
                    FaultTarget::State(s) => Target::State(s),
                    FaultTarget::Faulted => Target::Faulted,
                });
                env.observe_fault(&f, target_name(loaded, env, target));
                out = switch(loaded, env, target, tick);
            }
        }
    }
}

/// Meldung eines Faults ohne eigenen Text: die Art und, wenn vorhanden, der
/// Name des Sequenzschritts, in dem sie entstand (6.2).
fn fault_message(m: &Machine, leaf: Option<StateId>, kind: FaultKind) -> String {
    let step = leaf.and_then(|s| m.states[s.index()].step_name.clone());
    match step {
        Some(name) => format!("{kind:?} in Schritt `{name}`"),
        None => match leaf {
            Some(s) => format!("{kind:?} in `{}`", m.states[s.index()].name),
            None => format!("{kind:?}"),
        },
    }
}

fn target_name(loaded: &Loaded<'_>, env: &MachineEnv<'_, '_>, t: Target) -> String {
    match t {
        Target::Faulted => "FAULTED".to_string(),
        Target::State(s) => env.machine(loaded).states[s.index()].name.clone(),
        Target::Fault(k) => format!("[Fault {k:?}]"),
    }
}

/// Konfigurationswechsel in fester Reihenfolge (9.3, `switch`): neue
/// Konfiguration, `exit` innen nach aussen, Initialisierung, `enter` aussen
/// nach innen im Modus ENTRY.
pub fn switch(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, target: Target, tick: u64) -> Result<Out, Trap> {
    let m = env.machine(loaded);
    let target = match target {
        Target::Fault(kind) => {
            // Ein Timeout einer Sequenz nimmt den Fault-Pfad des Zustands (6.2)
            let leaf = env.state.leaf();
            let f = Fault::new(kind, fault_message(m, leaf, kind), Span::default(), tick);
            env.state.last_fault = Some(f.clone());
            let t = match fault_target(m, leaf) {
                FaultTarget::State(s) => Target::State(s),
                FaultTarget::Faulted => Target::Faulted,
            };
            env.observe_fault(&f, target_name(loaded, env, t));
            t
        }
        other => other,
    };
    let old = env.state.conf.clone();
    let goal = match target {
        Target::Faulted => None,
        Target::State(s) => Some(s),
        Target::Fault(_) => return bug("verschachtelter Fault-Pfad"),
    };
    let new = match goal {
        None => Vec::new(),
        Some(s) => descend(env.machine(loaded), s),
    };
    // (1) neue Konfiguration setzen. Der kleinste gemeinsame Vorfahr liegt
    // echt oberhalb des Zielzustands (9.3): eine Selbsttransition `-> S` aus
    // `S` verlaesst `S` und betritt ihn neu, mit `exit`, `enter`, frischen
    // zustandslokalen Variablen und zurueckgesetzten Timern.
    let mut common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    if let Some(s) = goal {
        let depth = chain_to(env.machine(loaded), s).len();
        common = common.min(depth - 1);
    }
    env.state.conf = new.clone();
    env.state.faulted = matches!(target, Target::Faulted);
    // (2) exit-Bloecke der verlassenen Zustaende, innen nach aussen
    for s in old[common..].iter().rev() {
        let block = env.machine(loaded).states[s.index()].exit.clone();
        let mut ctx = env.ctx(loaded, tick);
        match ctx.exec_block(&block, Mode::Entry) {
            Ok(Out::Normal) => {}
            Ok(other) => return Ok(other),
            Err(e) => return Err(e),
        }
    }
    if env.state.faulted {
        // FAULTED fuehrt keinen Nutzercode aus; die Outputs stehen auf safe (5.3)
        env.safe_outputs(loaded);
        return Ok(Out::Normal);
    }
    // (3) zustandslokale Variablen, Timer, Zaehler der betretenen Zustaende
    let entered: Vec<StateId> = new[common..].to_vec();
    for s in &entered {
        env.enter_state(loaded, *s, tick)?;
    }
    // (4) enter-Bloecke aussen nach innen
    for s in &entered {
        let block = env.machine(loaded).states[s.index()].enter.clone();
        let mut ctx = env.ctx(loaded, tick);
        match ctx.exec_block(&block, Mode::Entry) {
            Ok(Out::Normal) => {}
            Ok(other) => return Ok(other),
            Err(e) => return Err(e),
        }
    }
    // Entry-Tick-Regel: `loop:` der neu betretenen Zustaende im Modus ENTRY (5.2)
    exec_chain(loaded, env, &entered, Mode::Entry, tick)
}

/// Setzt das `step`-Flag jeder Blockinstanz zurueck (5.1).
fn clear_stepped(vars: &mut [Value]) {
    for v in vars {
        if let Value::Block(b) = v {
            b.stepped = false;
        }
    }
}

/// Tick 0: Anfangskonfiguration betreten (`initial`, 1.5). Die
/// Maschinenvariablen sind vorher gesetzt (`MachineEnv::init_vars`).
pub fn init(loaded: &Loaded<'_>, env: &mut MachineEnv<'_, '_>, tick: u64) -> Result<(), Trap> {
    let initial = env.machine(loaded).initial;
    let out = switch(loaded, env, Target::State(initial), tick);
    resolve_m(loaded, env, out, tick)
}

/// Beobachtungen, die eine Maschine in einem Tick erzeugt hat.
pub type Observations = Vec<Observation>;

/// Ein leerer Block fuer Zustaende ohne Inhalt.
pub fn empty_block() -> Block {
    Block::default()
}

/// `time_in_state` als Dauer (9.1): Timer der innersten Ebene mal Periode.
pub fn time_in_state(m: &Machine, state: &MachineState, tick_ns: i64) -> Value {
    let ticks = state.leaf().map_or(0, |s| state.timers[s.index()]);
    Value::Duration(i64::try_from(ticks).unwrap_or(i64::MAX).saturating_mul(i64::from(m.period) * tick_ns))
}

/// Eingebaute Groesse einer Maschine (3.3, 5.3).
pub fn builtin_value(
    b: Builtin,
    m: &Machine,
    state: &MachineState,
    tick: u64,
    tick_ns: i64,
    last_fault: Value,
) -> EvalResult<Value> {
    match b {
        Builtin::Now => Ok(Value::Duration(i64::try_from(tick).unwrap_or(i64::MAX).saturating_mul(tick_ns))),
        Builtin::Tick => Ok(Value::Duration(tick_ns)),
        Builtin::TimeInState => Ok(time_in_state(m, state, tick_ns)),
        Builtin::LastFault => Ok(last_fault),
        Builtin::Event => bug("`event` nur in Triggern (7.5)"),
    }
}
