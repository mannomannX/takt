//! Ausdruecke (Referenz 4, 9.2): typisiert, total, seiteneffektfrei.
//!
//! Jeder Ausdruck traegt seinen Typ; implizite Pruefungen (Validitaet 3.5,
//! Range 3.4, Arithmetik 4.1, `MissingValue` 3.8) sind explizite `Checked`-
//! Knoten, die M3 einsetzt. Der Interpreter fuehrt aus, was da steht.

use takt_diag::Span;

use crate::ids::*;
use crate::pattern::Pattern;
use crate::types::{IntWidth, Range};

/// Gewaehlte Darstellung eines `int` (3.4, Lemma 3.4). Die Annotation
/// aendert die Semantik nicht — der Interpreter liest sie nicht —, sondern
/// sagt dem Codegen, in welcher Breite er rechnen darf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repr {
    /// 32 Bit: das Intervall ist bewiesen und passt.
    I32,
    /// 64 Bit: die Semantik von `int` (3.2).
    I64,
}

/// Ein Ausdruck mit Typ und, wenn bewiesen, Intervall.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    /// Inhalt.
    pub kind: ExprKind,
    /// Typ.
    pub ty: TypeId,
    /// Bewiesenes Intervall (3.4).
    pub range: Option<Range>,
    /// Gewaehlte Darstellung (3.4); erst die Analyse aus M3 setzt sie.
    pub repr: Option<Repr>,
    /// Position.
    pub span: Span,
}

impl Expr {
    /// Ausdruck ohne Intervall und ohne Darstellung.
    pub fn new(kind: ExprKind, ty: TypeId, span: Span) -> Self {
        Expr { kind, ty, range: None, repr: None, span }
    }

    /// Die unmittelbaren Teilausdruecke, veraenderbar.
    /// Gleichheit ohne Position und ohne Annotationen.
    ///
    /// `PartialEq` vergleicht alle Felder, also auch `span`, `range` und
    /// `repr` — zwei gleich geschriebene Bedingungen an verschiedenen
    /// Stellen sind damit nie gleich. Wer fragt „steht hier zweimal
    /// dasselbe?" (etwa der Polaritaets-Lint, 5.6) braucht diese Form.
    pub fn same_as(&self, other: &Expr) -> bool {
        if self.ty != other.ty || std::mem::discriminant(&self.kind) != std::mem::discriminant(&other.kind) {
            return false;
        }
        // Der Vergleich laeuft ueber die normierten Kopien: Position und
        // Annotationen weg, Kinder ebenso normiert. Das ist eine Kopie je
        // Aufruf, aber der Lint laeuft einmal ueber die Bedingungen eines
        // Blocks — die Klarheit ist den Preis wert.
        self.normalised() == other.normalised()
    }

    /// Der Ausdruck ohne Position und Annotationen, rekursiv.
    fn normalised(&self) -> Expr {
        let mut out = self.clone();
        out.span = Span::default();
        out.range = None;
        out.repr = None;
        for c in out.children_mut() {
            *c = c.normalised();
        }
        out
    }

