//! Maschinen, Zustaende, Uebergaenge, Handler und die Sequenz-Oberflaeche
//! (Referenz 5, 6, 8.7, 9.1, 9.3; plan/mir.md 2.4 und 2.8).

use takt_diag::Span;

use crate::expr::{Expr, MatchKind, StreamRef};
use crate::ids::*;
use crate::pattern::Pattern;
use crate::program::Meta;
use crate::stmt::{Block, Stmt, StmtKind};
use crate::types::IntWidth;

/// Sichtbereich einer Variablen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarScope {
    /// Maschinenvariable (nie ueberlagert).
    Machine,
    /// Zustandslokal (5.8): bei jedem Eintritt neu initialisiert, ueberlagert
    /// mit Geschwistern (11.2).
    State(StateId),
    /// Aus einer Sequenz, einem Guard oder Handler gehoben (6.2); Overlay wie `State`.
    Lifted(StateId),
    /// Lokal in einer Funktion oder einem Block.
    Local,
    /// Parameter einer Maschine oder Funktion.
    Param,
}

/// Variable (5.8, 9.1).
#[derive(Clone, Debug, PartialEq)]
pub struct VarDef {
    /// Name.
    pub name: String,
    /// Typ.
    pub ty: TypeId,
    /// Initialwert; fehlt bei Parametern und Bindungen.
    pub init: Option<Expr>,
    /// Sichtbereich.
    pub scope: VarScope,
    /// `pub var`: in Ψ veroeffentlicht.
    pub public: bool,
    /// Position.
    pub span: Span,
}

/// `persist var` (5.9, v1.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistVar {
    /// Variable (POD-Typ, Maschinenebene).
    pub var: VarId,
    /// `min_interval` in Nanosekunden.
    pub min_interval: Option<i64>,
    /// Typ-Hash des Schluessels (Maschine.Variable plus Typ).
    pub type_hash: u64,
}

/// `signal` (5.8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalDef {
    /// Name.
    pub name: String,
    /// Position.
    pub span: Span,
}

/// Arithmetische Fault-Art (4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum ArithKind {
    Overflow,
    DivZero,
    NonFinite,
    Domain,
    Singular,
}

/// Runtime-Fault-Art (5.3, 7.3, 12.6, 12.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum RuntimeKind {
    Overrun,
    Driver,
    Hardware,
    /// Ab v2.
    Node,
}

/// Fault-Arten (5.3, 4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum FaultKind {
    CheckFailed,
    Expect,
    Timeout,
    SensorFault,
    MissingValue,
    Arithmetic(ArithKind),
    Range,
    StreamOverflow,
    Timing,
    ScheduleOverflow,
    Abort,
    Runtime(RuntimeKind),
}

impl FaultKind {
    /// Jede Art mit jeder Nutzlast, in der Reihenfolge des Prelude.
    pub fn all() -> Vec<FaultKind> {
        use ArithKind as A;
        use RuntimeKind as R;
        let mut out = vec![Self::CheckFailed, Self::Expect, Self::Timeout, Self::SensorFault, Self::MissingValue];
        out.extend([A::Overflow, A::DivZero, A::NonFinite, A::Domain, A::Singular].map(Self::Arithmetic));
        out.extend([Self::Range, Self::StreamOverflow, Self::Timing, Self::ScheduleOverflow, Self::Abort]);
        out.extend([R::Overrun, R::Driver, R::Hardware, R::Node].map(Self::Runtime));
        out
    }

    /// Die Variante von `FaultKind` im Prelude (5.3), die `last_fault.kind`
    /// traegt; ohne `message` liest ein Programm dort diesen Namen.
    pub fn prelude_name(self) -> &'static str {
        match self {
            Self::CheckFailed => "CHECK_FAILED",
            Self::Expect => "EXPECT",
            Self::Timeout => "TIMEOUT",
            Self::SensorFault => "SENSOR_FAULT",
            Self::MissingValue => "MISSING_VALUE",
            Self::Arithmetic(_) => "ARITHMETIC",
            Self::Range => "RANGE",
            Self::StreamOverflow => "STREAM_OVERFLOW",
            Self::Timing => "TIMING",
            Self::ScheduleOverflow => "SCHEDULE_OVERFLOW",
            Self::Abort => "ABORT",
            Self::Runtime(_) => "RUNTIME",
        }
    }

    /// Der Name im Trace (`grammar/trace.md`): `CheckFailed`,
    /// `Arithmetic(DivZero)`, `Runtime(Overrun)`.
    pub fn name(self) -> String {
        match self {
            Self::CheckFailed => "CheckFailed".into(),
            Self::Expect => "Expect".into(),
            Self::Timeout => "Timeout".into(),
            Self::SensorFault => "SensorFault".into(),
            Self::MissingValue => "MissingValue".into(),
            Self::Arithmetic(k) => format!("Arithmetic({k:?})"),
            Self::Range => "RangeFault".into(),
            Self::StreamOverflow => "StreamOverflow".into(),
            Self::Timing => "TimingFault".into(),
            Self::ScheduleOverflow => "ScheduleOverflow".into(),
            Self::Abort => "Abort".into(),
            Self::Runtime(k) => format!("Runtime({k:?})"),
        }
    }
}

