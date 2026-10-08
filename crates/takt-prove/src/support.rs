//! Was die Kodierung traegt: die Zeile des Beweisers in der
//! Faehigkeitsmatrix (13.3, 13.8, Schritt 25, FB-377).
//!
//! Ein `match` ohne Platzhalter ueber [`Construct`]: Kommt eine Konstruktion
//! in die MIR, uebersetzt dieses Crate erst, wenn hier entschieden ist, ob
//! [`crate::encode`] sie kodiert. `No` heisst: Ein Programm mit ihr ist kein
//! Gesamtmodell (oder eine Eigenschaft mit ihr bleibt ungeprueft);
//! `Partial`: in manchen Zusammenhaengen. Ob die Zeile stimmt, prueft
//! `takt-conformance/tests/capabilities.rs` am Korpus.

use takt_mir::census::{
    AccessorTag, CheckTag, Construct, ExprTag, Feature, PlaceTag, SeqTag, StmtTag, Support, TypeTag,
};
use takt_mir::expr::{BinaryOp, Builtin, ConvertKind, Intrinsic, MatOp, MatchKind, TemporalOp, UnaryOp};

use Support::{No, Partial, Yes};

/// Ein Typ ohne Sorte im Modell (13.3: Ganzzahlen, Enums, Dauern, Fliesskomma).
const NO_SORT: Support = No("Typ ohne Sorte im Modell");