    /// Die unmittelbaren Teilausdruecke, lesend.
    ///
    /// Die Entsprechung zu [`Expr::children_mut`]. Beide Listen muessen
    /// dieselben Kinder nennen — sonst sieht eine Analyse einen Teilbaum,
    /// den eine andere nicht kennt.
    pub fn children(&self) -> Vec<&Expr> {
        match &self.kind {
            ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter().collect(),
            ExprKind::Array(items) => items.iter().collect(),
            ExprKind::Tuple(a, b) => vec![a, b],
            ExprKind::BlockInit { args, .. }
            | ExprKind::Call { args, .. }
            | ExprKind::NativeCall { args, .. }
            | ExprKind::MatOp { args, .. }
            | ExprKind::Intrinsic { args, .. } => args.iter().collect(),
            ExprKind::Field { base, .. } => vec![base],
            ExprKind::Index { base, index } => vec![base, index],
            ExprKind::Index2 { base, row, col } => vec![base, row, col],
            ExprKind::Slice { base, from, to } => vec![base, from, to],
            ExprKind::Accessor { base, args, .. } => {
                let mut v: Vec<&Expr> = vec![base];
                v.extend(args.iter());
                v
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Cast { expr, .. }
            | ExprKind::Convert { expr, .. }
            | ExprKind::Checked { expr, .. } => vec![expr],
            ExprKind::Lift(x) | ExprKind::Ok(x) | ExprKind::Err(x) => vec![x],
            ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
            ExprKind::Cond { cond, then, otherwise } => vec![cond, then, otherwise],
            ExprKind::Matches { subject, .. } => vec![subject],
            ExprKind::Decode { bytes, .. } => vec![bytes],
            _ => Vec::new(),
        }
    }

    /// Die unmittelbaren Teilausdruecke, veraenderlich.
    pub fn children_mut(&mut self) -> Vec<&mut Expr> {
        match &mut self.kind {
            ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter_mut().collect(),
            ExprKind::Array(items) => items.iter_mut().collect(),
            ExprKind::Tuple(a, b) => vec![a, b],
            ExprKind::BlockInit { args, .. }
            | ExprKind::Call { args, .. }
            | ExprKind::NativeCall { args, .. }
            | ExprKind::MatOp { args, .. }
            | ExprKind::Intrinsic { args, .. } => args.iter_mut().collect(),
            ExprKind::Field { base, .. } => vec![base],
            ExprKind::Index { base, index } => vec![base, index],
            ExprKind::Index2 { base, row, col } => vec![base, row, col],
            ExprKind::Slice { base, from, to } => vec![base, from, to],
            ExprKind::Accessor { base, args, .. } => {
                let mut v: Vec<&mut Expr> = vec![base];
                v.extend(args.iter_mut());
                v
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Cast { expr, .. }
            | ExprKind::Convert { expr, .. }
            | ExprKind::Checked { expr, .. } => vec![expr],
            ExprKind::Lift(e) | ExprKind::Ok(e) | ExprKind::Err(e) => vec![e],
            ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
            ExprKind::Cond { cond, then, otherwise } => vec![cond, then, otherwise],
            ExprKind::Matches { subject, .. } => vec![subject],
            ExprKind::Decode { bytes, .. } => vec![bytes],
            _ => Vec::new(),
        }
    }
}

/// Bezug auf eine Maschine oder Instanz; `index` waehlt in einem
/// Instanz-Array (5.8) das Element.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineRef {
    /// Maschine (bei Arrays: das erste Element).
    pub machine: MachineId,
    /// Index im Instanz-Array.
    pub index: Option<Box<Expr>>,
}

/// Bezug auf einen Stream (8.6, 7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamRef {
    /// Channel mit Stream-Typ.
    Channel(ChannelId),
    /// Interner Stream.
    Internal(StreamId),
    /// `t.fired` eines Triggers (v1.2).
    Fired(TriggerId),
    /// Stream-typisierter Parameter oder Handle in einer Variablen.
    Var(VarId),
}

/// Eingebaute Groessen (3.3, 5.3, 7.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    /// `now`: Dauer seit Start.
    Now,
    /// `tick`: T₀.
    Tick,
    /// `time_in_state`.
    TimeInState,
    /// `last_fault` (Art, Nachricht, Position, Tick).
    LastFault,
    /// `event` im `then`-Teil eines Triggers.
    Event,
}

