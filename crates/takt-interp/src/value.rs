//! Werte, Input-Abtastungen und Faults (Referenz 9.1, 3.5, 5.3).
//!
//! Werte sind dynamisch, ihre Semantik exakt: Fliesskomma in Programmbreite
//! (`F32`/`F64`), Ganzzahlen mit der Breite des statischen Typs, Dauern als
//! Nanosekunden. `NaN` und `Inf` kommen nie vor (4.1).

use std::cmp::Ordering;

use takt_diag::Span;
use takt_mir::machine::{FaultKind, Target};
use takt_mir::program::Program;
use takt_mir::types::{FloatWidth, IntWidth, Type};
use takt_mir::{BlockId, TypeId};

/// Qualitaet eines Inputs (3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Quality {
    Good,
    Suspect,
    Stale,
    Bad,
}

/// Grund einer Degradierung (`x.reason`, 3.5, 12.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Reason {
    Stale,
    OutOfRange,
    Implausible,
    Driver,
    Node,
}

/// Abtastung eines Inputs in einem Tick (Wert, Qualitaet, Alter).
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Wert; fehlt bei `Bad` ohne letzten guten Wert.
    pub value: Option<Value>,
    /// Qualitaet.
    pub quality: Quality,
    /// Alter in Nanosekunden.
    pub age: i64,
    /// Grund, wenn nicht `Good`.
    pub reason: Option<Reason>,
}

impl Sample {
    /// `x.valid`: `Good` oder `Suspect` mit Wert (3.5).
    pub fn valid(&self) -> bool {
        matches!(self.quality, Quality::Good | Quality::Suspect) && self.value.is_some()
    }

    /// Ungueltige Abtastung mit Grund.
    pub fn bad(reason: Reason) -> Self {
        Sample { value: None, quality: Quality::Bad, age: 0, reason: Some(reason) }
    }

    /// Gute Abtastung mit Alter 0.
    pub fn good(value: Value) -> Self {
        Sample { value: Some(value), quality: Quality::Good, age: 0, reason: None }
    }
}

/// Der Treiberrand (12.6) prueft Ranges und `max_slew` ueber `takt-hal`;
/// dafuer braucht er von einem Wert nur Ordnung und Differenz.
///
/// Ein Wert, der keine Zahl ist, traegt weder Range noch Steigung: Beide
/// Methoden liefern `None`, und die Pruefungen lassen ihn durch.
impl takt_hal::Scalar for Value {
    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F32(f) => Some(f64::from(*f)),
            Value::F64(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::UInt(u) => Some(*u as f64),
            Value::Duration(d) => Some(*d as f64),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::UInt(u) => i64::try_from(*u).ok(),
            Value::Duration(d) => Some(*d),
            _ => None,
        }
    }
}

/// Zustand einer Blockinstanz (5.7): Parameter, dann Zustandsvariablen, wie
/// der `VarId`-Raum der Blockmethoden (plan/mir.md, Abschnitt 7).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockState {
    /// Block.
    pub block: BlockId,
    /// Parameter und Zustandsvariablen.
    pub vars: Vec<Value>,
    /// `step` in dieser Aktivierung schon ausgefuehrt (5.7).
    pub stepped: bool,
}

/// Ein Wert.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Value {
    Bool(bool),
    /// Vorzeichenbehaftete Breiten `i8` bis `i64`; `int` ist `i64`.
    Int(i64),
    /// Breiten `u8` bis `u64`.
    UInt(u64),
    F32(f32),
    F64(f64),
    /// Nanosekunden.
    Duration(i64),
    Enum {
        variant: u32,
        fields: Vec<Value>,
    },
    Record(Vec<Value>),
    Array(Vec<Value>),
    /// `bytes<N>` mit Laenge `0..N`.
    Bytes(Vec<u8>),
    /// `vec<T, N>` mit Laenge `0..N`.
    Vec(Vec<Value>),
    /// `str<N>`.
    Str(String),
    /// `line<N>` mit `truncated`.
    Line {
        text: String,
        truncated: bool,
    },
    Optional(Option<Box<Value>>),
    /// `T!E`
    Result(Result<Box<Value>, Box<Value>>),
    /// `mat<R, C>`, zeilenweise in Programmbreite.
    Mat {
        rows: u32,
        cols: u32,
        data: Vec<Value>,
    },
    /// `table<A, B>`: Stuetzstellen.
    Table(Vec<(Value, Value)>),
    /// `map<K, V, N>` (v1.1).
    Map(Vec<(Value, Value)>),
    /// `samples<T, N>` (M2).
    Samples(Vec<Value>),
    Block(Box<BlockState>),
    /// Job- oder Trigger-Handle (v1.1, v1.2), noch ohne Inhalt.
    Handle,
}

