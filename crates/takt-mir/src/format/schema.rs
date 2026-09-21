//! Schema von `TAKT-MIR`: Feldnummern aller Knoten (grammar/mir-format.md,
//! Abschnitt S). Regeln: Nummern werden nie umvergeben; neue Felder bekommen
//! neue Nummern und sind `opt` oder `rep`, damit alte Dateien lesbar bleiben;
//! `meta` markiert Positionen, Bindungen und Metadaten (nicht im Logik-Hash).

use super::codec::*;
use super::wire::{FormatError, Raw, Reader, Result, Wire, Writer};
use crate::expr::*;
use crate::fns::*;
use crate::machine::*;
use crate::pattern::*;
use crate::program::*;
use crate::stmt::*;
use crate::types::*;

codec_id!(
    TypeId, UnitId, EnumId, RecordId, FnId, NativeId, BlockId, MachineId, ChannelId, StreamId, ParamId, ProfileId,
    CommandId, NodeId, PropertyId, CampaignId, TriggerId, StateId, VarId, SignalId, SiteId, CounterId,
);

// ---------------------------------------------------------------- Typen

codec_unit_enum!(IntWidth { 0 I8, 1 I16, 2 I32, 3 I64, 4 U8, 5 U16, 6 U32, 7 U64 });
codec_unit_enum!(FloatWidth { 0 F32, 1 F64 });
codec_unit_enum!(RangeOrigin { 0 Declared, 1 Proven });
codec_enum!(HandleKind { 0 Job, 1 Trigger, 2 Block(1 one b) });
codec_unit_enum!(JobField { 0 Done, 1 Result });
codec_unit_enum!(Endian { 0 Little, 1 Big });
codec_enum!(Const { 0 Int(1 one v), 1 Float(1 one v), 2 Duration(1 one v), 3 Bool(1 one v) });
codec_struct!(Range { 1 one lo, 2 one hi, 3 one origin });
codec_enum!(MatUnits { 0 Uniform(1 opt u), 1 Dimensioned { 1 rep rows, 2 rep cols } });
codec_enum!(Type {
    0 Bool,
    1 Int { 1 one width, 2 opt unit, 3 opt range },
    2 Float { 1 one width, 2 opt unit, 3 opt range },
    3 Duration { 1 opt range },
    4 Enum(1 one id),
    5 Record(1 one id),
    6 Array { 1 one elem, 2 one len },
    7 Bytes { 1 one cap },
    8 Vec { 1 one elem, 2 one cap },
    9 Str { 1 one cap },
    10 Line { 1 one cap },
    11 Samples { 1 one elem, 2 one len },
    12 Table { 1 one key, 2 one value },
    13 Mat { 1 one rows, 2 one cols, 3 one units },
    14 Map { 1 one key, 2 one value, 3 one cap },
    15 Optional(1 one t),
    16 Result { 1 one ok, 2 one err },
    17 Stream(1 one elem),
    18 Capture { 1 one elem, 2 one len },
    19 Handle(1 one kind),
});
codec_struct!(VariantDef { 1 one name, 2 one discriminant, 3 rep fields, 4 meta span });
codec_struct!(EnumDef { 1 one name, 2 rep variants, 3 opt layout, 4 one open, 5 one builtin, 6 meta span });
codec_struct!(BitfieldDef { 1 one name, 2 one ty, 3 one lo, 4 one hi, 5 meta span });
codec_struct!(FieldDef { 1 one name, 2 one ty, 3 opt const_value, 4 opt offset, 5 opt len_field, 6 rep bits, 7 meta span });
codec_struct!(WireLayout { 1 one endian, 2 opt align });
codec_struct!(RecordDef { 1 one name, 2 rep fields, 3 opt layout, 4 one builtin, 5 meta span, 6 opt wire_size });
codec_struct!(Rational { 1 one num, 2 one den });
codec_struct!(UnitDef { 1 one name, 2 one dimension, 3 one factor, 4 opt affine_offset, 5 one predefined, 6 meta span });
codec_struct!(TypeTable { 1 rep list });

// ---------------------------------------------------------------- Muster

codec_enum!(CaptureKind { 0 Int, 1 Hex, 2 Float, 3 Word, 4 Str(1 one n) });
codec_enum!(PatternPiece { 0 Text(1 one s), 1 Capture { 1 one name, 2 one kind }, 2 Any });
codec_struct!(Dfa { 1 rep classes, 2 one class_count, 3 rep table, 4 rep accept });
codec_enum!(Pattern { 0 Text { 1 rep pieces, 2 opt dfa }, 1 Record { 1 one record, 2 rep fields } });
codec_enum!(FormatPiece { 0 Text(1 one s), 1 Expr { 1 one expr, 2 opt spec } });
codec_struct!(Format { 1 rep pieces, 2 one len_max });
codec_struct!(AddressSegment { 1 one name, 2 opt range });
codec_struct!(Address { 1 rep segments });