/// Reine Zugriffe ueber reservierte Membernamen (2.5), je Typ erlaubt.
/// Mutierende Methoden stehen in `stmt::Method`, Konversionen in
/// `ConvertKind`, Matrixoperationen in `MatOp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Accessor {
    Valid,
    Suspect,
    Stale,
    Age,
    Reason,
    /// `x.or(d)`
    Or,
    Ok,
    Err,
    /// `.t` eines Stream-Elements.
    T,
    Seq,
    Text,
    Data,
    Len,
    Count,
    Dropped,
    Malformed,
    Overflowed,
    Free,
    Jitter,
    TimeWarped,
    Done,
    Result,
    /// `x.bit(i)`
    Bit,
    /// `x.bits(hi, lo)`
    Bits,
    /// `x.with_bit(i, b)`
    WithBit,
    /// `x.wrap_u16()` und Geschwister, modulo 2^n (3.10).
    Wrap(IntWidth),
    Min,
    Max,
    Mean,
    Rms,
    Last,
    /// `f.encode()` eines Records mit `layout`.
    Encode,
    /// `v.get(i)`, `m.get(k)` → `T?`.
    Get,
    StartsWith,
    Contains,
    Armed,
    Pre,
    Post,
    Samples,
    Rate,
    Remaining,
    Truncated,
    /// `s.peek() -> E?`: das naechste Element, untersucht, nicht konsumiert (8.6, FB-15).
    Peek,
    /// `o.sent -> bytes<CAP>?`: der im letzten Tick gesendete Ausschnitt eines
    /// Ausgabestroms, mit Unit-Delay (8.8, FB-132).
    Sent,
}

impl Accessor {
    /// Der reservierte Membername (2.5), wie er im Quelltext steht.
    pub fn name(self) -> String {
        match self {
            Accessor::Valid => "valid",
            Accessor::Suspect => "suspect",
            Accessor::Stale => "stale",
            Accessor::Age => "age",
            Accessor::Reason => "reason",
            Accessor::Or => "or",
            Accessor::Ok => "ok",
            Accessor::Err => "err",
            Accessor::T => "t",
            Accessor::Seq => "seq",
            Accessor::Text => "text",
            Accessor::Data => "data",
            Accessor::Len => "len",
            Accessor::Count => "count",
            Accessor::Dropped => "dropped",
            Accessor::Malformed => "malformed",
            Accessor::Overflowed => "overflowed",
            Accessor::Free => "free",
            Accessor::Jitter => "jitter",
            Accessor::TimeWarped => "time_warped",
            Accessor::Done => "done",
            Accessor::Result => "result",
            Accessor::Bit => "bit",
            Accessor::Bits => "bits",
            Accessor::WithBit => "with_bit",
            Accessor::Wrap(w) => return format!("wrap_{}", format!("{w:?}").to_lowercase()),
            Accessor::Min => "min",
            Accessor::Max => "max",
            Accessor::Mean => "mean",
            Accessor::Rms => "rms",
            Accessor::Last => "last",
            Accessor::Encode => "encode",
            Accessor::Get => "get",
            Accessor::StartsWith => "starts_with",
            Accessor::Contains => "contains",
            Accessor::Armed => "armed",
            Accessor::Pre => "pre",
            Accessor::Post => "post",
            Accessor::Samples => "samples",
            Accessor::Rate => "rate",
            Accessor::Remaining => "remaining",
            Accessor::Truncated => "truncated",
            Accessor::Peek => "peek",
            Accessor::Sent => "sent",
        }
        .to_string()
    }

    /// Alle Zugriffe ohne Nutzlast (fuer Paritaetstests gegen 2.5).
    pub const ALL: [Accessor; 41] = [
        Accessor::Valid,
        Accessor::Suspect,
        Accessor::Stale,
        Accessor::Age,
        Accessor::Reason,
        Accessor::Or,
        Accessor::Ok,
        Accessor::Err,
        Accessor::T,
        Accessor::Seq,
        Accessor::Text,
        Accessor::Data,
        Accessor::Len,
        Accessor::Count,
        Accessor::Dropped,
        Accessor::Malformed,
        Accessor::Overflowed,
        Accessor::Free,
        Accessor::Jitter,
        Accessor::TimeWarped,
        Accessor::Done,
        Accessor::Result,
        Accessor::Bit,
        Accessor::Bits,
        Accessor::WithBit,
        Accessor::Min,
        Accessor::Max,
        Accessor::Mean,
        Accessor::Rms,
        Accessor::Last,
        Accessor::Encode,
        Accessor::Get,
        Accessor::StartsWith,
        Accessor::Contains,
        Accessor::Armed,
        Accessor::Pre,
        Accessor::Post,
        Accessor::Samples,
        Accessor::Rate,
        Accessor::Remaining,
        Accessor::Truncated,
    ];
}