/// Fault-Ziel φ(s) (5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultTarget {
    /// Deklarierter Zustand.
    State(StateId),
    /// Impliziter Zustand `FAULTED`.
    Faulted,
}

/// Ziel eines Uebergangs oder `->`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Zustand.
    State(StateId),
    /// Explizit `-> FAULTED`.
    Faulted,
    /// Fault-Pfad mit dieser Art (Timeout einer Sequenz, 6.2).
    Fault(FaultKind),
}

/// Guard (5.2, 8.7).
#[derive(Clone, Debug, PartialEq)]
pub enum Guard {
    /// Bool-Ausdruck.
    Expr(Expr),
    /// `x matches P as m` als Guard; erstes passendes Element eines Fensters.
    Match {
        /// Subjekt (Stream oder Wert).
        subject: Expr,
        /// `matches` oder `has` (8.7).
        kind: MatchKind,
        /// Muster.
        pattern: Pattern,
        /// Bindung (gehobene Variable).
        binding: Option<VarId>,
    },
    /// `s as e`: naechstes Element eines Streams.
    Next {
        /// Stream.
        stream: StreamRef,
        /// Bindung.
        binding: VarId,
    },
}

/// Ausloeser eines Uebergangs.
#[derive(Clone, Debug, PartialEq)]
pub enum TransTrigger {
    /// `when g`
    When(Guard),
    /// `after d` (Mindestverweildauer, 7.1).
    After(Expr),
}

/// Starke Transition aus einem `loop:` (`->` bricht ab) oder schwache aus
/// der Transitionsliste (6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TransKind {
    Strong,
    Weak,
}

/// Uebergang (5.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// Ausloeser.
    pub trigger: TransTrigger,
    /// Aktionen (Aktionsblock, Modus ENTRY).
    pub actions: Block,
    /// Ziel.
    pub target: Target,
    /// Art.
    pub kind: TransKind,
    /// Position.
    pub span: Span,
}

/// `on`-Handler (8.7).
#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    /// Stream.
    pub stream: StreamRef,
    /// Muster; ohne Muster Catch-all.
    pub pattern: Option<(MatchKind, Pattern)>,
    /// Bindung (gehobene Variable).
    pub binding: Option<VarId>,
    /// `when g`: laeuft nach dem Muster mit der Bindung; `false` heisst
    /// „passt nicht", das Element gilt als untersucht (8.7, FB-14).
    pub guard: Option<Expr>,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// Reaktion auf `timeout d` (6.2).
#[derive(Clone, Debug, PartialEq)]
pub enum TimeoutAction {
    /// Fault-Art `Timeout` zum Fault-Ziel.
    Fault,
    /// `-> X` als schwache Transition.
    Goto(Target),
    /// `else:`-Aktionsblock (weicher Timeout).
    Else(Block),
}

/// `timeout d …`
#[derive(Clone, Debug, PartialEq)]
pub struct Timeout {
    /// Frist.
    pub duration: Expr,
    /// Reaktion.
    pub action: TimeoutAction,
}

/// Item der Sequenz-Oberflaeche (6.2); `desugar` ersetzt sie durch Zustaende.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum SeqItem {
    /// Gewoehnliche Anweisung; `check` ist ab hier kontinuierlich, `->` beendet das Segment.
    Stmt(Stmt),
    Wait(Expr),
    Until {
        guard: Guard,
        timeout: Option<Timeout>,
        span: Span,
    },
    /// Einmalige Pruefung (Fault-Art `Expect`).
    Expect {
        cond: Expr,
        message: Option<crate::pattern::Format>,
        /// Anforderung (13.4).
        req: Option<String>,
        span: Span,
    },
    /// `repeat n:` mit Zaehlervariable `k_r : int in 0..n = 0` (gehoben).
    Repeat {
        count: Expr,
        counter: VarId,
        body: Vec<SeqItem>,
        span: Span,
    },
    Step {
        name: String,
        body: Vec<SeqItem>,
        span: Span,
    },
}