// ---------------------------------------------------------------- Ausdruecke

codec_struct!(Expr { 1 one kind, 2 one ty, 3 opt range, 4 meta span, 5 opt repr });
codec_unit_enum!(Repr { 0 I32, 1 I64 });
codec_struct!(MachineRef { 1 one machine, 2 opt index });
codec_enum!(StreamRef { 0 Channel(1 one c), 1 Internal(1 one s), 2 Fired(1 one t), 3 Var(1 one v) });
codec_unit_enum!(Builtin { 0 Now, 1 Tick, 2 TimeInState, 3 LastFault, 4 Event });
codec_enum!(Accessor {
    0 Valid, 1 Suspect, 2 Stale, 3 Age, 4 Reason, 5 Or, 6 Ok, 7 Err, 8 T, 9 Seq, 10 Text, 11 Data, 12 Len,
    13 Count, 14 Dropped, 15 Malformed, 16 Overflowed, 17 Free, 18 Jitter, 19 TimeWarped, 20 Done, 21 Result,
    22 Bit, 23 Bits, 24 WithBit, 25 Wrap(1 one w), 26 Min, 27 Max, 28 Mean, 29 Rms, 30 Last, 31 Encode, 32 Get,
    33 StartsWith, 34 Contains, 35 Armed, 36 Pre, 37 Post, 38 Samples, 39 Rate, 40 Remaining, 41 Truncated,
    42 Peek, 43 Sent,
});
codec_unit_enum!(UnaryOp { 0 Neg, 1 Not, 2 BitNot });
codec_unit_enum!(BinaryOp {
    0 Or, 1 And, 2 Lt, 3 Le, 4 Gt, 5 Ge, 6 Eq, 7 Ne, 8 BitOr, 9 BitXor, 10 BitAnd, 11 Shl, 12 Shr, 13 Add,
    14 Sub, 15 Mul, 16 Div, 17 Rem,
});
codec_unit_enum!(ConvertKind { 0 To, 1 ToFloat, 2 As });
codec_unit_enum!(MatOp { 0 Transpose, 1 Inv, 2 Det, 3 Solve, 4 Cholesky });
codec_unit_enum!(MatchKind { 0 Matches, 1 Has });
codec_enum!(CheckedKind {
    0 DivZero, 1 Overflow, 2 NonFinite, 3 Domain, 4 Index { 1 one len }, 5 Range(1 one r), 6 Convert, 7 Shift,
    8 Valid, 9 Missing,
});
codec_enum!(ExprKind {
    0 Bool(1 one v),
    1 Int(1 one v),
    2 Float(1 one v),
    3 Duration(1 one v),
    4 Str(1 one v),
    5 None,
    6 Default,
    7 Variant { 1 one enum_id, 2 one variant, 3 rep fields },
    8 Record { 1 one record, 2 rep fields },
    9 Array(1 rep items),
    10 Tuple(1 one a, 2 one b),
    11 BlockInit { 1 one block, 2 rep args, 3 opt count },
    12 Var(1 one v),
    13 Param(1 one p),
    14 Command(1 one c),
    15 Input { 1 one channel, 2 one dominated },
    16 Output(1 one c),
    17 Published { 1 one machine, 2 one var },
    18 StateOf(1 one m),
    19 Signal { 1 one machine, 2 one signal },
    20 Builtin(1 one b),
    21 Field { 1 one base, 2 one field },
    22 Index { 1 one base, 2 one index },
    23 Index2 { 1 one base, 2 one row, 3 one col },
    24 Slice { 1 one base, 2 one from, 3 one to },
    25 Accessor { 1 one base, 2 one accessor, 3 rep args },
    26 Unary { 1 one op, 2 one expr },
    27 Binary { 1 one op, 2 one lhs, 3 one rhs },
    28 Cond { 1 one cond, 2 one then, 3 one otherwise },
    29 Cast { 1 one expr, 2 one to },
    30 Convert { 1 one expr, 2 one kind, 3 one unit },
    31 Matches { 1 one subject, 2 one kind, 3 one pattern, 4 opt binding },
    32 Call { 1 one callee, 2 rep args },
    33 NativeCall { 1 one native, 2 rep args },
    34 MatOp { 1 one op, 2 rep args },
    35 Decode { 1 one record, 2 one bytes },
    36 Checked { 1 one expr, 2 one kind },
    37 Lift(1 one e),
    38 Ok(1 one e),
    39 Err(1 one e),
    40 Intrinsic { 1 one op, 2 rep args },
    41 Stream(1 one s),
    42 Format(1 one f),
    43 JobState { 1 one handle, 2 one field },
    44 Armed(1 one t),
});
codec_unit_enum!(Intrinsic {
    0 Abs, 1 Min, 2 Max, 3 Sqrt, 4 Sin, 5 Cos, 6 Tan, 7 Asin, 8 Acos, 9 Atan, 10 Atan2, 11 Exp, 12 Log, 13 Pow,
    14 Fma, 15 Round, 16 Floor, 17 Ceil, 18 Rotl, 19 Rotr, 20 WrappingAdd, 21 WrappingSub, 22 WrappingMul,
    23 SaturatingAdd, 24 SaturatingSub, 25 Interp,
});
codec_unit_enum!(TemporalOp { 0 Always, 1 Never, 2 Eventually, 3 Stable, 4 Once });
codec_enum!(TProp {
    0 Temporal { 1 one op, 2 opt window, 3 one inner },
    1 Implies(1 one a, 2 one b),
    2 And(1 one a, 2 one b),
    3 Or(1 one a, 2 one b),
    4 Not(1 one a),
    5 Atom(1 one e),
});

