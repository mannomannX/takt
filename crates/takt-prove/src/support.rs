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
        Construct::Convert(ConvertKind::ToFloat | ConvertKind::As | ConvertKind::To) => Yes,
        Construct::Mat(MatOp::Transpose) => Yes,
        // 3.11: In der Auswertung genau aus `libtaktm::mat`, im Solver eine
        // Funktion ohne Schranken.
        Construct::Mat(MatOp::Inv | MatOp::Det | MatOp::Solve | MatOp::Cholesky) => {
            Partial("im Solver uninterpretiert")
        }
        Construct::Builtin(Builtin::Now | Builtin::Tick) => Yes,
        Construct::Builtin(Builtin::TimeInState) => Partial("`time_in_state` nur in einem Zustand"),
        Construct::Builtin(Builtin::LastFault) => Yes,
        Construct::Builtin(Builtin::Event) => Yes,
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
        Construct::Match(MatchKind::Matches | MatchKind::Has) => Partial("kein `{x:float}`"),
        Construct::Stmt(s) => stmt(s),
        Construct::Place(PlaceTag::Var | PlaceTag::Output | PlaceTag::Field | PlaceTag::Index) => Yes,
        Construct::Place(PlaceTag::Port) => Yes,
        Construct::Place(PlaceTag::Index2) => Yes,
        Construct::Type(t) => ty(t),
        Construct::Seq(
            SeqTag::Stmt | SeqTag::Wait | SeqTag::Expect | SeqTag::Repeat | SeqTag::Step | SeqTag::Until,
        ) => Yes,
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
        ExprTag::Index => Partial("auf Arrays, Samples, Bytes und Vektoren; nicht auf Channel-Arrays"),
        ExprTag::Published | ExprTag::StateOf | ExprTag::Signal => Yes,
        ExprTag::Cast => Partial("nur zwischen Ganzzahlen und von Ganzzahl nach Fliesskomma"),
        ExprTag::Call => Partial("Funktionen aus kodierbaren Anweisungen"),
        ExprTag::Str => Yes,
        ExprTag::Format => Partial("Ganzzahlen, Wahrheitswerte, Varianten ohne Felder und Text; keine Fliesskommazahl"),
        ExprTag::Ok | ExprTag::Err => Yes,
        ExprTag::Tuple => Partial("nur als Punkt einer konstanten Tabelle von `interp`"),
        ExprTag::Index2 => Yes,
        ExprTag::Slice => Partial("auf Bytes und Vektoren; nicht auf einem Array"),
        ExprTag::BlockInit => Partial("Blockinstanzen nur ueber ihre Felder"),
        ExprTag::Accessor => Partial("die Qualitaet eines Inputs, ein Optional, die Laenge einer Sammlung"),
        ExprTag::PortRead => Partial("nicht mit einem Strom als Modell des Lesekanals"),
        ExprTag::Armed => Yes,
        ExprTag::Stream => Yes,
        ExprTag::Matches => Partial("kein `{x:float}`; eine Bindung nicht im Rumpf einer Funktion"),
        ExprTag::JobState => Partial("nur Jobs der kuratierten Menge, keine Projekt-Native"),
        ExprTag::Decode => Partial("ohne Fliesskommafelder"),
        ExprTag::NativeCall => Partial("nur die kuratierte Menge, keine Projekt-Native"),
        ExprTag::MatOp => Partial("`det`, `inv`, `solve` und `cholesky` im Solver uninterpretiert"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn intrinsic(i: Intrinsic) -> Support {
    match i {
        Intrinsic::Abs
        | Intrinsic::Min
        | Intrinsic::Max
        | Intrinsic::Rotl
        | Intrinsic::Rotr
        | Intrinsic::WrappingAdd
        | Intrinsic::WrappingSub
        | Intrinsic::WrappingMul
        | Intrinsic::SaturatingAdd
        | Intrinsic::SaturatingSub => Yes,
        Intrinsic::Sqrt | Intrinsic::Fma | Intrinsic::Round | Intrinsic::Floor | Intrinsic::Ceil => {
            Partial("nur auf Fliesskomma")
        }
        // 4.2: In der Auswertung genau aus `libtaktm`, im Solver eine Funktion
        // mit den Schranken ihres Wertebereichs.
        Intrinsic::Sin
        | Intrinsic::Cos
        | Intrinsic::Tan
        | Intrinsic::Asin
        | Intrinsic::Acos
        | Intrinsic::Atan
        | Intrinsic::Atan2
        | Intrinsic::Exp
        | Intrinsic::Log
        | Intrinsic::Pow => Partial("im Solver uninterpretiert mit den Schranken ihres Wertebereichs"),
        Intrinsic::Interp => Partial("ueber einer konstanten Tabelle"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn accessor(a: AccessorTag) -> Support {
    match a {
        AccessorTag::Valid | AccessorTag::Or => {
            Partial("auf einem Input (3.5), einem Optional und einem Ergebnis (3.8)")
        }
        AccessorTag::Suspect | AccessorTag::Stale => Partial("auf einem Input (3.5)"),
        // Beobachtungen laesst das Modell aus; nur dort darf ein Programm sie lesen.
        AccessorTag::Age | AccessorTag::Reason => Partial("nur in Beobachtungen, die das Modell auslaesst"),
        AccessorTag::Len => Partial("auf Arrays, Bytes, Vektoren und Text"),
        AccessorTag::StartsWith
        | AccessorTag::Contains
        | AccessorTag::Truncated
        | AccessorTag::Bit
        | AccessorTag::Bits
        | AccessorTag::WithBit => Yes,
        AccessorTag::Count => Partial("auf Arrays, Samples und Stroemen"),
        AccessorTag::Dropped
        | AccessorTag::Malformed
        | AccessorTag::Overflowed
        | AccessorTag::Peek
        | AccessorTag::Free
        | AccessorTag::Idle
        | AccessorTag::Sent => Yes,
        AccessorTag::Wrap => Yes,
        AccessorTag::Ok | AccessorTag::Err | AccessorTag::Get => Yes,
        AccessorTag::Min | AccessorTag::Max | AccessorTag::Mean | AccessorTag::Rms | AccessorTag::Last => Yes,
        AccessorTag::Encode => Partial("ohne Fliesskommafelder"),
        AccessorTag::T | AccessorTag::Pre | AccessorTag::Post | AccessorTag::Samples | AccessorTag::Rate => Yes,
        AccessorTag::Seq
        | AccessorTag::Text
        | AccessorTag::Data
        | AccessorTag::Jitter
        | AccessorTag::TimeWarped
        | AccessorTag::Done
        | AccessorTag::Result
        | AccessorTag::Armed
        | AccessorTag::Remaining => No("Zugriffe sind nicht kodiert (FB-372, FB-373)"),
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
        StmtTag::Check => Yes,
        StmtTag::ForRange => Partial("bis `UNROLL_LIMIT` Durchlaeufe auf einem Pfad (FB-403)"),
        StmtTag::Match => Partial("nicht im Rumpf einer Funktion"),
        StmtTag::MethodCall => Partial(
            "`step` einer Blockinstanz; `push`, `append`, `clear`, `insert`, `remove` auf Bytes, Vektoren und maps \
             ohne Index",
        ),
        StmtTag::ForEach => Partial(
            "ueber Arrays, Bytes, Vektoren, maps und das Fenster eines Stroms; kein `every` und kein `check … for` \
             in einer Schleife ueber eine map",
        ),
        StmtTag::Break | StmtTag::Skip => Yes,
        StmtTag::Send => Partial("auf einen Ausgabestrom Text, Bytes, Ganzzahlen, Wahrheitswerte und Arrays daraus"),
        StmtTag::Cancel => Yes,
        StmtTag::Arm => Yes,
        StmtTag::Every => Yes,
        StmtTag::At => Yes,
        StmtTag::Job => Partial("nur Jobs der kuratierten Menge, keine Projekt-Native"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn ty(t: TypeTag) -> Support {
    match t {
        TypeTag::Bool | TypeTag::Float | TypeTag::Duration | TypeTag::Enum | TypeTag::Result => Yes,
        TypeTag::Record | TypeTag::Array | TypeTag::Optional | TypeTag::Bytes | TypeTag::Vec => Yes,
        TypeTag::Int => Yes,
        TypeTag::HandleBlock => Partial("Blockinstanzen nur ueber ihre Felder"),
        TypeTag::Stream => Partial(
            "Eingabestroeme mit `max_rate` oder aus einem `sim`-Ausgang, Records vom Rand fester Groesse; \
             Ausgabestroeme ohne Leser",
        ),
        // Ein Text vom Rand ist gueltiges UTF-8; das Modell nimmt jedes Byte.
        TypeTag::Str | TypeTag::Line => Partial("als Input ohne die UTF-8-Pruefung des Randes"),
        TypeTag::Table => Partial("nur als konstante Tabelle von `interp`"),
        TypeTag::Map => Yes,
        TypeTag::Capture | TypeTag::HandleJob | TypeTag::Samples => Yes,
        TypeTag::Mat => Yes,
        TypeTag::HandleTrigger => NO_SORT,
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
        Feature::MachineHandler | Feature::StateHandler | Feature::HandlerGuard | Feature::GuardNext => Yes,
        Feature::HandlerPattern | Feature::GuardMatch => Partial("kein `{x:float}`"),
        Feature::InputStream => Partial(
            "mit `max_rate` oder aus einem `sim`-Ausgang mit Elementen `u8` oder Bytes; ein Record vom Rand fester \
             Groesse",
        ),
        Feature::InternalStream => Yes,
        Feature::OutputStream => Partial("ohne Leser; `send` von Text, Bytes, Ganzzahlen und Arrays daraus"),
        Feature::FramingRaw
        | Feature::FramingLines
        | Feature::FramingCobs
        | Feature::FramingLengthPrefixed
        | Feature::FramingFixed => Yes,
        Feature::FaultedTransition => No("Uebergaenge aus FAULTED sind nicht kodiert"),
        Feature::ScopedInstance => Partial("ohne Instanzen, die einen Strom lesen"),
        Feature::CheckConfirm => Yes,
        Feature::CheckWithin => Yes,
        Feature::Port => Partial("nicht mit einem Strom als Modell des Lesekanals"),
        Feature::Trigger => Yes,
    }
}