impl SeqItem {
    /// Wo das Item im Quelltext steht.
    pub fn span(&self) -> Span {
        match self {
            SeqItem::Stmt(s) => s.span,
            SeqItem::Wait(d) => d.span,
            SeqItem::Until { span, .. }
            | SeqItem::Expect { span, .. }
            | SeqItem::Repeat { span, .. }
            | SeqItem::Step { span, .. } => *span,
        }
    }
}

/// Die Dauer einer Sequenz bis `done` in Basis-Ticks (6.2): jede Grenze
/// kostet mindestens einen Tick, `wait d` genau `ceil(d / T0)`, ein
/// `until` hoechstens seinen `timeout` — ohne ihn ist das Ende offen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SequenceTicks {
    /// Untere Schranke, exakt.
    pub min: u64,
    /// Obere Schranke; `None` heisst unbeschraenkt.
    pub max: Option<u64>,
}

/// `sequence:` eines Zustands (Oberflaeche).
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    /// Items.
    pub items: Vec<SeqItem>,
    /// `done`-Flag (gehobene Bool-Variable), lesbar als `<state>.done`.
    pub done: VarId,
    /// Position.
    pub span: Span,
}

/// Gescopte Instanz (5.11, v1.2).
#[derive(Clone, Debug, PartialEq)]
pub struct ScopedInstance {
    /// Instanzmaschine (`MachineKind::Instance`).
    pub machine: MachineId,
    /// Deklarierender Zustand.
    pub scope: StateId,
    /// `resume`: behaelt die Konfiguration ueber Deaktivierungen (5.12).
    pub resume: bool,
    /// Position.
    pub span: Span,
}

/// Zustand (5.1, 5.8, 5.10 bis 5.12).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// Name, maschinenweit eindeutig.
    pub name: String,
    /// Elternzustand; `None` auf Maschinenebene.
    pub parent: Option<StateId>,
    /// Kinder in Deklarationsreihenfolge.
    pub children: Vec<StateId>,
    /// `initial` der Kinder.
    pub initial: Option<StateId>,
    /// `idle` (v1.1).
    pub idle: bool,
    /// `resume` (v1.2): gespeicherter Pfad in `Layout::saved_paths`.
    pub resume: bool,
    /// Zustandslokale und gehobene Variablen.
    pub vars: Vec<VarId>,
    /// `enter:`
    pub enter: Block,
    /// `exit:`
    pub exit: Block,
    /// `loop:`
    pub loop_block: Block,
    /// Handler in Quelltextreihenfolge.
    pub handlers: Vec<Handler>,
    /// Uebergaenge in Quelltextreihenfolge.
    pub transitions: Vec<Transition>,
    /// Explizites `fault -> X`.
    pub fault_target: Option<FaultTarget>,
    /// `sequence:` (nur Oberflaeche).
    pub sequence: Option<Sequence>,
    /// Dauer der Sequenz in Basis-Ticks (6.2, 11.5); `desugar` rechnet sie.
    pub sequence_ticks: Option<SequenceTicks>,
    /// Gescopte Instanzen.
    pub instances: Vec<ScopedInstance>,
    /// Name aus `step "name"` fuer die Telemetrie.
    pub step_name: Option<String>,
    /// Metadaten.
    pub meta: Meta,
    /// Position.
    pub span: Span,
}

impl State {
    /// Leerer Zustand.
    pub fn new(name: impl Into<String>, parent: Option<StateId>) -> Self {
        State {
            name: name.into(),
            parent,
            children: Vec::new(),
            initial: None,
            idle: false,
            resume: false,
            vars: Vec::new(),
            enter: Block::default(),
            exit: Block::default(),
            loop_block: Block::default(),
            handlers: Vec::new(),
            transitions: Vec::new(),
            fault_target: None,
            sequence: None,
            sequence_ticks: None,
            instances: Vec::new(),
            step_name: None,
            meta: Meta::default(),
            span: Span::default(),
        }
    }
}

/// Zustaende von `FAULTED` (5.3): nur explizite Uebergaenge, kein Nutzercode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaultedState {
    /// Uebergaenge aus `state FAULTED:`; Guards ohne implizite Pruefungen.
    pub transitions: Vec<Transition>,
    /// Position.
    pub span: Span,
}

