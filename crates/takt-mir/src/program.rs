//! Programm und Tabellen (Referenz 2.4, 7.1, 8.1 bis 8.6, 11.3, 12.9, 13.3,
//! 13.7; plan/mir.md 2.1).

use takt_diag::Span;

use crate::expr::{Expr, TProp};
use crate::fns::{BlockDef, Fn, Native};
use crate::ids::*;
use crate::machine::{Guard, Machine};
use crate::pattern::Address;
use crate::stmt::Block;
use crate::types::{EnumDef, FloatWidth, IntWidth, RecordDef, TypeTable, UnitDef};

/// `output_timing` (2.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum OutputTiming {
    #[default]
    Asap,
    Boundary,
}

/// `tick_tolerance = p pct for n ticks` (7.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickTolerance {
    /// Abweichung in Prozent.
    pub pct: f64,
    /// Zahl aufeinanderfolgender Verletzungen.
    pub ticks: u32,
}

/// `system:`-Block.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Edition (2.5); Teil des Logik-Hashs.
    pub edition: u32,
    /// Basis-Tick T₀ in Nanosekunden.
    pub tick: i64,
    /// Output-Commit.
    pub output_timing: OutputTiming,
    /// `fault_is_fail` (13.5), Default true.
    pub fault_is_fail: bool,
    /// Breite von `float` (4.2); Teil des Logik-Hashs.
    pub float_width: FloatWidth,
    /// `tick_source = hw(...)` (7.1); Bindung, kein Logikanteil.
    pub tick_source: Option<Address>,
    /// Toleranz der Tick-Quelle.
    pub tick_tolerance: Option<TickTolerance>,
    /// `target`.
    pub target: Option<String>,
    /// `tcb_policy = allowlist(…)` (4.5, 9.5): die erlaubten Projekt-Natives;
    /// leer heisst `curated_only`.
    pub tcb_allowlist: Vec<String>,
}

impl Config {
    /// Konfiguration mit Defaults (Edition 1, f64, asap, fault_is_fail).
    pub fn new(edition: u32, tick: i64) -> Self {
        Config {
            edition,
            tick,
            output_timing: OutputTiming::Asap,
            fault_is_fail: true,
            float_width: FloatWidth::F64,
            tick_source: None,
            tick_tolerance: None,
            target: None,
            tcb_allowlist: Vec::new(),
        }
    }
}

/// Metadaten (2.5): ohne Einfluss auf die Logik, nicht im Logik-Hash.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// `label`
    pub label: Option<String>,
    /// `display = U`
    pub display: Option<UnitId>,
    /// `group`
    pub group: Option<String>,
    /// `doc`
    pub doc: Option<String>,
}

/// `input | output`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Direction {
    Input,
    Output,
}

/// Bindung eines Channels (8.1); Metadatum, nicht im Logik-Hash.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Binding {
    /// `@ hw("…")`
    Hw(Address),
    /// `@ sim("…")`
    Sim(Address),
    /// `@ none`
    #[default]
    None,
}

/// Rahmung eines Streams (8.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Framing {
    Raw,
    Lines,
    Cobs,
    LengthPrefixed(IntWidth),
    Fixed(u32),
}

/// `overflow = …` (8.6, 8.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Overflow {
    #[default]
    Fault,
    DropOldest,
    Drop,
}

