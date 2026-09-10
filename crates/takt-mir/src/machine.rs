//! Maschinen, Zustaende, Uebergaenge, Handler und die Sequenz-Oberflaeche
//! (Referenz 5, 6, 8.7, 9.1, 9.3; plan/mir.md 2.4 und 2.8).

use takt_diag::Span;

use crate::expr::{Expr, MatchKind, StreamRef};
use crate::ids::*;
use crate::pattern::Pattern;
use crate::program::Meta;
use crate::stmt::{Block, Stmt};
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

/// Runtime-Fault-Art (5.3, 7.3, 12.6, 12.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum RuntimeKind {
    Overrun,
    Driver,
    Watchdog,
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
    JobOverflow,
    Abort,
    Runtime(RuntimeKind),
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
    /// `resume`-Zustaende mit gespeichertem Pfad (5.12).
    pub saved_paths: Vec<StateId>,
    /// Trigger mit `armed`-Flag (7.5).
    pub trigger_flags: Vec<TriggerId>,
    /// Eigene Outputs mit Sendepuffer oder `sched`-Warteschlange (8.8, 9.8).
    pub output_queues: Vec<ChannelId>,
    /// Gelesene Streams mit Cursor `cur[s, m]` (9.6).
    pub cursors: Vec<StreamRef>,
    /// Statischer Scratch in Bytes (11.2), aus M3.
    pub scratch_bytes: Option<u32>,
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
    /// Metadaten.
    pub meta: Meta,
    /// Position.
    pub span: Span,
}

impl Machine {
    /// Leere Maschine mit Periode 1; `initial` zeigt auf den ersten Zustand,
    /// der hinzugefuegt wird.
    pub fn new(name: impl Into<String>) -> Self {
        Machine {
            name: name.into(),
            kind: MachineKind::Regular,
            driver: false,
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
            layout: Layout::default(),
            budget: None,
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

    /// Zustand mit diesem Namen.
    pub fn state_named(&self, name: &str) -> Option<StateId> {
        self.states.iter().position(|s| s.name == name).map(|i| StateId(i as u32))
    }

    /// Kern-MIR: keine Sequenz-Oberflaeche mehr (6.2).
    pub fn is_core(&self) -> bool {
        self.states.iter().all(|s| s.sequence.is_none())
    }
}