// ---------------------------------------------------------------- Anweisungen

codec_struct!(Block { 1 rep stmts, 2 meta span });
codec_struct!(Stmt { 1 one kind, 2 meta span });
codec_enum!(Place {
    0 Var(1 one v),
    1 Output(1 one c),
    2 Field(1 one base, 2 one field),
    3 Index(1 one base, 2 one index),
    4 Index2(1 one base, 2 one row, 3 one col),
});
codec_unit_enum!(CheckKind { 0 Check, 1 Expect });
codec_struct!(Confirm { 1 one duration, 2 one site });
codec_enum!(ForVars { 0 One(1 one v), 1 Pair(1 one k, 2 one v) });
codec_struct!(CaseValue { 1 one lo, 2 opt hi });
codec_enum!(ArmPattern { 0 Variant { 1 one variant, 2 rep fields }, 1 Values(1 rep values), 2 Wild });
codec_struct!(Arm { 1 one pattern, 2 one body, 3 meta span });
codec_enum!(Observe {
    0 Alert { 1 one cond, 2 one message, 3 opt confirm },
    1 Log(1 one f),
    2 Measure { 1 one name, 2 one value },
    3 Verify { 1 one cond, 2 one message, 3 opt req },
    4 Verdict { 1 one pass, 2 opt message },
});
codec_enum!(Method { 0 Step, 1 Reset, 2 Block(1 one f), 3 Push, 4 Insert, 5 Remove, 6 Clear, 7 Append });
codec_enum!(StmtKind {
    0 Assign { 1 one target, 2 one value },
    1 Check { 1 one cond, 2 opt message, 3 opt confirm, 4 opt target, 5 opt req, 6 one kind, 7 opt within },
    2 Goto(1 one t),
    3 Abort { 1 opt message },
    4 If { 1 one cond, 2 one then, 3 one otherwise },
    5 ForRange { 1 one var, 2 one count, 3 one body },
    6 ForEach { 1 one vars, 2 one iter, 3 one body },
    7 Match { 1 one subject, 2 rep arms },
    8 Return(1 one e),
    9 Send { 1 one stream, 2 one value, 3 one len_max },
    10 At { 1 one time, 2 one body },
    11 Cancel(1 one c),
    12 Raise(1 one s),
    13 Job { 1 one handle, 2 one native, 3 rep args },
    14 Every { 1 one period, 2 one counter, 3 one body },
    15 Break,
    16 Observe(1 one o),
    17 Arm { 1 one trigger, 2 one on },
    18 MethodCall { 1 opt target, 2 one receiver, 3 one method, 4 rep args },
    19 Pass,
    20 Skip(1 one s),
});

// ---------------------------------------------------------------- Funktionen