/// Attribute eines Channels (8.1, 8.6, 8.8, 8.9, 5.10, 3.5, 12.7).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelAttrs {
    /// `safe = v` (Outputs, Pflicht).
    pub safe: Option<Expr>,
    /// `max_age` in Nanosekunden.
    pub max_age: Option<i64>,
    /// `rate` oversampelter Kanaele (8.9), in Hz.
    pub rate: Option<Expr>,
    /// `max_rate` eines Streams, in Hz.
    pub max_rate: Option<Expr>,
    /// `capacity` (Elemente oder Bytes bei Ausgabestroemen).
    pub capacity: Option<u32>,
    /// `framing`
    pub framing: Option<Framing>,
    /// `overflow`
    pub overflow: Option<Overflow>,
    /// `wake = true` (5.10).
    pub wake: bool,
    /// `jitter` als Anforderung, in Nanosekunden (7.5).
    pub jitter: Option<i64>,
    /// `max_slew` (3.5).
    pub max_slew: Option<Expr>,
    /// `debounce` (3.5).
    pub debounce: Option<u32>,
    /// `capacity_bytes` (8.6).
    pub capacity_bytes: Option<u32>,
    /// `expect_len` (8.6).
    pub expect_len: Option<u32>,
    /// `irreversible` (12.7).
    pub irreversible: bool,
}

/// Channel (8.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    /// Richtung.
    pub dir: Direction,
    /// Name.
    pub name: String,
    /// Typ.
    pub ty: TypeId,
    /// Bindung.
    pub binding: Binding,
    /// Attribute.
    pub attrs: ChannelAttrs,
    /// Metadaten.
    pub meta: Meta,
    /// Besitzer eines Outputs (Single-Writer), aus Sema.
    pub owner: Option<MachineId>,
    /// Position.
    pub span: Span,
}

/// Interner Stream (8.6): Warteschlange zwischen Maschinen.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    /// Name.
    pub name: String,
    /// Elementtyp.
    pub elem: TypeId,
    /// Kapazitaet in Elementen.
    pub capacity: u32,
    /// Byte-Ring bei variabler Laenge.
    pub capacity_bytes: Option<u32>,
    /// `expect_len`.
    pub expect_len: Option<u32>,
    /// Ueberlaufverhalten (trifft den Schreiber).
    pub overflow: Overflow,
    /// Schreiber (Single-Writer), aus Sema.
    pub writer: Option<MachineId>,
    /// Leser, aus Sema.
    pub readers: Vec<MachineId>,
    /// Position.
    pub span: Span,
}

/// `param` oder `tunable param` (8.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    /// Name.
    pub name: String,
    /// Typ mit Range.
    pub ty: TypeId,
    /// Default.
    pub default: Expr,
    /// `tunable` (v1.1): Input mit Halte-Semantik.
    pub tunable: bool,
    /// Metadaten.
    pub meta: Meta,
    /// Position.
    pub span: Span,
}

/// `profile` (8.4).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// Name.
    pub name: String,
    /// Belegungen.
    pub assignments: Vec<(ParamId, Expr)>,
    /// Position.
    pub span: Span,
}

/// `command` (8.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Name.
    pub name: String,
    /// `wake = true`.
    pub wake: bool,
    /// Metadaten.
    pub meta: Meta,
    /// Position.
    pub span: Span,
}

/// `node` (12.9, v2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// Name.
    pub name: String,
    /// Adresse.
    pub address: Address,
    /// Knotentick in Nanosekunden (Vielfaches von T₀).
    pub tick: Option<i64>,
    /// Position.
    pub span: Span,
}

/// `property` (13.3, v1.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    /// Name.
    pub name: String,
    /// Formel.
    pub formula: TProp,
    /// `with monitor = true`: Laufzeitmonitor auf Hardware.
    pub monitor: bool,
    /// Position.
    pub span: Span,
}

/// Sweep einer Kampagne (13.7).
#[derive(Clone, Debug, PartialEq)]
pub enum Sweep {
    /// `sweep P = a..b step s`
    Range {
        /// Parameter.
        param: ParamId,
        /// Von.
        from: Expr,
        /// Bis.
        to: Expr,
        /// Schritt.
        step: Expr,
    },
    /// `sweep P = [v1, v2]`
    List {
        /// Parameter.
        param: ParamId,
        /// Werte.
        values: Vec<Expr>,
    },
}

/// `stop_on` einer Kampagne.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum StopOn {
    Fail,
    #[default]
    Never,
}