/// Budget einer Maschine (9.4.3), aus M3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// `B_m`: Operationen je Aktivierung.
    pub activation: crate::fns::CostVec,
    /// `F_m`: Fault-Pfad-Budget.
    pub fault_path: crate::fns::CostVec,
}

/// Deklariertes Budget einer Maschine (`with budget = {…}`, 7.2).
///
/// Ohne Deklaration prueft erst die Integration, ob die Summe passt — in
/// einem Projekt mit mehreren Teams faellt die Ueberschreitung dann auf,
/// wenn sie teuer ist. Mit Deklaration ist das Budget ein Vertrag, der
/// lokal und sofort scheitert (Pruefung 62).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeclaredBudget {
    /// `ram = …` in Byte.
    pub ram: Option<u64>,
    /// `wcet = …` in Nanosekunden, je Aktivierung (7.2, 9.4.3).
    ///
    /// Geprueft wird erst mit der kalibrierten Kostentabelle (13.8): Ohne
    /// sie sind `B_m` und `F_m` Operationszahlen, keine Zeiten.
    pub wcet_ns: Option<i64>,
    /// Position der Deklaration.
    pub span: Span,
}

/// Timer eines Zustands (7.1): Tick-Zaehler `time_in_state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timer {
    /// Zustand.
    pub state: StateId,
    /// Darstellung (u32, wenn die laengste Frist unter 2³² Aktivierungen liegt).
    pub width: IntWidth,
}

/// Zaehlerstelle eines `every` oder eines `check … for d`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CounterSite {
    /// Zustand, dessen Eintritt den Zaehler zuruecksetzt; `None` auf Maschinenebene.
    pub state: Option<StateId>,
    /// Position der Anweisung.
    pub span: Span,
}

/// Blockinstanz im Maschinenspeicher (5.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInstance {
    /// Variable, die die Instanz haelt.
    pub var: VarId,
    /// Block.
    pub block: BlockId,
    /// Zahl der Instanzen (`[N] block(...)`).
    pub count: u32,
}

/// Ein Job-Slot (4.5): das Handle und die Native, die es traegt. Der
/// Slot-Index ist die Position in `Layout::job_slots`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobSlot {
    /// Die Handle-Variable.
    pub handle: VarId,
    /// Die native Funktion (`native job`).
    pub native: crate::NativeId,
}

/// Beschreibung von Σ je Maschine (9.1, 11.2, 11.5): alles, was neben den
/// Variablen Speicher braucht. `takt size` liest nur dies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Timer je Zustand.
    pub timers: Vec<Timer>,
    /// `every`-Zaehler (`CounterId`).
    pub every_counters: Vec<CounterSite>,
    /// Bestaetigungszaehler `viol[site]` (`SiteId`).
    pub viol_sites: Vec<CounterSite>,
    /// Blockinstanzen.
    pub block_instances: Vec<BlockInstance>,
    /// Hoechstzahl gleichzeitiger Jobs `K_j` (4.5).
    pub jobs_max: u32,
    /// Die Job-Handles der Maschine in Slot-Reihenfolge (4.5).
    pub job_slots: Vec<JobSlot>,
    /// `resume`-Zustaende mit gespeichertem Pfad (5.12).
    pub saved_paths: Vec<StateId>,
    /// Trigger mit `armed`-Flag (7.5).
    pub trigger_flags: Vec<TriggerId>,
    /// Eigene Outputs mit Sendepuffer oder `sched`-Warteschlange (8.8, 9.8).
    pub output_queues: Vec<ChannelId>,
    /// Gelesene Streams mit Cursor `cur[s, m]` (9.6).
    pub cursors: Vec<StreamRef>,
}

/// Herkunft einer Instanzmaschine.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceInfo {
    /// Vorlage (`MachineKind::Template`).
    pub template: MachineId,
    /// Argumente in Parameterreihenfolge.
    pub args: Vec<Expr>,
    /// Index in einem Instanz-Array und dessen Laenge.
    pub array: Option<(u32, u32)>,
}

/// Rolle einer Maschine.
#[derive(Clone, Debug, PartialEq)]
pub enum MachineKind {
    /// Gewoehnliche Maschine.
    Regular,
    /// Vorlage mit Parametern; laeuft nie selbst.
    Template,
    /// Instanz einer Vorlage (5.8, 5.11).
    Instance(InstanceInfo),
    /// Szenario (13.6, v1.1): schreibt nur `sim`-Outputs.
    Scenario,
}