codec_struct!(CostVec { 1 one i32, 2 one i64, 3 one f32, 4 one f64, 5 one mem, 6 one call, 7 one native });
codec_enum!(GenericArgVal { 0 Unit(1 one u), 1 Type(1 one t), 2 Const(1 one c) });
codec_struct!(GenericOrigin { 1 one template, 2 rep args });
codec_struct!(FnParam { 1 one name, 2 one ty, 3 one inout, 4 opt default, 5 meta span });
codec_struct!(Fn { 1 one name, 2 rep params, 3 opt ret, 4 rep locals, 5 one body, 6 opt cost, 7 opt stack, 8 opt origin, 9 meta span });
codec_unit_enum!(NativeKind { 0 Fn, 1 Job });
codec_struct!(Native {
    1 one kind, 2 one name, 3 rep params, 4 one ret, 5 one cost, 6 one stack, 7 opt duration, 8 one total,
    9 opt from, 10 opt origin, 11 meta span,
});
codec_struct!(BlockDef { 1 one name, 2 rep params, 3 rep state_vars, 4 opt step, 5 rep methods, 6 opt origin, 7 meta span, 8 rep requires, 9 rep ensures });

// ---------------------------------------------------------------- Maschinen

codec_enum!(VarScope { 0 Machine, 1 State(1 one s), 2 Lifted(1 one s), 3 Local, 4 Param });
codec_struct!(VarDef { 1 one name, 2 one ty, 3 opt init, 4 one scope, 5 one public, 6 meta span });
codec_struct!(PersistVar { 1 one var, 2 opt min_interval, 3 one type_hash });
codec_struct!(SignalDef { 1 one name, 2 meta span });
codec_unit_enum!(ArithKind { 0 Overflow, 1 DivZero, 2 NonFinite, 3 Domain, 4 Singular });
codec_unit_enum!(RuntimeKind { 0 Overrun, 1 Driver, 2 Watchdog, 3 Hardware, 4 Node });
codec_enum!(FaultKind {
    0 CheckFailed, 1 Expect, 2 Timeout, 3 SensorFault, 4 MissingValue, 5 Arithmetic(1 one k), 6 Range,
    7 StreamOverflow, 8 Timing, 9 ScheduleOverflow, 10 JobOverflow, 11 Abort, 12 Runtime(1 one k),
});
codec_enum!(FaultTarget { 0 State(1 one s), 1 Faulted });
codec_enum!(Target { 0 State(1 one s), 1 Faulted, 2 Fault(1 one k) });
codec_enum!(Guard {
    0 Expr(1 one e),
    1 Match { 1 one subject, 2 one pattern, 3 opt binding, 4 one kind },
    2 Next { 1 one stream, 2 one binding },
});
codec_enum!(TransTrigger { 0 When(1 one g), 1 After(1 one d) });
codec_unit_enum!(TransKind { 0 Strong, 1 Weak });
codec_struct!(Transition { 1 one trigger, 2 one actions, 3 one target, 4 one kind, 5 meta span });
codec_struct!(Handler { 1 one stream, 2 opt pattern, 3 opt binding, 4 one body, 5 meta span, 6 opt guard });
codec_enum!(TimeoutAction { 0 Fault, 1 Goto(1 one t), 2 Else(1 one b) });
codec_struct!(Timeout { 1 one duration, 2 one action });
codec_enum!(SeqItem {
    0 Stmt(1 one s),
    1 Wait(1 one d),
    2 Until { 1 one guard, 2 opt timeout, 3 meta span },
    3 Expect { 1 one cond, 2 opt message, 3 meta span },
    4 Repeat { 1 one count, 2 one counter, 3 rep body, 4 meta span },
    5 Step { 1 one name, 2 rep body, 3 meta span },
});
codec_struct!(Sequence { 1 rep items, 2 one done, 3 meta span });
codec_struct!(ScopedInstance { 1 one machine, 2 one scope, 3 one resume, 4 meta span });
codec_struct!(State {
    1 one name, 2 opt parent, 3 rep children, 4 opt initial, 5 one idle, 6 one resume, 7 rep vars, 8 one enter,
    9 one exit, 10 one loop_block, 11 rep handlers, 12 rep transitions, 13 opt fault_target, 14 opt sequence,
    15 rep instances, 16 opt step_name, 17 meta meta, 18 meta span,
});
codec_struct!(FaultedState { 1 rep transitions, 2 meta span });
codec_struct!(Budget { 1 one activation, 2 one fault_path });
codec_struct!(Timer { 1 one state, 2 one width });
codec_struct!(CounterSite { 1 opt state, 2 meta span });
codec_struct!(BlockInstance { 1 one var, 2 one block, 3 one count });
codec_struct!(JobSlot { 1 one handle, 2 one native });
codec_struct!(Layout {
    1 rep timers, 2 rep every_counters, 3 rep viol_sites, 4 rep block_instances, 5 one jobs_max, 6 rep saved_paths,
    7 rep trigger_flags, 8 rep output_queues, 9 rep cursors, 10 opt scratch_bytes, 11 rep job_slots,
});
codec_struct!(InstanceInfo { 1 one template, 2 rep args, 3 opt array });
codec_enum!(MachineKind { 0 Regular, 1 Template, 2 Instance(1 one i), 3 Scenario });
codec_struct!(Machine {
    1 one name, 2 one kind, 3 one driver, 4 rep params, 5 one period, 6 one phase, 7 rep follows, 8 opt node,
    9 rep vars, 10 rep persist, 11 rep signals, 12 one fault_target, 13 rep states, 14 rep roots, 15 one initial,
    16 one loop_block, 17 rep handlers, 18 one faulted, 19 one layout, 20 opt budget, 21 meta meta, 22 meta span,
    23 opt declared_budget,
});