/// `campaign` (13.7, v1.1); Eingabe der CLI, von der Runtime ignoriert.
#[derive(Clone, Debug, PartialEq)]
pub struct Campaign {
    /// Name.
    pub name: String,
    /// `program "datei"`.
    pub program: Option<String>,
    /// `profile P`.
    pub profile: Option<ProfileId>,
    /// Sweeps.
    pub sweeps: Vec<Sweep>,
    /// `repeat n`.
    pub repeat: u32,
    /// `stop_on`.
    pub stop_on: StopOn,
    /// Position.
    pub span: Span,
}

/// `trigger` (7.5, v1.2): Regel auf dem I/O-Knoten.
#[derive(Clone, Debug, PartialEq)]
pub struct Trigger {
    /// Name.
    pub name: String,
    /// Knoten; `None` ist der Hauptknoten.
    pub node: Option<NodeId>,
    /// `when`-Guard ueber knotenlokale Streams.
    pub guard: Guard,
    /// Zeitpunkt der geplanten Ausgabe (`event.t + d`).
    pub time: Expr,
    /// Output-Zuweisungen.
    pub then: Block,
    /// `bound` in Nanosekunden (`guard = bound`).
    pub bound: i64,
    /// Position.
    pub span: Span,
}

/// Das Programm: Konfiguration und alle Tabellen. Namen sind Indizes in
/// diese Tabellen; die MIR ist editionsfrei bis auf `config.edition`.
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    /// `system:`.
    pub config: Config,
    /// Internierte Typen.
    pub types: TypeTable,
    /// Einheiten.
    pub units: Vec<UnitDef>,
    /// Enums und Summentypen.
    pub enums: Vec<EnumDef>,
    /// Records.
    pub records: Vec<RecordDef>,
    /// Funktionen (monomorphisiert).
    pub fns: Vec<Fn>,
    /// Native Funktionen und Jobs.
    pub natives: Vec<Native>,
    /// Bloecke.
    pub blocks: Vec<BlockDef>,
    /// Maschinen, Vorlagen, Instanzen, Szenarien.
    pub machines: Vec<Machine>,
    /// Channels.
    pub channels: Vec<Channel>,
    /// Interne Streams.
    pub streams: Vec<Stream>,
    /// Parameter.
    pub params: Vec<Param>,
    /// Profile.
    pub profiles: Vec<Profile>,
    /// Commands.
    pub commands: Vec<Command>,
    /// Knoten (v2).
    pub nodes: Vec<Node>,
    /// Eigenschaften (v1.1).
    pub properties: Vec<Property>,
    /// Kampagnen (v1.1).
    pub campaigns: Vec<Campaign>,
    /// Trigger (v1.2).
    pub triggers: Vec<Trigger>,
}

impl Program {
    /// Leeres Programm.
    pub fn new(config: Config) -> Self {
        Program {
            config,
            types: TypeTable::default(),
            units: Vec::new(),
            enums: Vec::new(),
            records: Vec::new(),
            fns: Vec::new(),
            natives: Vec::new(),
            blocks: Vec::new(),
            machines: Vec::new(),
            channels: Vec::new(),
            streams: Vec::new(),
            params: Vec::new(),
            profiles: Vec::new(),
            commands: Vec::new(),
            nodes: Vec::new(),
            properties: Vec::new(),
            campaigns: Vec::new(),
            triggers: Vec::new(),
        }
    }

    /// Fuegt eine Maschine ein.
    pub fn add_machine(&mut self, m: Machine) -> MachineId {
        self.machines.push(m);
        MachineId(self.machines.len() as u32 - 1)
    }

    /// Fuegt einen Channel ein.
    pub fn add_channel(&mut self, c: Channel) -> ChannelId {
        self.channels.push(c);
        ChannelId(self.channels.len() as u32 - 1)
    }

    /// Kern-MIR: keine Sequenz-Oberflaeche in irgendeiner Maschine.
    pub fn is_core(&self) -> bool {
        self.machines.iter().all(Machine::is_core)
    }
}