/// Maschine (5.1, 9.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    /// Name; Instanzen `maschine.ZUSTAND.inst` (5.11).
    pub name: String,
    /// Rolle.
    pub kind: MachineKind,
    /// `driver machine`.
    pub driver: bool,
    /// `with polling = unchecked`: Pruefung 59 ausdruecklich freigegeben.
    pub polling_unchecked: bool,
    /// Ein Szenario mit `with fault_is_fail = …` (13.5): Es gilt fuer
    /// dessen Lauf statt des Werts aus `system:`.
    pub fault_is_fail: Option<bool>,
    /// Parameter einer Vorlage.
    pub params: Vec<crate::fns::FnParam>,
    /// Periode `n_m` in Basis-Ticks.
    pub period: u32,
    /// Phase in Basis-Ticks.
    pub phase: u32,
    /// `follows` (7.2, v1.1).
    pub follows: Vec<MachineId>,
    /// Knoten (12.9, v2); `None` ist der Hauptknoten.
    pub node: Option<NodeId>,
    /// Alle Variablen (Maschine, Zustaende, gehoben, Parameter).
    pub vars: Vec<VarDef>,
    /// `persist var` (v1.1).
    pub persist: Vec<PersistVar>,
    /// Signale.
    pub signals: Vec<SignalDef>,
    /// Fault-Ziel der Maschine.
    pub fault_target: FaultTarget,
    /// Zustandsbaum; `StateId` ist der Index.
    pub states: Vec<State>,
    /// Zustaende der obersten Ebene.
    pub roots: Vec<StateId>,
    /// `initial`.
    pub initial: StateId,
    /// Maschinenweiter `loop:`.
    pub loop_block: Block,
    /// Handler auf Maschinenebene.
    pub handlers: Vec<Handler>,
    /// `FAULTED`.
    pub faulted: FaultedState,
    /// Speicherbeschreibung.
    pub layout: Layout,
    /// Budget, aus M3.
    pub budget: Option<Budget>,
    /// Deklariertes Budget (7.2), aus `with budget = {…}`.
    pub declared_budget: Option<DeclaredBudget>,
    /// Metadaten.
    pub meta: Meta,
    /// Position.
    pub span: Span,
}

impl Machine {
    /// Alle Bloecke: Maschinenebene, `FAULTED`, je Zustand Eintritt,
    /// Austritt, Schleife, Handler und Uebergaenge.
    pub fn blocks(&self) -> Vec<&Block> {
        let mut out = vec![&self.loop_block];
        out.extend(self.handlers.iter().map(|h| &h.body));
        out.extend(self.faulted.transitions.iter().map(|t| &t.actions));
        for s in &self.states {
            out.extend([&s.enter, &s.exit, &s.loop_block]);
            out.extend(s.handlers.iter().map(|h| &h.body));
            out.extend(s.transitions.iter().map(|t| &t.actions));
        }
        out
    }

    /// Leere Maschine mit Periode 1; `initial` zeigt auf den ersten Zustand,
    /// der hinzugefuegt wird.
    pub fn new(name: impl Into<String>) -> Self {
        Machine {
            name: name.into(),
            kind: MachineKind::Regular,
            driver: false,
            polling_unchecked: false,
            fault_is_fail: None,
            params: Vec::new(),
            period: 1,
            phase: 0,
            follows: Vec::new(),
            node: None,
            vars: Vec::new(),
            persist: Vec::new(),
            signals: Vec::new(),
            fault_target: FaultTarget::Faulted,
            states: Vec::new(),
            roots: Vec::new(),
            initial: StateId(0),
            loop_block: Block::default(),
            handlers: Vec::new(),
            faulted: FaultedState::default(),
            // 4.5: hoechstens K_j gleichzeitige Jobs je Maschine, Default 2.
            layout: Layout { jobs_max: 2, ..Layout::default() },
            budget: None,
            declared_budget: None,
            meta: Meta::default(),
            span: Span::default(),
        }
    }

    /// Fuegt einen Zustand ein und traegt ihn beim Elternzustand oder in
    /// `roots` ein.
    pub fn add_state(&mut self, state: State) -> StateId {
        let id = StateId(self.states.len() as u32);
        match state.parent {
            Some(p) => self.states[p.index()].children.push(id),
            None => self.roots.push(id),
        }
        self.states.push(state);
        id
    }