/// Einstellige Operatoren.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

/// Zweistellige Operatoren (2.3), klassenweise total (4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum BinaryOp {
    Or,
    And,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitOr,
    BitXor,
    BitAnd,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// Konversion zwischen Einheiten (3.2, 3.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConvertKind {
    /// `x.to(U)`: gleiche Dimension; bei Integern nur ganzzahliger Faktor.
    To,
    /// `x.to_float(U)` fuer Integer mit Einheit.
    ToFloat,
    /// `d.as(U)`: Dauer in `float[U]`.
    As,
}

/// Matrixoperationen (3.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum MatOp {
    Transpose,
    Inv,
    Det,
    Solve,
    Cholesky,
}

/// `matches` oder `has` (8.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum MatchKind {
    Matches,
    Has,
}

/// Art einer eingefuegten Pruefung (M3, Warnung 4 in Referenz 10).
#[derive(Clone, Debug, PartialEq)]
pub enum CheckedKind {
    /// Divisor null → `ArithmeticFault(DivZero)`.
    DivZero,
    /// Ganzzahlueberlauf → `ArithmeticFault(Overflow)`.
    Overflow,
    /// Nicht endliches Ergebnis → `ArithmeticFault(NonFinite)`.
    NonFinite,
    /// Definitionsbereich verletzt → `ArithmeticFault(Domain)`.
    Domain,
    /// Index ausserhalb `0..len-1` → `RangeFault`.
    Index {
        /// Laenge.
        len: u32,
    },
    /// Wert ausserhalb der Range → `RangeFault`.
    Range(Range),
    /// `as`-Konversion mit Verlust → `RangeFault`.
    Convert,
    /// Shift-Betrag ausserhalb `0..width-1` → `RangeFault`.
    Shift,
    /// Channel nicht gueltig → `SensorFault` (3.5).
    Valid,
    /// `T?`/`T!E` ohne Wert → `MissingValue` (3.8).
    Missing,
}