impl Value {
    /// `default` eines Typs (3.7): 0, `false`, erste Variante, Records feldweise,
    /// leere Sammlungen, `none`.
    pub fn default_for(ty: TypeId, p: &Program) -> Value {
        match p.types.get(ty) {
            Type::Bool => Value::Bool(false),
            Type::Int { width, .. } => {
                if width.signed() {
                    Value::Int(0)
                } else {
                    Value::UInt(0)
                }
            }
            Type::Float { width: FloatWidth::F32, .. } => Value::F32(0.0),
            Type::Float { width: FloatWidth::F64, .. } => Value::F64(0.0),
            Type::Duration { .. } => Value::Duration(0),
            Type::Enum(id) => {
                let fields = p.enums[id.index()]
                    .variants
                    .first()
                    .map(|v| v.fields.iter().map(|f| Value::default_for(f.ty, p)).collect())
                    .unwrap_or_default();
                Value::Enum { variant: 0, fields }
            }
            Type::Record(id) => {
                Value::Record(p.records[id.index()].fields.iter().map(|f| Value::default_for(f.ty, p)).collect())
            }
            Type::Array { elem, len } => Value::Array((0..*len).map(|_| Value::default_for(*elem, p)).collect()),
            Type::Bytes { .. } => Value::Bytes(Vec::new()),
            Type::Vec { .. } => Value::Vec(Vec::new()),
            Type::Str { .. } => Value::Str(String::new()),
            Type::Line { .. } => Value::Line { text: String::new(), truncated: false },
            Type::Samples { .. } => Value::Samples(Vec::new()),
            Type::Table { .. } => Value::Table(Vec::new()),
            Type::Mat { rows, cols, .. } => {
                let zero = match p.config.float_width {
                    FloatWidth::F32 => Value::F32(0.0),
                    FloatWidth::F64 => Value::F64(0.0),
                };
                Value::Mat { rows: *rows, cols: *cols, data: vec![zero; mat_len(*rows, *cols)] }
            }
            Type::Map { .. } => Value::Map(Vec::new()),
            Type::Optional(_) => Value::Optional(None),
            Type::Result { ok, .. } => Value::Result(Ok(Box::new(Value::default_for(*ok, p)))),
            Type::Stream(_) | Type::Capture { .. } => Value::Samples(Vec::new()),
            Type::Handle(_) => Value::Handle,
        }
    }