    /// Fuegt eine Variable ein.
    pub fn add_var(&mut self, var: VarDef) -> VarId {
        let id = VarId(self.vars.len() as u32);
        if let VarScope::State(s) | VarScope::Lifted(s) = var.scope {
            self.states[s.index()].vars.push(id);
        }
        self.vars.push(var);
        id
    }

    /// Fault-Ziel φ(s) nach 5.3: explizit, sonst φ des Elternzustands,
    /// sonst das der Maschine — mit der Ausnahme, dass der als Fault-Ziel
    /// der Maschine deklarierte Zustand nicht von ihr erbt, sondern
    /// `FAULTED` bekommt, und mit ihm jedes Kind, das von ihm erbt. Sonst
    /// faende sich der Fault-Wald in einer Schleife wieder (FB-419).
    pub fn fault_target_of(&self, s: StateId) -> FaultTarget {
        let mut cur = s;
        loop {
            let state = &self.states[cur.index()];
            if let Some(t) = state.fault_target {
                return t;
            }
            match state.parent {
                Some(p) => cur = p,
                None if self.fault_target == FaultTarget::State(cur) => return FaultTarget::Faulted,
                None => return self.fault_target,
            }
        }
    }

    /// Die Zyklen der Fault-Pfade (5.3), je einer als Folge seiner Ziele,
    /// beginnend beim kleinsten Zustand. Leer heisst: Jeder Fault-Pfad
    /// endet, spaetestens bei `FAULTED`.
    pub fn fault_cycles(&self) -> Vec<Vec<StateId>> {
        let graph = FaultGraph::of(self);
        let mut color = vec![0u8; graph.nodes.len()];
        let mut cycles: Vec<Vec<StateId>> = Vec::new();
        for start in 0..graph.nodes.len() {
            if color[start] != 0 {
                continue;
            }
            // Tiefensuche ohne Rekursion: je Knoten auf dem Pfad die Kante,
            // ueber die er erreicht wurde, und die noch offenen Kanten.
            let mut path: Vec<(usize, Option<StateId>)> = vec![(start, None)];
            let mut open = vec![graph.edges[start].clone()];
            color[start] = 1;
            while let Some(edges) = open.last_mut() {
                let Some((next, via)) = edges.pop() else {
                    let (done, _) = path.pop().expect("Pfad");
                    color[done] = 2;
                    open.pop();
                    continue;
                };
                match color[next] {
                    0 => {
                        color[next] = 1;
                        path.push((next, Some(via)));
                        open.push(graph.edges[next].clone());
                    }
                    1 => {
                        let from = path.iter().position(|(n, _)| *n == next).expect("auf dem Pfad");
                        let mut cycle: Vec<StateId> = path[from + 1..].iter().filter_map(|(_, v)| *v).collect();
                        cycle.push(via);
                        let low = cycle.iter().enumerate().min_by_key(|(_, s)| **s).map_or(0, |(i, _)| i);
                        cycle.rotate_left(low);
                        if !cycles.contains(&cycle) {
                            cycles.push(cycle);
                        }
                    }
                    _ => {}
                }
            }
        }
        cycles
    }

    /// Die laengste Folge von Fault-Wechseln in einem Tick (Lemma 9.3.1).
    /// Ein Zyklus zaehlt nicht weiter; Pruefung 9 lehnt ihn ab.
    pub fn fault_depth(&self) -> u32 {
        FaultGraph::of(self).longest().into_iter().max().unwrap_or(0)
    }

    /// Wie viele Fault-Wechsel ein Fault aus `at` (einem Zustand der
    /// aktiven Kette, `None`: der maschinenweite `loop:`) nach `to` im
    /// schlimmsten Fall nimmt, bis `FAULTED`: den ersten, dann einen je
    /// Ziel, das im Entry-Modus erneut scheitert (9.4.5). Gezaehlt wird
    /// ueber jedes Blatt unter `at`.
    pub fn fault_switches(&self, at: Option<StateId>, to: FaultTarget) -> u64 {
        let FaultTarget::State(to) = to else { return 1 };
        let starts: Vec<FaultNode> = (0..self.states.len())
            .map(|i| StateId(i as u32))
            .filter(|l| self.states[l.index()].children.is_empty())
            .filter(|l| at.is_none_or(|a| self.chain_to(Some(*l)).contains(&a)))
            .flat_map(|l| FaultGraph::switch(self, &self.chain_to(Some(l)), to))
            .collect();
        let graph = FaultGraph::build(self, starts.clone());
        let longest = graph.longest();
        starts.iter().map(|n| 2 + u64::from(longest[graph.index[n]])).max().unwrap_or(1)
    }

