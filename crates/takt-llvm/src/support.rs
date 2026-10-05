//! Was der Codegen traegt: seine Zeile der Faehigkeitsmatrix (13.8,
//! Schritt 25, FB-377).
//!
//! Ein `match` ohne Platzhalter ueber [`Construct`]: Kommt eine Konstruktion
//! in die MIR, uebersetzt dieses Crate erst, wenn hier entschieden ist, ob der
//! Codegen sie senkt. `Partial` heisst: in manchen Zusammenhaengen ja, in
//! anderen meldet `lower` sie als `Skipped`. Ob die Zeile stimmt, prueft
//! `takt-conformance/tests/capabilities.rs` am Korpus.

use takt_mir::census::{
    AccessorTag, CheckTag, Construct, ExprTag, Feature, PlaceTag, SeqTag, StmtTag, Support, TypeTag,
};
use takt_mir::expr::{BinaryOp, Builtin, ConvertKind, Intrinsic, MatOp, MatchKind, TemporalOp, UnaryOp};

use Support::{Partial, Yes};

/// Traegt der Codegen die Konstruktion?
#[deny(clippy::wildcard_enum_match_arm)]
pub fn support(c: Construct) -> Support {
    match c {
        Construct::Expr(e) => expr(e),
        Construct::Unary(UnaryOp::Neg | UnaryOp::Not | UnaryOp::BitNot) => Yes,
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
            | BinaryOp::Div
            | BinaryOp::Rem,
        ) => Yes,
        Construct::Convert(ConvertKind::To | ConvertKind::ToFloat) => Yes,
        Construct::Convert(ConvertKind::As) => Partial("`as` nicht zwischen allen Typen"),
        Construct::Mat(MatOp::Transpose | MatOp::Inv | MatOp::Det | MatOp::Solve | MatOp::Cholesky) => Yes,
        Construct::Builtin(Builtin::Now | Builtin::Tick | Builtin::TimeInState | Builtin::Event) => Yes,
        Construct::Builtin(Builtin::LastFault) => Partial("`LastFault` nur in der Gestalt `str<128>`, `int`, `int`"),
        Construct::Check(
            CheckTag::DivZero
            | CheckTag::Overflow
            | CheckTag::NonFinite
            | CheckTag::Domain
            | CheckTag::Index
            | CheckTag::Range
            | CheckTag::Convert
            | CheckTag::Shift
            | CheckTag::Valid
            | CheckTag::Missing,
        ) => Yes,
        Construct::Intrinsic(i) => intrinsic(i),
        Construct::Accessor(a) => accessor(a),
        Construct::Match(MatchKind::Matches | MatchKind::Has) => Yes,
        Construct::Stmt(s) => stmt(s),
        Construct::Place(
            PlaceTag::Var | PlaceTag::Output | PlaceTag::Port | PlaceTag::Field | PlaceTag::Index | PlaceTag::Index2,
        ) => Yes,
        Construct::Type(t) => ty(t),
        Construct::Seq(
            SeqTag::Stmt | SeqTag::Wait | SeqTag::Until | SeqTag::Expect | SeqTag::Repeat | SeqTag::Step,
        ) => Yes,
        Construct::Temporal(
            TemporalOp::Always | TemporalOp::Never | TemporalOp::Eventually | TemporalOp::Stable | TemporalOp::Once,
        ) => Yes,
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
        | ExprTag::Str
        | ExprTag::None
        | ExprTag::Default
        | ExprTag::Variant
        | ExprTag::Record
        | ExprTag::Array
        | ExprTag::Var
        | ExprTag::Param
        | ExprTag::Command
        | ExprTag::Input
        | ExprTag::Output
        | ExprTag::Published
        | ExprTag::StateOf
        | ExprTag::Signal
        | ExprTag::Builtin
        | ExprTag::Armed
        | ExprTag::PortRead
        | ExprTag::Field
        | ExprTag::Index
        | ExprTag::Index2
        | ExprTag::Slice
        | ExprTag::Accessor
        | ExprTag::Unary
        | ExprTag::Binary
        | ExprTag::Cond
        | ExprTag::Cast
        | ExprTag::Convert
        | ExprTag::JobState
        | ExprTag::Call
        | ExprTag::NativeCall
        | ExprTag::MatOp
        | ExprTag::Decode
        | ExprTag::Checked
        | ExprTag::Lift
        | ExprTag::Ok
        | ExprTag::Err
        | ExprTag::Intrinsic => Yes,
        ExprTag::Tuple => Partial("nur als Stuetzstelle von `interp`"),
        ExprTag::BlockInit => Partial("nur als Anfangswert einer Blockinstanz"),
        ExprTag::Format => Partial("nur in Meldungen und `send`; kein `float`, kein zusammengesetzter Wert"),
        ExprTag::Stream => Partial("nur als Empfaenger eines Zugriffs"),
        ExprTag::Matches => Partial("nur in Handlern, Guards und Bedingungen"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn intrinsic(i: Intrinsic) -> Support {
    match i {
        Intrinsic::Abs
        | Intrinsic::Min
        | Intrinsic::Max
        | Intrinsic::Sqrt
        | Intrinsic::Sin
        | Intrinsic::Cos
        | Intrinsic::Tan
        | Intrinsic::Asin
        | Intrinsic::Acos
        | Intrinsic::Atan
        | Intrinsic::Atan2
        | Intrinsic::Exp
        | Intrinsic::Log
        | Intrinsic::Pow
        | Intrinsic::Fma
        | Intrinsic::Round
        | Intrinsic::Floor
        | Intrinsic::Ceil
        | Intrinsic::Rotl
        | Intrinsic::Rotr
        | Intrinsic::WrappingAdd
        | Intrinsic::WrappingSub
        | Intrinsic::WrappingMul
        | Intrinsic::SaturatingAdd
        | Intrinsic::SaturatingSub => Yes,
        Intrinsic::Interp => Partial("nur ueber eine Tabelle aus Literalen mit mindestens zwei Stuetzstellen"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn accessor(a: AccessorTag) -> Support {
    match a {
        AccessorTag::Valid
        | AccessorTag::Suspect
        | AccessorTag::Stale
        | AccessorTag::Age
        | AccessorTag::Reason
        | AccessorTag::Ok
        | AccessorTag::T
        | AccessorTag::Seq
        | AccessorTag::Text
        | AccessorTag::Data
        | AccessorTag::Count
        | AccessorTag::Dropped
        | AccessorTag::Malformed
        | AccessorTag::Overflowed
        | AccessorTag::Free
        | AccessorTag::Jitter
        | AccessorTag::TimeWarped
        | AccessorTag::Done
        | AccessorTag::Result
        | AccessorTag::Bit
        | AccessorTag::WithBit
        | AccessorTag::Wrap
        | AccessorTag::Min
        | AccessorTag::Max
        | AccessorTag::Mean
        | AccessorTag::Rms
        | AccessorTag::Last
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
        | AccessorTag::Idle => Yes,
        AccessorTag::Or => Partial("`.or` nur auf einem Channel oder Wrapper"),
        AccessorTag::Err => Partial("`.err` nicht auf `T?`"),
        AccessorTag::Len => Partial("`.len` nur auf Sammlungen"),
        AccessorTag::Bits => Partial("`.bits` nur mit konstanten, geordneten Grenzen"),
        AccessorTag::Encode => Partial("`encode` nur auf Records"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn stmt(s: StmtTag) -> Support {
    match s {
        StmtTag::Assign
        | StmtTag::Check
        | StmtTag::Goto
        | StmtTag::Abort
        | StmtTag::If
        | StmtTag::ForRange
        | StmtTag::ForEach
        | StmtTag::Match
        | StmtTag::Return
        | StmtTag::Send
        | StmtTag::At
        | StmtTag::Cancel
        | StmtTag::Skip
        | StmtTag::Raise
        | StmtTag::Job
        | StmtTag::Every
        | StmtTag::Break
        | StmtTag::Observe
        | StmtTag::Arm
        | StmtTag::Pass => Yes,
        StmtTag::MethodCall => Partial("Methoden je nach Ziel: `step`, `reset`, Blockmethoden, `insert`, `remove`"),
    }
}

#[deny(clippy::wildcard_enum_match_arm)]
fn ty(t: TypeTag) -> Support {
    match t {
        TypeTag::Bool
        | TypeTag::Int
        | TypeTag::Float
        | TypeTag::Duration
        | TypeTag::Enum
        | TypeTag::Record
        | TypeTag::Array
        | TypeTag::Bytes
        | TypeTag::Vec
        | TypeTag::Str
        | TypeTag::Line
        | TypeTag::Samples
        | TypeTag::Table
        | TypeTag::Mat
        | TypeTag::Map
        | TypeTag::Optional
        | TypeTag::Result
        | TypeTag::Stream
        | TypeTag::Capture
        | TypeTag::HandleJob
        | TypeTag::HandleTrigger
        | TypeTag::HandleBlock => Yes,
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
        | Feature::FaultedTransition
        | Feature::MachineHandler
        | Feature::ChildStates
        | Feature::Idle
        | Feature::Resume
        | Feature::Exit
        | Feature::Sequence
        | Feature::ScopedInstance
        | Feature::StateHandler
        | Feature::StateFaultTarget
        | Feature::HandlerGuard
        | Feature::After
        | Feature::GuardExpr
        | Feature::GuardNext
        | Feature::TargetState
        | Feature::TargetFaulted
        | Feature::TargetFault
        | Feature::CheckConfirm
        | Feature::CheckWithin
        | Feature::Expect
        | Feature::ArmVariant
        | Feature::ArmValues
        | Feature::ArmWild
        | Feature::EnumWithFields
        | Feature::Function
        | Feature::Block
        | Feature::Native
        | Feature::InputStream
        | Feature::OutputStream
        | Feature::InternalStream
        | Feature::Command
        | Feature::Param
        | Feature::Tunable
        | Feature::Profile
        | Feature::Property
        | Feature::Assumption
        | Feature::Monitor
        | Feature::Trigger
        | Feature::Port
        | Feature::Recorded
        | Feature::Campaign
        | Feature::Node
        | Feature::Label
        | Feature::Display
        | Feature::Group => Yes,
        // Die Rahmung schneidet der Rand der Runtime (8.6, 12.6); der
        // erzeugte Code sieht nur die fertigen Elemente.
        Feature::FramingRaw
        | Feature::FramingLines
        | Feature::FramingCobs
        | Feature::FramingLengthPrefixed
        | Feature::FramingFixed => Yes,
        Feature::HandlerPattern => Partial("kein `{x:float}` im Handler-Muster"),
        // Text nach Fliesskomma waere eine zweite Rundungsquelle neben der
        // des Interpreters (4.2, `captures.rs`); der Guard teilt die Grenze
        // des Handlers (GEN-014).
        Feature::GuardMatch => Partial("kein `{x:float}` im Guard-Muster"),
    }
}