/// Kodiert der Beweiser die Konstruktion?
#[deny(clippy::wildcard_enum_match_arm)]
pub fn support(c: Construct) -> Support {
    match c {
        Construct::Expr(e) => expr(e),
        Construct::Unary(UnaryOp::Neg | UnaryOp::Not) => Yes,
        Construct::Unary(UnaryOp::BitNot) => No("`~` ist nicht kodiert"),
        Construct::Binary(
            BinaryOp::Or
            | BinaryOp::And
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge
            | BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::BitAnd
            | BinaryOp::Shl
            | BinaryOp::Shr
            | BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div,
        ) => Yes,
        Construct::Binary(BinaryOp::Rem) => Partial("`%` nur auf Ganzzahlen"),
        Construct::Convert(ConvertKind::ToFloat | ConvertKind::As) => Yes,
        Construct::Convert(ConvertKind::To) => No("Einheitenumrechnung `.to`"),
        Construct::Mat(MatOp::Transpose | MatOp::Inv | MatOp::Det | MatOp::Solve | MatOp::Cholesky) => {
            No("Matrizen sind nicht kodiert")
        }
        Construct::Builtin(Builtin::Now | Builtin::Tick) => Yes,
        Construct::Builtin(Builtin::TimeInState) => Partial("`time_in_state` nur in einem Zustand"),
        Construct::Builtin(Builtin::LastFault | Builtin::Event) => No("eingebaute Groesse ohne Modell"),
        Construct::Check(
            CheckTag::DivZero
            | CheckTag::NonFinite
            | CheckTag::Range
            | CheckTag::Overflow
            | CheckTag::Shift
            | CheckTag::Convert
            | CheckTag::Domain
            | CheckTag::Valid
            | CheckTag::Index
            | CheckTag::Missing,
        ) => Yes,
        Construct::Intrinsic(i) => intrinsic(i),
        Construct::Accessor(a) => accessor(a),
        Construct::Match(MatchKind::Matches | MatchKind::Has) => No("Mustervergleich ist nicht kodiert"),
        Construct::Stmt(s) => stmt(s),
        Construct::Place(PlaceTag::Var | PlaceTag::Output | PlaceTag::Field | PlaceTag::Index) => Yes,
        Construct::Place(PlaceTag::Port | PlaceTag::Index2) => No("Zuweisung an Port oder Matrixelement"),
        Construct::Type(t) => ty(t),
        Construct::Seq(SeqTag::Stmt | SeqTag::Wait | SeqTag::Expect | SeqTag::Repeat | SeqTag::Step) => Yes,
        Construct::Seq(SeqTag::Until) => Partial("kein Timeout-Fault einer Sequenz"),
        Construct::Temporal(TemporalOp::Always | TemporalOp::Never) => Yes,
        Construct::Temporal(TemporalOp::Once) => Yes,
        Construct::Temporal(TemporalOp::Eventually | TemporalOp::Stable) => Partial(
            "beliebig lang als `a implies eventually[d](b)` und `a implies stable[d](b)`, sonst bis 256 Ticks Fenster",
        ),
        Construct::Feature(f) => feature(f),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn expr(e: ExprTag) -> Support {
    match e {
        ExprTag::Bool
        | ExprTag::Int
        | ExprTag::Float
        | ExprTag::Duration
        | ExprTag::Default
        | ExprTag::Var
        | ExprTag::Param
        | ExprTag::Command
        | ExprTag::Input
        | ExprTag::Output
        | ExprTag::Builtin
        | ExprTag::Unary
        | ExprTag::Binary
        | ExprTag::Cond
        | ExprTag::Convert
        | ExprTag::Checked
        | ExprTag::Intrinsic
        | ExprTag::Variant
        | ExprTag::None
        | ExprTag::Record
        | ExprTag::Array
        | ExprTag::Lift => Yes,
        ExprTag::Field => Partial("auf Records; nicht auf den Feldern einer Variante"),
        ExprTag::Index => Partial("auf Arrays, Bytes und Vektoren; nicht auf Samples und Channel-Arrays"),
        ExprTag::Published | ExprTag::StateOf | ExprTag::Signal => Partial("nicht ueber ein Instanz-Array"),
        ExprTag::Cast => Partial("nur zwischen Ganzzahlen und von Ganzzahl nach Fliesskomma"),
        ExprTag::Call => Partial("Funktionen mit Rueckgabe, ohne `inout`, aus kodierbaren Anweisungen"),
        ExprTag::Str | ExprTag::Format => No("Text ist nicht kodiert"),
        ExprTag::Tuple | ExprTag::Index2 | ExprTag::Slice | ExprTag::Ok | ExprTag::Err => {
            No("Tupel, Matrizen, Ausschnitte und Ergebnisse sind nicht kodiert")
        }
        ExprTag::BlockInit => Partial("Blockinstanzen nur ueber ihre Felder"),
        ExprTag::Accessor => Partial("die Qualitaet eines Inputs, ein Optional, die Laenge einer Sammlung"),
        ExprTag::Armed | ExprTag::PortRead => No("Trigger und Registerports sind nicht kodiert"),
        ExprTag::JobState | ExprTag::Stream | ExprTag::Matches | ExprTag::Decode => {
            No("Jobs, Stroeme und Muster sind nicht kodiert")
        }
        ExprTag::NativeCall | ExprTag::MatOp => No("Natives und Matrizen sind nicht kodiert"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn intrinsic(i: Intrinsic) -> Support {
    match i {
        Intrinsic::Abs | Intrinsic::Min | Intrinsic::Max => Yes,
        Intrinsic::Sqrt | Intrinsic::Fma => Partial("nur auf Fliesskomma"),
        Intrinsic::Sin
        | Intrinsic::Cos
        | Intrinsic::Tan
        | Intrinsic::Asin
        | Intrinsic::Acos
        | Intrinsic::Atan
        | Intrinsic::Atan2
        | Intrinsic::Exp
        | Intrinsic::Log
        | Intrinsic::Pow
        | Intrinsic::Round
        | Intrinsic::Floor
        | Intrinsic::Ceil
        | Intrinsic::Rotl
        | Intrinsic::Rotr
        | Intrinsic::WrappingAdd
        | Intrinsic::WrappingSub
        | Intrinsic::WrappingMul
        | Intrinsic::SaturatingAdd
        | Intrinsic::SaturatingSub
        | Intrinsic::Interp => No("Primitive ohne Kodierung"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn accessor(a: AccessorTag) -> Support {
    match a {
        AccessorTag::Valid | AccessorTag::Or => {
            Partial("auf einem Input (3.5) und einem Optional (3.8); nicht auf einem Ergebnis")
        }
        AccessorTag::Suspect | AccessorTag::Stale => Partial("auf einem Input (3.5)"),
        // Beobachtungen laesst das Modell aus; nur dort darf ein Programm sie lesen.
        AccessorTag::Age | AccessorTag::Reason => Partial("nur in Beobachtungen, die das Modell auslaesst"),
        AccessorTag::Len => Partial("auf Arrays, Bytes und Vektoren"),
        AccessorTag::Count => Partial("auf Arrays"),
        AccessorTag::Ok
        | AccessorTag::Err
        | AccessorTag::T
        | AccessorTag::Seq
        | AccessorTag::Text
        | AccessorTag::Data
        | AccessorTag::Dropped
        | AccessorTag::Malformed
        | AccessorTag::Overflowed
        | AccessorTag::Free
        | AccessorTag::Jitter
        | AccessorTag::TimeWarped
        | AccessorTag::Done
        | AccessorTag::Result
        | AccessorTag::Bit
        | AccessorTag::Bits
        | AccessorTag::WithBit
        | AccessorTag::Wrap
        | AccessorTag::Min
        | AccessorTag::Max
        | AccessorTag::Mean
        | AccessorTag::Rms
        | AccessorTag::Last
        | AccessorTag::Encode
        | AccessorTag::Get
        | AccessorTag::StartsWith
        | AccessorTag::Contains
        | AccessorTag::Armed
        | AccessorTag::Pre
        | AccessorTag::Post
        | AccessorTag::Samples
        | AccessorTag::Rate
        | AccessorTag::Remaining
        | AccessorTag::Truncated
        | AccessorTag::Peek
        | AccessorTag::Sent
        | AccessorTag::Idle => No("Zugriffe sind nicht kodiert (FB-372, FB-373)"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn stmt(s: StmtTag) -> Support {
    match s {
        StmtTag::Assign
        | StmtTag::Goto
        | StmtTag::Abort
        | StmtTag::If
        | StmtTag::Return
        | StmtTag::Raise
        | StmtTag::Observe
        | StmtTag::Pass => Yes,
        StmtTag::Check => Partial("ohne `for` und `within`"),
        StmtTag::ForRange => Partial("bis `UNROLL_LIMIT` Durchlaeufe auf einem Pfad (FB-403)"),
        StmtTag::Match => Partial("nicht im Rumpf einer Funktion"),
        StmtTag::MethodCall => {
            Partial("`step` einer Blockinstanz; `push`, `append`, `clear` auf Bytes und Vektoren ohne Index")
        }
        StmtTag::ForEach => {
            Partial("ueber ein Array, Bytes oder einen Vektor mit einer Variablen, nicht im Rumpf einer Funktion")
        }
        StmtTag::Break => No("`break` ist nicht kodiert"),
        StmtTag::Send | StmtTag::Cancel | StmtTag::Skip | StmtTag::Arm => No("Stroeme und Trigger sind nicht kodiert"),
        StmtTag::At | StmtTag::Every => No("`at` und `every` sind nicht kodiert"),
        StmtTag::Job => No("Jobs sind nicht kodiert"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn ty(t: TypeTag) -> Support {
    match t {
        TypeTag::Bool | TypeTag::Float | TypeTag::Duration => Yes,
        TypeTag::Enum => Partial("mit Feldern nicht als Typ eines Inputs"),
        TypeTag::Record | TypeTag::Array | TypeTag::Optional | TypeTag::Bytes | TypeTag::Vec => {
            Partial("nicht als Typ eines Inputs")
        }
        TypeTag::Int => Partial("ohne `u64`: Die Kodierung rechnet in 64 Bit mit Vorzeichen"),
        TypeTag::HandleBlock => Partial("Blockinstanzen nur ueber ihre Felder"),
        TypeTag::Str
        | TypeTag::Line
        | TypeTag::Samples
        | TypeTag::Table
        | TypeTag::Mat
        | TypeTag::Map
        | TypeTag::Result
        | TypeTag::Stream
        | TypeTag::Capture
        | TypeTag::HandleJob
        | TypeTag::HandleTrigger => NO_SORT,
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn feature(f: Feature) -> Support {
    match f {
        Feature::Multirate
        | Feature::Phase
        | Feature::Follows
        | Feature::DriverMachine
        | Feature::Template
        | Feature::Instance
        | Feature::Scenario
        | Feature::Persist
        | Feature::Signal
        | Feature::ChildStates
        | Feature::Idle
        | Feature::Resume
        | Feature::Exit
        | Feature::Sequence
        | Feature::StateFaultTarget
        | Feature::After
        | Feature::GuardExpr
        | Feature::TargetState
        | Feature::TargetFaulted
        | Feature::TargetFault
        | Feature::Expect
        | Feature::ArmVariant
        | Feature::ArmWild
        | Feature::ArmValues
        | Feature::EnumWithFields
        | Feature::Tunable
        | Feature::Function
        | Feature::Block
        | Feature::Command
        | Feature::Param
        | Feature::Profile
        | Feature::Property
        | Feature::Assumption
        | Feature::Monitor
        | Feature::Recorded
        | Feature::Campaign
        | Feature::Node
        | Feature::Native
        | Feature::Label
        | Feature::Display
        | Feature::Group => Yes,
        Feature::MachineHandler
        | Feature::StateHandler
        | Feature::HandlerPattern
        | Feature::HandlerGuard
        | Feature::InputStream
        | Feature::OutputStream
        | Feature::InternalStream
        | Feature::FramingRaw
        | Feature::FramingLines
        | Feature::FramingCobs
        | Feature::FramingLengthPrefixed
        | Feature::FramingFixed => No("Handler und Stroeme sind nicht kodiert"),
        Feature::FaultedTransition => No("Uebergaenge aus FAULTED sind nicht kodiert"),
        Feature::ScopedInstance => No("gescopte Instanzen sind nicht kodiert"),
        Feature::GuardMatch | Feature::GuardNext => No("Guards mit Muster sind nicht kodiert"),
        Feature::CheckConfirm | Feature::CheckWithin => No("`check … for` und `within` sind nicht kodiert"),
        Feature::Trigger | Feature::Port => No("Trigger und Registerports sind nicht kodiert"),
    }
}