    /// Die Kette von der Wurzel zu `s`, ohne Zustand leer.
    fn chain_to(&self, s: Option<StateId>) -> Vec<StateId> {
        let mut out = Vec::new();
        let mut cur = s;
        while let Some(id) = cur {
            out.push(id);
            cur = self.states[id.index()].parent;
        }
        out.reverse();
        out
    }

    /// Die Konfiguration, die ein Fault-Wechsel nach `s` betritt: `s` samt
    /// Vorfahren und `initial` abwaerts, nie ein gespeicherter Pfad (5.12).
    fn fault_entry(&self, s: StateId) -> Vec<StateId> {
        let mut out = self.chain_to(Some(s));
        let mut cur = s;
        while let Some(i) = self.states[cur.index()].initial {
            out.push(i);
            cur = i;
        }
        out
    }

    /// Zustand mit diesem Namen.
    pub fn state_named(&self, name: &str) -> Option<StateId> {
        self.states.iter().position(|s| s.name == name).map(|i| StateId(i as u32))
    }

    /// Kern-MIR: keine Sequenz-Oberflaeche mehr (6.2).
    pub fn is_core(&self) -> bool {
        self.states.iter().all(|s| s.sequence.is_none())
    }
}

/// Die Fault-Pfade einer Maschine als Graph (5.3, 9.3, Lemma 9.3.1).
///
/// Ein Knoten ist eine Lage in `resolve_m`: der innerste aktive Zustand,
/// die Zustaende, deren `loop:` danach noch laufen, und ob es der
/// Run-Modus ist (maschinenweiter `loop:` und Handler laufen mit). Eine
/// Kante ist ein Fault-Wechsel, beschriftet mit seinem Ziel: das Fault-Ziel
/// des innersten Zustands oder das eigene Ziel eines `check … -> X`, der
/// dort laeuft. Ein Vorfahr, den der Wechsel nicht verlaesst, laeuft im
/// Entry-Modus nicht erneut; ein Fault in `exit:` setzt beim kleinsten
/// gemeinsamen Vorfahren fort. Ein Graph allein ueber Zustaende saehe
/// weder den Abstieg ueber `initial` noch den Unterschied, ob ein Vorfahr
/// neu betreten wird (FB-419).
struct FaultGraph {
    nodes: Vec<FaultNode>,
    edges: Vec<Vec<(usize, StateId)>>,
    index: std::collections::HashMap<FaultNode, usize>,
}

/// Eine Lage in `resolve_m`, siehe [`FaultGraph`].
#[derive(Clone, PartialEq, Eq, Hash)]
struct FaultNode {
    at: Option<StateId>,
    running: Vec<StateId>,
    run: bool,
}

impl FaultGraph {
    /// Alle Lagen, die ein Fault im Run-Modus eines Blatts erreicht.
    fn of(m: &Machine) -> FaultGraph {
        let starts = (0..m.states.len())
            .map(|i| StateId(i as u32))
            .filter(|s| m.states[s.index()].children.is_empty())
            .map(|leaf| FaultNode { at: Some(leaf), running: m.chain_to(Some(leaf)), run: true })
            .collect();
        FaultGraph::build(m, starts)
    }

    /// Alle Lagen, die Fault-Wechsel ab `starts` erreichen.
    fn build(m: &Machine, starts: Vec<FaultNode>) -> FaultGraph {
        let mut graph = FaultGraph { nodes: Vec::new(), edges: Vec::new(), index: Default::default() };
        let mut todo = Vec::new();
        for n in starts {
            graph.add(n, &mut todo);
        }
        while let Some(from) = todo.pop() {
            for (via, next) in FaultGraph::steps(m, &graph.nodes[from].clone()) {
                let to = graph.add(next, &mut todo);
                if !graph.edges[from].contains(&(to, via)) {
                    graph.edges[from].push((to, via));
                }
            }
        }
        graph
    }

    /// Der Index einer Lage; eine neue kommt in `todo`.
    fn add(&mut self, n: FaultNode, todo: &mut Vec<usize>) -> usize {
        if let Some(i) = self.index.get(&n) {
            return *i;
        }
        let i = self.nodes.len();
        self.index.insert(n.clone(), i);
        self.nodes.push(n);
        self.edges.push(Vec::new());
        todo.push(i);
        i
    }