/// Inhalt eines Ausdrucks (2.6 in plan/mir.md).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum ExprKind {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Nanosekunden.
    Duration(i64),
    Str(String),
    None,
    Default,
    /// Variante mit Feldern in Deklarationsreihenfolge.
    Variant {
        enum_id: EnumId,
        variant: u32,
        fields: Vec<Expr>,
    },
    /// Record-Konstruktion, Felder in Deklarationsreihenfolge.
    Record {
        record: RecordId,
        fields: Vec<Expr>,
    },
    Array(Vec<Expr>),
    /// Stuetzstelle `(x, y)` einer Tabelle.
    Tuple(Box<Expr>, Box<Expr>),
    /// Blockinstanz `lowpass(tau = 50 ms)` oder Array `[N] lowpass(…)`, nur als Initialwert.
    BlockInit {
        block: BlockId,
        args: Vec<Expr>,
        count: Option<u32>,
    },
    Var(VarId),
    /// Parameter oder Tunable (8.4).
    Param(ParamId),
    /// Command als Puls-Input (8.5).
    Command(CommandId),
    /// Input-Channel; `dominated` heisst: statisch unter `.valid` (3.5).
    Input {
        channel: ChannelId,
        dominated: bool,
    },
    /// Latch-Wert eines eigenen Outputs (`latch(o)`, 9.2).
    Output(ChannelId),
    /// `pub var` einer anderen Maschine (Unit-Delay, 7.2 bei `follows` frisch).
    Published {
        machine: MachineRef,
        var: VarId,
    },
    /// `m.state`
    StateOf(MachineRef),
    /// `m.done` eines Signals.
    Signal {
        machine: MachineRef,
        signal: SignalId,
    },
    Builtin(Builtin),
    /// `t.armed` (7.5, v1.2): das Flag im Layout der armierenden Maschine.
    Armed(TriggerId),
    /// Lesen eines Registerports (12.10, v1.2): sofort, in
    /// Programmreihenfolge, nicht ueber das Prozessabbild.
    PortRead(PortId),
    Field {
        base: Box<Expr>,
        field: u32,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Index2 {
        base: Box<Expr>,
        row: Box<Expr>,
        col: Box<Expr>,
    },
    Slice {
        base: Box<Expr>,
        from: Box<Expr>,
        to: Box<Expr>,
    },
    Accessor {
        base: Box<Expr>,
        accessor: Accessor,
        args: Vec<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `a if c else b`
    Cond {
        cond: Box<Expr>,
        then: Box<Expr>,
        otherwise: Box<Expr>,
    },
    /// `x as T` (3.10), range-geprueft ueber `Checked`.
    Cast {
        expr: Box<Expr>,
        to: TypeId,
    },
    Convert {
        expr: Box<Expr>,
        kind: ConvertKind,
        unit: UnitId,
    },
    /// Ein String mit Platzhaltern (8.8, 2.5): `send tx, "UPDATE {n}"`.
    /// Ein Literal ohne Platzhalter bleibt `Str`; erst die Interpolation
    /// braucht die Teilausdruecke.
    Format(crate::pattern::Format),
    /// `v.done` / `v.result` eines Job-Handles (4.5): ein Input, gelesen
    /// ueber den Slot des Handles (`Layout::job_slots`).
    JobState {
        handle: VarId,
        field: JobField,
    },
    /// Ein interner Stream als Wert (8.6): Subjekt eines Guards und Traeger
    /// der Zaehler `.count`, `.dropped`, `.overflowed`, `.malformed`. Ein
    /// Stream-Channel steht als `Input` da; hier fehlt die `ChannelId`.
    Stream(StreamId),
    /// `x matches P as m` / `x has P`; die Bindung ist eine gehobene Variable.
    Matches {
        subject: Box<Expr>,
        kind: MatchKind,
        pattern: Pattern,
        binding: Option<VarId>,
    },
    Call {
        callee: FnId,
        args: Vec<Expr>,
    },
    NativeCall {
        native: NativeId,
        args: Vec<Expr>,
    },
    MatOp {
        op: MatOp,
        args: Vec<Expr>,
    },
    /// `R.decode(b)` → `R?`
    Decode {
        record: RecordId,
        bytes: Box<Expr>,
    },
    /// Eingefuegte Pruefung um einen Teilausdruck.
    Checked {
        expr: Box<Expr>,
        kind: CheckedKind,
    },
    /// `T` nach `T?` gehoben (3.8).
    Lift(Box<Expr>),
    /// `OK(v)` eines `T!E` (3.8).
    Ok(Box<Expr>),
    /// `ERR(e)` eines `T!E` (3.8).
    Err(Box<Expr>),
    /// Primitive mit eigener Fault-Semantik und Kostenklasse (4.1, 3.9, 3.10).
    Intrinsic {
        op: Intrinsic,
        args: Vec<Expr>,
    },
}

/// Eingebaute Primitive: total oder mit definiertem Fault (`Domain`, `RangeFault`,
/// `NonFinite`); anders als Natives ohne Kostenvertrag, ihre Kosten zaehlt das
/// Kostenmodell nach Klasse. Polymorph ueber Breiten und Einheiten
/// (`sqrt`: `U^2 → U`; `min`, `max`, `abs`: Einheit bleibt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Intrinsic {
    Abs,
    Min,
    Max,
    Sqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Log,
    Pow,
    /// Korrekt gerundetes `a * b + c` (4.2).
    Fma,
    /// `float → int` mit Range-Pruefung (4.1).
    Round,
    Floor,
    Ceil,
    Rotl,
    Rotr,
    WrappingAdd,
    WrappingSub,
    WrappingMul,
    SaturatingAdd,
    SaturatingSub,
    /// Stueckweise lineare Interpolation in einer Tabelle (3.9).
    Interp,
}