// ---------------------------------------------------------------- Programm

codec_unit_enum!(OutputTiming { 0 Asap, 1 Boundary });
codec_unit_enum!(OverrunPolicy { 0 Fault, 1 Alert });
codec_struct!(TickTolerance { 1 one pct, 2 one ticks });
codec_struct!(Config {
    1 one edition, 2 one tick, 3 one output_timing, 4 one fault_is_fail, 5 one float_width, 6 metaopt tick_source,
    7 opt tick_tolerance, 8 metaopt target, 9 rep tcb_allowlist, 10 meta overrun,
});
codec_struct!(DeclaredBudget { 1 opt ram, 2 meta span, 3 opt wcet_ns });
codec_struct!(Meta { 1 opt label, 2 opt display, 3 opt group, 4 opt doc });
codec_unit_enum!(Direction { 0 Input, 1 Output });
codec_enum!(Binding { 0 Hw(1 one a), 1 Sim(1 one a), 2 None });
codec_enum!(Framing { 0 Raw, 1 Lines, 2 Cobs, 3 LengthPrefixed(1 one w), 4 Fixed(1 one n) });
codec_unit_enum!(Overflow { 0 Fault, 1 DropOldest, 2 Drop });
codec_struct!(ChannelAttrs {
    1 opt safe, 2 opt max_age, 3 opt rate, 4 opt max_rate, 5 opt capacity, 6 opt framing, 7 opt overflow, 8 one wake,
    9 opt jitter, 10 opt max_slew, 11 opt debounce, 12 opt capacity_bytes, 13 opt expect_len, 14 one irreversible,
});
codec_struct!(Channel { 1 one dir, 2 one name, 3 one ty, 4 meta binding, 5 one attrs, 6 meta meta, 7 opt owner, 8 meta span });
codec_struct!(Stream {
    1 one name, 2 one elem, 3 one capacity, 4 opt capacity_bytes, 5 opt expect_len, 6 one overflow, 7 opt writer,
    8 rep readers, 9 meta span,
});
codec_struct!(Param { 1 one name, 2 one ty, 3 one default, 4 one tunable, 5 meta meta, 6 meta span });
codec_struct!(Profile { 1 one name, 2 rep assignments, 3 meta span });
codec_struct!(Command { 1 one name, 2 one wake, 3 meta meta, 4 meta span });
codec_struct!(Node { 1 one name, 2 one address, 3 opt tick, 4 meta span });
// `assumption` als `one`: Eigenschaften gab es vor M6 in keiner Datei.
codec_struct!(Property { 1 one name, 2 one formula, 3 one monitor, 4 meta span, 5 one assumption });
codec_enum!(Sweep {
    0 Range { 1 one param, 2 one from, 3 one to, 4 one step },
    1 List { 1 one param, 2 rep values },
});
codec_unit_enum!(StopOn { 0 Fail, 1 Never });
codec_struct!(Campaign { 1 one name, 2 opt program, 3 opt profile, 4 rep sweeps, 5 one repeat, 6 one stop_on, 7 meta span });
codec_struct!(Trigger { 1 one name, 2 opt node, 3 one guard, 4 one time, 5 one then, 6 one bound, 7 meta span, 8 opt owner, 9 one fired });
codec_struct!(Program {
    1 one config, 2 one types, 3 rep units, 4 rep enums, 5 rep records, 6 rep fns, 7 rep natives, 8 rep blocks,
    9 rep machines, 10 rep channels, 11 rep streams, 12 rep params, 13 rep profiles, 14 rep commands, 15 rep nodes,
    16 rep properties, 17 rep campaigns, 18 rep triggers,
});