    /// Je Lage die Zahl der Kanten des laengsten Pfads, der dort beginnt.
    fn longest(&self) -> Vec<u32> {
        fn depth(g: &FaultGraph, n: usize, memo: &mut [Option<u32>], open: &mut [bool]) -> u32 {
            if let Some(d) = memo[n] {
                return d;
            }
            if open[n] {
                return 0;
            }
            open[n] = true;
            let d = g.edges[n].iter().map(|(t, _)| 1 + depth(g, *t, memo, open)).max().unwrap_or(0);
            open[n] = false;
            memo[n] = Some(d);
            d
        }
        let mut memo = vec![None; self.nodes.len()];
        let mut open = vec![false; self.nodes.len()];
        (0..self.nodes.len()).map(|n| depth(self, n, &mut memo, &mut open)).collect()
    }

    /// Die Fault-Wechsel aus einer Lage, mit ihrem Ziel.
    fn steps(m: &Machine, n: &FaultNode) -> Vec<(StateId, FaultNode)> {
        let mut targets = vec![match n.at {
            Some(s) => m.fault_target_of(s),
            None => m.fault_target,
        }];
        let mut push = |st: &Stmt, _: u32| {
            if let StmtKind::Check { target: Some(Target::State(x)), .. } = &st.kind {
                targets.push(FaultTarget::State(*x));
            }
        };
        for s in &n.running {
            let state = &m.states[s.index()];
            crate::visit::walk_stmts(&state.loop_block.stmts, 0, &mut push);
            if n.run {
                state.handlers.iter().for_each(|h| crate::visit::walk_stmts(&h.body.stmts, 0, &mut push));
            }
            // Vor dem Entzuckern stehen Checks noch in der Sequenz ihres Zustands.
            if let Some(seq) = &state.sequence {
                crate::visit::walk_seq(&seq.items, &mut push);
            }
        }
        if n.run {
            crate::visit::walk_stmts(&m.loop_block.stmts, 0, &mut push);
            m.handlers.iter().for_each(|h| crate::visit::walk_stmts(&h.body.stmts, 0, &mut push));
        }
        let mut out = Vec::new();
        let old = m.chain_to(n.at);
        for t in targets {
            let FaultTarget::State(to) = t else { continue };
            for next in FaultGraph::switch(m, &old, to) {
                if !out.contains(&(to, next.clone())) {
                    out.push((to, next));
                }
            }
        }
        out
    }

    /// Die Lagen nach einem Fault-Wechsel von der Kette `old` nach `to`:
    /// betreten bis zum Blatt, oder — scheitert ein `exit:` — beim
    /// kleinsten gemeinsamen Vorfahren (9.3).
    fn switch(m: &Machine, old: &[StateId], to: StateId) -> Vec<FaultNode> {
        let new = m.fault_entry(to);
        let depth = m.chain_to(Some(to)).len();
        let common = old.iter().zip(&new).take_while(|(a, b)| a == b).count().min(depth - 1);
        let mut out = vec![FaultNode { at: new.last().copied(), running: new[common..].to_vec(), run: false }];
        if old[common..].iter().any(|s| !m.states[s.index()].exit.stmts.is_empty()) {
            out.push(FaultNode { at: common.checked_sub(1).map(|i| old[i]), running: Vec::new(), run: false });
        }
        out
    }
}

/// Der Maschinenname eines Szenarios (13.6): der String der Deklaration
/// mit `_` statt Leerraum, damit er in Trace-Zeilen ein Feld bleibt (T1).
pub fn scenario_name(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join("_")
}

/// Wo eine gescopte Instanz haengt (5.11): Besitzer und Zustand.
///
/// Der Index faellt aus den Zustaenden; er liegt hier, damit Interpreter
/// und Codegen dieselbe Quelle lesen und die Aktivitaet nicht zweimal
/// hergeleitet wird.
pub fn scope_of(p: &crate::Program, inst: MachineId) -> Option<(MachineId, ScopedInstance)> {
    for (i, m) in p.machines.iter().enumerate() {
        for s in &m.states {
            if let Some(si) = s.instances.iter().find(|si| si.machine == inst) {
                return Some((MachineId(i as u32), si.clone()));
            }
        }
    }
    None
}

/// Alle gescopten Instanzen eines Programms mit ihrem Besitzer.
pub fn scoped_instances(p: &crate::Program) -> Vec<(MachineId, ScopedInstance)> {
    let mut out = Vec::new();
    for (i, m) in p.machines.iter().enumerate() {
        for s in &m.states {
            out.extend(s.instances.iter().map(|si| (MachineId(i as u32), si.clone())));
        }
    }
    out
}