    /// Wahrheitswert.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Ganzzahl als `i128` (beide Breitenklassen).
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(i128::from(*i)),
            Value::UInt(u) => Some(i128::from(*u)),
            _ => None,
        }
    }

    /// Fliesskommawert als `f64` (verlustfrei fuer `F32`).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F32(f) => Some(f64::from(*f)),
            Value::F64(f) => Some(*f),
            _ => None,
        }
    }

    /// Dauer in Nanosekunden.
    pub fn as_duration(&self) -> Option<i64> {
        match self {
            Value::Duration(d) => Some(*d),
            _ => None,
        }
    }

    /// Fliesskommawert in einer Breite.
    pub fn float(width: FloatWidth, x: f64) -> Value {
        match width {
            FloatWidth::F32 => Value::F32(x as f32),
            FloatWidth::F64 => Value::F64(x),
        }
    }

    /// Ganzzahl in eine Fliesskommabreite, mit genau einer Rundung (4.1).
    /// Der Umweg ueber f64 rundete zweimal: `9007199791611905 as f64 as f32`
    /// ergibt 9007199254740992, `as f32` dagegen 9007200328482816. Ein Ziel
    /// emittiert `sitofp`, also eine Rundung.
    pub fn float_from_int(width: FloatWidth, x: i128) -> Value {
        match width {
            FloatWidth::F32 => Value::F32(x as f32),
            FloatWidth::F64 => Value::F64(x as f64),
        }
    }

    /// Ganzzahl in einer Breite, ohne Pruefung des Wertebereichs.
    pub fn int(width: IntWidth, x: i128) -> Value {
        if width.signed() { Value::Int(x as i64) } else { Value::UInt(x as u64) }
    }

    /// Totale Ordnung fuer Zahlen, Dauern, Bytes und Strings (bytewise, 3.9).
    pub fn compare(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Int(_) | Value::UInt(_), Value::Int(_) | Value::UInt(_)) => {
                self.as_int()?.partial_cmp(&other.as_int()?)
            }
            (Value::F32(a), Value::F32(b)) => a.partial_cmp(b),
            (Value::F64(a), Value::F64(b)) => a.partial_cmp(b),
            (Value::Duration(a), Value::Duration(b)) => Some(a.cmp(b)),
            (Value::Str(a), Value::Str(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
            (Value::Line { text: a, .. }, Value::Line { text: b, .. }) => Some(a.as_bytes().cmp(b.as_bytes())),
            (Value::Bytes(a), Value::Bytes(b)) => Some(a.cmp(b)),
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }

    /// Kurzname der Wertart fuer Meldungen.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::UInt(_) => "uint",
            Value::F32(_) => "f32",
            Value::F64(_) => "f64",
            Value::Duration(_) => "Duration",
            Value::Enum { .. } => "enum",
            Value::Record(_) => "record",
            Value::Array(_) => "array",
            Value::Bytes(_) => "bytes",
            Value::Vec(_) => "vec",
            Value::Str(_) => "str",
            Value::Line { .. } => "line",
            Value::Optional(_) => "optional",
            Value::Result(_) => "result",
            Value::Mat { .. } => "mat",
            Value::Table(_) => "table",
            Value::Map(_) => "map",
            Value::Samples(_) => "samples",
            Value::Block(_) => "block",
            Value::Handle => "handle",
        }
    }
}

/// Fault (5.3): Art, Nachricht, Quellposition, Tick.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    /// Art.
    pub kind: FaultKind,
    /// Nachricht, in einen festen Puffer formatiert.
    pub message: String,
    /// Position der Anweisung oder des Ausdrucks.
    pub span: Span,
    /// Tick des Auftretens.
    pub tick: u64,
    /// Eigenes Ziel eines `check … -> X` (5.3); sonst das Fault-Ziel des Zustands.
    pub target: Option<Target>,
}

impl Fault {
    /// Neuer Fault ohne eigenes Ziel.
    pub fn new(kind: FaultKind, message: impl Into<String>, span: Span, tick: u64) -> Self {
        Fault { kind, message: message.into(), span, tick, target: None }
    }
}

/// Abbruch der Auswertung: ein Fault der Semantik oder ein Fehler des
/// Interpreters selbst (unerlaubte MIR, die der Verifier nicht abfing).
#[derive(Clone, Debug, PartialEq)]
pub enum Trap {
    /// Fault nach 4.1/5.3; wird vom Fault-Wald behandelt.
    Fault(Fault),
    /// Interner Fehler; beendet die Simulation mit Diagnose, nie mit Panik.
    Bug(String),
}

impl From<Fault> for Trap {
    fn from(f: Fault) -> Self {
        Trap::Fault(f)
    }
}

/// Ergebnis der Auswertung.
pub type EvalResult<T> = Result<T, Trap>;

/// Interner Fehler.
pub fn bug<T>(msg: impl Into<String>) -> EvalResult<T> {
    Err(Trap::Bug(msg.into()))
}

/// Elementzahl einer Matrix. `rows * cols` als `u32` wickelte um und lieferte
/// eine falsch dimensionierte Matrix, deren Indizierung dann ausserhalb der
/// Daten lag; das Speicherbudget begrenzt die Groesse erst ab M3 (3.11).
pub fn mat_len(rows: u32, cols: u32) -> usize {
    usize::try_from(u64::from(rows) * u64::from(cols)).unwrap_or(usize::MAX)
}