impl Intrinsic {
    /// Name im Quelltext.
    pub fn name(self) -> &'static str {
        match self {
            Intrinsic::Abs => "abs",
            Intrinsic::Min => "min",
            Intrinsic::Max => "max",
            Intrinsic::Sqrt => "sqrt",
            Intrinsic::Sin => "sin",
            Intrinsic::Cos => "cos",
            Intrinsic::Tan => "tan",
            Intrinsic::Asin => "asin",
            Intrinsic::Acos => "acos",
            Intrinsic::Atan => "atan",
            Intrinsic::Atan2 => "atan2",
            Intrinsic::Exp => "exp",
            Intrinsic::Log => "log",
            Intrinsic::Pow => "pow",
            Intrinsic::Fma => "fma",
            Intrinsic::Round => "round",
            Intrinsic::Floor => "floor",
            Intrinsic::Ceil => "ceil",
            Intrinsic::Rotl => "rotl",
            Intrinsic::Rotr => "rotr",
            Intrinsic::WrappingAdd => "wrapping_add",
            Intrinsic::WrappingSub => "wrapping_sub",
            Intrinsic::WrappingMul => "wrapping_mul",
            Intrinsic::SaturatingAdd => "saturating_add",
            Intrinsic::SaturatingSub => "saturating_sub",
            Intrinsic::Interp => "interp",
        }
    }

    /// Alle Primitive.
    pub const ALL: [Intrinsic; 26] = [
        Intrinsic::Abs,
        Intrinsic::Min,
        Intrinsic::Max,
        Intrinsic::Sqrt,
        Intrinsic::Sin,
        Intrinsic::Cos,
        Intrinsic::Tan,
        Intrinsic::Asin,
        Intrinsic::Acos,
        Intrinsic::Atan,
        Intrinsic::Atan2,
        Intrinsic::Exp,
        Intrinsic::Log,
        Intrinsic::Pow,
        Intrinsic::Fma,
        Intrinsic::Round,
        Intrinsic::Floor,
        Intrinsic::Ceil,
        Intrinsic::Rotl,
        Intrinsic::Rotr,
        Intrinsic::WrappingAdd,
        Intrinsic::WrappingSub,
        Intrinsic::WrappingMul,
        Intrinsic::SaturatingAdd,
        Intrinsic::SaturatingSub,
        Intrinsic::Interp,
    ];

    /// Primitive zu einem Namen.
    pub fn from_name(name: &str) -> Option<Intrinsic> {
        Intrinsic::ALL.into_iter().find(|i| i.name() == name)
    }
}

/// Temporaloperator einer Eigenschaft (13.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TemporalOp {
    Always,
    Never,
    /// `eventually[d]`
    Eventually,
    /// `stable[d]`
    Stable,
    /// `once[d]`
    Once,
}

/// Eigenschaft (13.3, Grammatik `tprop`): beschraenkte Temporallogik ueber
/// Tick-Rand-Snapshots; ein Atom ist ein Bool-Ausdruck.
#[derive(Clone, Debug, PartialEq)]
pub enum TProp {
    /// `op[d](inner)`; `window` in Nanosekunden, nur bei den beschraenkten Operatoren.
    Temporal {
        /// Operator.
        op: TemporalOp,
        /// Fenster `d`.
        window: Option<i64>,
        /// Formel.
        inner: Box<TProp>,
    },
    /// `a implies b`
    Implies(Box<TProp>, Box<TProp>),
    /// `a and b`
    And(Box<TProp>, Box<TProp>),
    /// `a or b`
    Or(Box<TProp>, Box<TProp>),
    /// `not a`
    Not(Box<TProp>),
    /// Vergleichsausdruck.
    Atom(Expr),
}

/// Was von einem Job gelesen wird (4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobField {
    /// `v.done : bool`.
    Done,
    /// `v.result : T!JobErr`.
    Result,
}
