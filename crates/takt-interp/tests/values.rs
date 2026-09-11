//! Totale Arithmetik nach der Tabelle in Referenz 4.1 (mit 3.3, 3.10, 4.2),
//! Konversionen, Primitive, Funktionsaufrufe, Blockinstanzen, Formatierung.

use takt_diag::Span;
use takt_interp::env::Outer;
use takt_interp::{Ctx, Loaded, Mode, Out, Trap, Value, eval_const};
use takt_mir::expr::*;
use takt_mir::fns::{BlockDef, Fn, FnParam};
use takt_mir::machine::{ArithKind, FaultKind, VarDef, VarScope};
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::program::{Config, Program};
use takt_mir::stmt::*;
use takt_mir::types::*;
use takt_mir::*;

struct Rig {
    p: Program,
    t_int: TypeId,
    t_u8: TypeId,
    t_i32: TypeId,
    t_f64: TypeId,
    t_f32: TypeId,
    t_bool: TypeId,
    t_dur: TypeId,
    t_bar: TypeId,
    t_psi: TypeId,
    t_degc: TypeId,
    t_k: TypeId,
    t_opt: TypeId,
    t_table: TypeId,
    t_vec: TypeId,
    u_bar: UnitId,
    u_psi: UnitId,
    u_k: UnitId,
    u_s: UnitId,
    u_ms: UnitId,
}

fn var(v: VarId, ty: TypeId) -> Expr {
    Expr::new(ExprKind::Var(v), ty, Span::new(1, 2))
}

fn unit(name: &str, dim: [i8; 7], num: i64, den: u64, affine: Option<(i64, u64)>) -> UnitDef {
    UnitDef {
        name: name.into(),
        dimension: dim,
        factor: Rational { num, den },
        affine_offset: affine.map(|(n, d)| Rational { num: n, den: d }),
        predefined: true,
        span: Span::default(),
    }
}

impl Rig {
    fn new(width: FloatWidth) -> Self {
        let mut config = Config::new(1, 1_000_000);
        config.float_width = width;
        let mut p = Program::new(config);
        let pressure = [-1, 1, -2, 0, 0, 0, 0];
        p.units.push(unit("bar", pressure, 100_000, 1, None));
        p.units.push(unit("psi", pressure, 6_894_757_293_168, 1_000_000_000, None));
        p.units.push(unit("K", [0, 0, 0, 0, 1, 0, 0], 1, 1, None));
        p.units.push(unit("degC", [0, 0, 0, 0, 1, 0, 0], 1, 1, Some((27315, 100))));
        p.units.push(unit("s", [0, 0, 1, 0, 0, 0, 0], 1, 1, None));
        p.units.push(unit("ms", [0, 0, 1, 0, 0, 0, 0], 1, 1000, None));
        let (u_bar, u_psi, u_k, u_degc, u_s, u_ms) = (UnitId(0), UnitId(1), UnitId(2), UnitId(3), UnitId(4), UnitId(5));
        let t_int = p.types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
        let t_u8 = p.types.intern(Type::Int { width: IntWidth::U8, unit: None, range: None });
        let t_i32 = p.types.intern(Type::Int { width: IntWidth::I32, unit: None, range: None });
        let t_f64 = p.types.intern(Type::Float { width: FloatWidth::F64, unit: None, range: None });
        let t_f32 = p.types.intern(Type::Float { width: FloatWidth::F32, unit: None, range: None });
        let t_bool = p.types.intern(Type::Bool);
        let t_dur = p.types.intern(Type::Duration { range: None });
        let t_bar = p.types.intern(Type::Float { width, unit: Some(u_bar), range: None });
        let t_psi = p.types.intern(Type::Float { width, unit: Some(u_psi), range: None });
        let t_degc = p.types.intern(Type::Float { width, unit: Some(u_degc), range: None });
        let t_k = p.types.intern(Type::Float { width, unit: Some(u_k), range: None });
        let t_opt = p.types.intern(Type::Optional(t_int));
        let t_table = p.types.intern(Type::Table { key: t_f64, value: t_f64 });
        let t_vec = p.types.intern(Type::Vec { elem: t_int, cap: 2 });
        Rig {
            p,
            t_int,
            t_u8,
            t_i32,
            t_f64,
            t_f32,
            t_bool,
            t_dur,
            t_bar,
            t_psi,
            t_degc,
            t_k,
            t_opt,
            t_table,
            t_vec,
            u_bar,
            u_psi,
            u_k,
            u_s,
            u_ms,
        }
    }

    fn e(&self, kind: ExprKind, ty: TypeId) -> Expr {
        Expr::new(kind, ty, Span::new(1, 2))
    }

    fn int(&self, i: i64) -> Expr {
        self.e(ExprKind::Int(i), self.t_int)
    }

    fn f64(&self, x: f64) -> Expr {
        self.e(ExprKind::Float(x), self.t_f64)
    }

    fn f32(&self, x: f64) -> Expr {
        self.e(ExprKind::Float(x), self.t_f32)
    }

    fn dur(&self, ns: i64) -> Expr {
        self.e(ExprKind::Duration(ns), self.t_dur)
    }

    fn bin(&self, op: BinaryOp, a: Expr, b: Expr, ty: TypeId) -> Expr {
        self.e(ExprKind::Binary { op, lhs: Box::new(a), rhs: Box::new(b) }, ty)
    }

    fn call(&self, op: Intrinsic, args: Vec<Expr>, ty: TypeId) -> Expr {
        self.e(ExprKind::Intrinsic { op, args }, ty)
    }

    fn eval(&self, e: &Expr) -> Result<Value, Trap> {
        eval_const(&self.p, e)
    }

    fn fault(&self, e: &Expr) -> FaultKind {
        match self.eval(e) {
            Err(Trap::Fault(f)) => f.kind,
            other => panic!("Fault erwartet, {other:?} erhalten"),
        }
    }
}

#[test]
fn integer_arithmetic_is_total() {
    let r = Rig::new(FloatWidth::F64);
    assert_eq!(r.eval(&r.bin(BinaryOp::Add, r.int(2), r.int(3), r.t_int)), Ok(Value::Int(5)));
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Add, r.int(i64::MAX), r.int(1), r.t_int)),
        FaultKind::Arithmetic(ArithKind::Overflow)
    );
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Mul, r.int(i64::MIN), r.int(-1), r.t_int)),
        FaultKind::Arithmetic(ArithKind::Overflow)
    );
    assert_eq!(r.fault(&r.bin(BinaryOp::Div, r.int(1), r.int(0), r.t_int)), FaultKind::Arithmetic(ArithKind::DivZero));
    assert_eq!(r.fault(&r.bin(BinaryOp::Rem, r.int(1), r.int(0), r.t_int)), FaultKind::Arithmetic(ArithKind::DivZero));
    assert_eq!(r.eval(&r.bin(BinaryOp::Div, r.int(7), r.int(-2), r.t_int)), Ok(Value::Int(-3)), "trunkiert gegen 0");
    assert_eq!(
        r.eval(&r.bin(BinaryOp::Rem, r.int(-7), r.int(2), r.t_int)),
        Ok(Value::Int(-1)),
        "Vorzeichen des Dividenden"
    );
    // schmale Breiten faulten an ihrer Grenze (3.10)
    let u8 = |i| r.e(ExprKind::Int(i), r.t_u8);
    assert_eq!(r.eval(&r.bin(BinaryOp::Add, u8(200), u8(55), r.t_u8)), Ok(Value::UInt(255)));
    assert_eq!(r.fault(&r.bin(BinaryOp::Add, u8(200), u8(100), r.t_u8)), FaultKind::Arithmetic(ArithKind::Overflow));
    assert_eq!(r.fault(&r.bin(BinaryOp::Sub, u8(0), u8(1), r.t_u8)), FaultKind::Arithmetic(ArithKind::Overflow));
    let i32 = |i| r.e(ExprKind::Int(i), r.t_i32);
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Add, i32(i64::from(i32::MAX)), i32(1), r.t_i32)),
        FaultKind::Arithmetic(ArithKind::Overflow)
    );
    // Vergleiche sind total
    assert_eq!(r.eval(&r.bin(BinaryOp::Lt, r.int(-1), r.int(1), r.t_bool)), Ok(Value::Bool(true)));
    assert_eq!(r.eval(&r.bin(BinaryOp::Eq, u8(7), u8(7), r.t_bool)), Ok(Value::Bool(true)));
}

#[test]
fn bit_operations_and_shifts() {
    let r = Rig::new(FloatWidth::F64);
    let u8 = |i| r.e(ExprKind::Int(i), r.t_u8);
    assert_eq!(r.eval(&r.bin(BinaryOp::Shl, u8(1), r.int(7), r.t_u8)), Ok(Value::UInt(128)));
    assert_eq!(
        r.eval(&r.bin(BinaryOp::Shl, u8(0xC0), r.int(1), r.t_u8)),
        Ok(Value::UInt(0x80)),
        "Bits ueber der Breite fallen weg"
    );
    assert_eq!(
        r.eval(&r.bin(BinaryOp::Shr, u8(0x80), r.int(1), r.t_u8)),
        Ok(Value::UInt(0x40)),
        "logisch auf unsigniert"
    );
    assert_eq!(
        r.eval(&r.bin(BinaryOp::Shr, r.int(-8), r.int(1), r.t_int)),
        Ok(Value::Int(-4)),
        "arithmetisch auf signiert"
    );
    assert_eq!(r.fault(&r.bin(BinaryOp::Shl, r.int(1), r.int(64), r.t_int)), FaultKind::Range);
    assert_eq!(r.fault(&r.bin(BinaryOp::Shr, u8(1), r.int(8), r.t_u8)), FaultKind::Range);
    assert_eq!(r.eval(&r.bin(BinaryOp::BitAnd, r.int(0x1234), r.int(0xFF), r.t_int)), Ok(Value::Int(0x34)));
    assert_eq!(r.eval(&r.bin(BinaryOp::BitXor, u8(0xF0), u8(0xFF), r.t_u8)), Ok(Value::UInt(0x0F)));
    let not = r.e(ExprKind::Unary { op: UnaryOp::BitNot, expr: Box::new(u8(0)) }, r.t_u8);
    assert_eq!(r.eval(&not), Ok(Value::UInt(255)));
    let acc = |a: Accessor, args: Vec<Expr>| {
        r.e(ExprKind::Accessor { base: Box::new(u8(0b1010_0110)), accessor: a, args }, r.t_int)
    };
    assert_eq!(r.eval(&acc(Accessor::Bit, vec![r.int(1)])), Ok(Value::Bool(true)));
    assert_eq!(r.eval(&acc(Accessor::Bits, vec![r.int(7), r.int(4)])), Ok(Value::Int(0b1010)));
    assert_eq!(
        r.eval(&acc(Accessor::WithBit, vec![r.int(0), r.e(ExprKind::Bool(true), r.t_bool)])),
        Ok(Value::UInt(0b1010_0111))
    );
    let wrap = r.e(
        ExprKind::Accessor { base: Box::new(r.int(300)), accessor: Accessor::Wrap(IntWidth::U8), args: vec![] },
        r.t_u8,
    );
    assert_eq!(r.eval(&wrap), Ok(Value::UInt(44)));
    assert_eq!(r.eval(&r.call(Intrinsic::Rotl, vec![u8(0x81), r.int(1)], r.t_u8)), Ok(Value::UInt(0x03)));
    assert_eq!(r.eval(&r.call(Intrinsic::Rotr, vec![u8(0x81), r.int(1)], r.t_u8)), Ok(Value::UInt(0xC0)));
    assert_eq!(r.eval(&r.call(Intrinsic::WrappingAdd, vec![u8(250), u8(10)], r.t_u8)), Ok(Value::UInt(4)));
    assert_eq!(r.eval(&r.call(Intrinsic::SaturatingAdd, vec![u8(250), u8(10)], r.t_u8)), Ok(Value::UInt(255)));
    assert_eq!(r.eval(&r.call(Intrinsic::SaturatingSub, vec![u8(3), u8(10)], r.t_u8)), Ok(Value::UInt(0)));
    assert_eq!(r.eval(&r.call(Intrinsic::WrappingMul, vec![r.int(i64::MAX), r.int(2)], r.t_int)), Ok(Value::Int(-2)));
}

#[test]
fn float_arithmetic_has_no_nan_or_inf() {
    let r = Rig::new(FloatWidth::F64);
    assert_eq!(r.eval(&r.bin(BinaryOp::Add, r.f64(0.1), r.f64(0.2), r.t_f64)), Ok(Value::F64(0.1 + 0.2)));
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Mul, r.f64(1e308), r.f64(10.0), r.t_f64)),
        FaultKind::Arithmetic(ArithKind::NonFinite)
    );
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Div, r.f64(1.0), r.f64(0.0), r.t_f64)),
        FaultKind::Arithmetic(ArithKind::NonFinite)
    );
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Div, r.f64(0.0), r.f64(0.0), r.t_f64)),
        FaultKind::Arithmetic(ArithKind::NonFinite)
    );
    assert_eq!(r.fault(&r.call(Intrinsic::Sqrt, vec![r.f64(-1.0)], r.t_f64)), FaultKind::Arithmetic(ArithKind::Domain));
    assert_eq!(r.fault(&r.call(Intrinsic::Log, vec![r.f64(0.0)], r.t_f64)), FaultKind::Arithmetic(ArithKind::Domain));
    assert_eq!(r.fault(&r.call(Intrinsic::Asin, vec![r.f64(2.0)], r.t_f64)), FaultKind::Arithmetic(ArithKind::Domain));
    // `pow` und `exp` sind noch nicht kuratiert (13.8): Ihr Fault entsteht
    // erst, wenn `libtaktm` sie rechnet. Die Domaenenwaechter von `log`,
    // `asin` und `sqrt` stehen davor und wirken schon heute.
    assert!(
        matches!(r.eval(&r.call(Intrinsic::Pow, vec![r.f64(-8.0), r.f64(0.5)], r.t_f64)), Err(Trap::Bug(_))),
        "`pow` meldet, dass es nicht kuratiert ist"
    );
    assert!(matches!(r.eval(&r.call(Intrinsic::Exp, vec![r.f64(1000.0)], r.t_f64)), Err(Trap::Bug(_))), "`exp` ebenso");
    assert_eq!(r.eval(&r.call(Intrinsic::Sqrt, vec![r.f64(16.0)], r.t_f64)), Ok(Value::F64(4.0)));
    assert_eq!(r.eval(&r.call(Intrinsic::Fma, vec![r.f64(2.0), r.f64(3.0), r.f64(1.0)], r.t_f64)), Ok(Value::F64(7.0)));
    assert_eq!(r.eval(&r.call(Intrinsic::Abs, vec![r.f64(-2.5)], r.t_f64)), Ok(Value::F64(2.5)));
    assert_eq!(r.eval(&r.call(Intrinsic::Max, vec![r.f64(-2.5), r.f64(1.0)], r.t_f64)), Ok(Value::F64(1.0)));
    assert_eq!(r.eval(&r.call(Intrinsic::Min, vec![r.int(-2), r.int(1)], r.t_int)), Ok(Value::Int(-2)));
    // f32 rechnet nativ in f32 (4.2), kein Doppelrunden
    let sum32 = r.bin(BinaryOp::Add, r.f32(0.1), r.f32(0.2), r.t_f32);
    assert_eq!(r.eval(&sum32), Ok(Value::F32(0.1f32 + 0.2f32)));
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Mul, r.f32(1e38), r.f32(10.0), r.t_f32)),
        FaultKind::Arithmetic(ArithKind::NonFinite)
    );
    assert_eq!(r.eval(&r.call(Intrinsic::Sqrt, vec![r.f32(2.0)], r.t_f32)), Ok(Value::F32(2.0f32.sqrt())));
    // float -> int nur ueber round/floor/ceil mit Range-Pruefung (4.1)
    assert_eq!(r.eval(&r.call(Intrinsic::Round, vec![r.f64(2.5)], r.t_int)), Ok(Value::Int(3)));
    assert_eq!(r.eval(&r.call(Intrinsic::Floor, vec![r.f64(-2.5)], r.t_int)), Ok(Value::Int(-3)));
    assert_eq!(r.eval(&r.call(Intrinsic::Ceil, vec![r.f64(2.1)], r.t_int)), Ok(Value::Int(3)));
    assert_eq!(r.fault(&r.call(Intrinsic::Round, vec![r.f64(1e19)], r.t_int)), FaultKind::Range);
}

#[test]
fn casts_are_range_checked() {
    let r = Rig::new(FloatWidth::F64);
    let cast = |e: Expr, to: TypeId| r.e(ExprKind::Cast { expr: Box::new(e), to }, to);
    assert_eq!(r.eval(&cast(r.int(255), r.t_u8)), Ok(Value::UInt(255)));
    assert_eq!(r.fault(&cast(r.int(256), r.t_u8)), FaultKind::Range);
    assert_eq!(r.fault(&cast(r.int(-1), r.t_u8)), FaultKind::Range);
    assert_eq!(r.eval(&cast(r.e(ExprKind::Int(7), r.t_u8), r.t_int)), Ok(Value::Int(7)), "aufwaerts frei");
    assert_eq!(r.eval(&cast(r.int(5), r.t_f64)), Ok(Value::F64(5.0)));
    assert_eq!(
        r.eval(&cast(r.int(1 << 53 | 1), r.t_f64)),
        Ok(Value::F64((1i64 << 53) as f64)),
        "gerundet jenseits 2^53"
    );
    assert_eq!(r.fault(&cast(r.f64(1e300), r.t_f32)), FaultKind::Arithmetic(ArithKind::NonFinite));
}

#[test]
fn durations_follow_3_3() {
    let r = Rig::new(FloatWidth::F64);
    assert_eq!(
        r.eval(&r.bin(BinaryOp::Add, r.dur(1_000_000_000), r.dur(500_000_000), r.t_dur)),
        Ok(Value::Duration(1_500_000_000))
    );
    assert_eq!(r.eval(&r.bin(BinaryOp::Mul, r.dur(1_000), r.int(3), r.t_dur)), Ok(Value::Duration(3_000)));
    assert_eq!(r.eval(&r.bin(BinaryOp::Mul, r.int(3), r.dur(1_000), r.t_dur)), Ok(Value::Duration(3_000)));
    assert_eq!(r.eval(&r.bin(BinaryOp::Div, r.dur(3_000), r.int(2), r.t_dur)), Ok(Value::Duration(1_500)));
    assert_eq!(r.eval(&r.bin(BinaryOp::Div, r.dur(-3), r.int(2), r.t_dur)), Ok(Value::Duration(-1)), "trunkiert");
    assert_eq!(r.eval(&r.bin(BinaryOp::Div, r.dur(3_000), r.dur(1_000), r.t_int)), Ok(Value::Int(3)));
    assert_eq!(r.fault(&r.bin(BinaryOp::Div, r.dur(1), r.int(0), r.t_dur)), FaultKind::Arithmetic(ArithKind::DivZero));
    assert_eq!(
        r.fault(&r.bin(BinaryOp::Add, r.dur(i64::MAX), r.dur(1), r.t_dur)),
        FaultKind::Arithmetic(ArithKind::Overflow)
    );
    assert_eq!(r.eval(&r.bin(BinaryOp::Lt, r.dur(1), r.dur(2), r.t_bool)), Ok(Value::Bool(true)));
    // d.as(U) = round(d) / round(ns je U), korrekt gerundeter Quotient
    let as_s =
        r.e(ExprKind::Convert { expr: Box::new(r.dur(10_000_000)), kind: ConvertKind::As, unit: r.u_s }, r.t_f64);
    assert_eq!(r.eval(&as_s), Ok(Value::F64(0.01)));
    let as_ms =
        r.e(ExprKind::Convert { expr: Box::new(r.dur(1_500_000)), kind: ConvertKind::As, unit: r.u_ms }, r.t_f64);
    assert_eq!(r.eval(&as_ms), Ok(Value::F64(1.5)));
    let r32 = Rig::new(FloatWidth::F32);
    let as_s32 = r32
        .e(ExprKind::Convert { expr: Box::new(r32.dur(10_000_000)), kind: ConvertKind::As, unit: r32.u_s }, r32.t_f32);
    assert_eq!(r32.eval(&as_s32), Ok(Value::F32(10_000_000f32 / 1e9f32)));
}

#[test]
fn unit_conversions_use_rational_factors() {
    let r = Rig::new(FloatWidth::F64);
    let one_bar = r.e(ExprKind::Float(1.0), r.t_bar);
    let to_psi = r.e(ExprKind::Convert { expr: Box::new(one_bar), kind: ConvertKind::To, unit: r.u_psi }, r.t_psi);
    let Ok(Value::F64(psi)) = r.eval(&to_psi) else { panic!() };
    assert!((psi - 14.503_773_8).abs() < 1e-6, "{psi}");
    let twenty = r.e(ExprKind::Float(20.0), r.t_degc);
    let to_k = r.e(ExprKind::Convert { expr: Box::new(twenty), kind: ConvertKind::To, unit: r.u_k }, r.t_k);
    assert_eq!(r.eval(&to_k), Ok(Value::F64(20.0 + 273.15)));
    let k = r.e(ExprKind::Float(300.0), r.t_k);
    let back = r.e(ExprKind::Convert { expr: Box::new(k), kind: ConvertKind::To, unit: UnitId(3) }, r.t_degc);
    assert_eq!(r.eval(&back), Ok(Value::F64(300.0 - 273.15)));
    let same = r.e(
        ExprKind::Convert { expr: Box::new(r.e(ExprKind::Float(2.5), r.t_bar)), kind: ConvertKind::To, unit: r.u_bar },
        r.t_bar,
    );
    assert_eq!(r.eval(&same), Ok(Value::F64(2.5)));
}

#[test]
fn checked_nodes_fault_by_kind() {
    let r = Rig::new(FloatWidth::F64);
    let range = Range { lo: Const::Int(0), hi: Const::Int(10), origin: RangeOrigin::Declared };
    let ok = r.e(ExprKind::Checked { expr: Box::new(r.int(10)), kind: CheckedKind::Range(range) }, r.t_int);
    assert_eq!(r.eval(&ok), Ok(Value::Int(10)));
    let bad = r.e(ExprKind::Checked { expr: Box::new(r.int(11)), kind: CheckedKind::Range(range) }, r.t_int);
    assert_eq!(r.fault(&bad), FaultKind::Range);
    let none = r.e(ExprKind::None, r.t_opt);
    let missing = r.e(ExprKind::Checked { expr: Box::new(none), kind: CheckedKind::Missing }, r.t_int);
    assert_eq!(r.fault(&missing), FaultKind::MissingValue);
    let some = r.e(ExprKind::Lift(Box::new(r.int(4))), r.t_opt);
    let present = r.e(ExprKind::Checked { expr: Box::new(some.clone()), kind: CheckedKind::Missing }, r.t_int);
    assert_eq!(r.eval(&present), Ok(Value::Int(4)));
    let or = r.e(
        ExprKind::Accessor {
            base: Box::new(r.e(ExprKind::None, r.t_opt)),
            accessor: Accessor::Or,
            args: vec![r.int(9)],
        },
        r.t_int,
    );
    assert_eq!(r.eval(&or), Ok(Value::Int(9)));
    let valid = r.e(ExprKind::Accessor { base: Box::new(some), accessor: Accessor::Valid, args: vec![] }, r.t_bool);
    assert_eq!(r.eval(&valid), Ok(Value::Bool(true)));
    // Index ausserhalb faultet aus der Operation heraus
    let arr = r.e(
        ExprKind::Array(vec![r.int(1), r.int(2)]),
        r.p.types.list.iter().position(|t| matches!(t, Type::Int { .. })).map(|i| TypeId(i as u32)).unwrap(),
    );
    let idx = r.e(ExprKind::Index { base: Box::new(arr.clone()), index: Box::new(r.int(2)) }, r.t_int);
    assert_eq!(r.fault(&idx), FaultKind::Range);
    let idx_ok = r.e(ExprKind::Index { base: Box::new(arr), index: Box::new(r.int(1)) }, r.t_int);
    assert_eq!(r.eval(&idx_ok), Ok(Value::Int(2)));
}

#[test]
fn interp_is_piecewise_linear_and_clamped() {
    let r = Rig::new(FloatWidth::F64);
    let point = |x: f64, y: f64| r.e(ExprKind::Tuple(Box::new(r.f64(x)), Box::new(r.f64(y))), r.t_table);
    let table = r.e(ExprKind::Array(vec![point(0.0, 0.0), point(10.0, 100.0), point(20.0, 0.0)]), r.t_table);
    let at = |x: f64| r.call(Intrinsic::Interp, vec![table.clone(), r.f64(x)], r.t_f64);
    assert_eq!(r.eval(&at(5.0)), Ok(Value::F64(50.0)));
    assert_eq!(r.eval(&at(15.0)), Ok(Value::F64(50.0)));
    assert_eq!(r.eval(&at(-3.0)), Ok(Value::F64(0.0)), "geklemmt");
    assert_eq!(r.eval(&at(99.0)), Ok(Value::F64(0.0)), "geklemmt");
    assert_eq!(r.eval(&at(10.0)), Ok(Value::F64(100.0)));
}

/// `fn sq(x: int) -> int: return x * x` und eine Funktion mit Schleife und `break`.
fn with_fns(r: &mut Rig) -> (FnId, FnId) {
    let x = r.e(ExprKind::Var(VarId(0)), r.t_int);
    let sq = Fn {
        name: "sq".into(),
        params: vec![FnParam { name: "x".into(), ty: r.t_int, inout: false, default: None, span: Span::default() }],
        ret: Some(r.t_int),
        locals: vec![VarDef {
            name: "x".into(),
            ty: r.t_int,
            init: None,
            scope: VarScope::Param,
            public: false,
            span: Span::default(),
        }],
        body: Block::new(vec![Stmt::new(
            StmtKind::Return(r.bin(BinaryOp::Mul, x.clone(), x, r.t_int)),
            Span::default(),
        )]),
        cost: None,
        stack: None,
        origin: None,
        span: Span::default(),
    };
    r.p.fns.push(sq);
    // fn sum_to(n): var s = 0; for i in range(n): if i == 5: break; s = s + sq(i); return s
    let (n, s, i) = (VarId(0), VarId(1), VarId(2));
    let body = Block::new(vec![
        Stmt::new(
            StmtKind::ForRange {
                var: i,
                count: var(n, r.t_int),
                body: Block::new(vec![
                    Stmt::new(
                        StmtKind::If {
                            cond: r.bin(BinaryOp::Eq, var(i, r.t_int), r.int(5), r.t_bool),
                            then: Block::new(vec![Stmt::new(StmtKind::Break, Span::default())]),
                            otherwise: Block::default(),
                        },
                        Span::default(),
                    ),
                    Stmt::new(
                        StmtKind::Assign {
                            target: Place::Var(s),
                            value: r.bin(
                                BinaryOp::Add,
                                var(s, r.t_int),
                                r.e(ExprKind::Call { callee: FnId(0), args: vec![var(i, r.t_int)] }, r.t_int),
                                r.t_int,
                            ),
                        },
                        Span::default(),
                    ),
                ]),
            },
            Span::default(),
        ),
        Stmt::new(StmtKind::Return(var(s, r.t_int)), Span::default()),
    ]);
    let sum_to = Fn {
        name: "sum_to".into(),
        params: vec![FnParam { name: "n".into(), ty: r.t_int, inout: false, default: None, span: Span::default() }],
        ret: Some(r.t_int),
        locals: vec![
            VarDef {
                name: "n".into(),
                ty: r.t_int,
                init: None,
                scope: VarScope::Param,
                public: false,
                span: Span::default(),
            },
            VarDef {
                name: "s".into(),
                ty: r.t_int,
                init: Some(r.int(0)),
                scope: VarScope::Local,
                public: false,
                span: Span::default(),
            },
            VarDef {
                name: "i".into(),
                ty: r.t_int,
                init: None,
                scope: VarScope::Local,
                public: false,
                span: Span::default(),
            },
        ],
        body,
        cost: None,
        stack: None,
        origin: None,
        span: Span::default(),
    };
    r.p.fns.push(sum_to);
    (FnId(0), FnId(1))
}

#[test]
fn functions_run_with_frames_loops_and_break() {
    let mut r = Rig::new(FloatWidth::F64);
    let (sq, sum_to) = with_fns(&mut r);
    let call = r.e(ExprKind::Call { callee: sq, args: vec![r.int(12)] }, r.t_int);
    assert_eq!(r.eval(&call), Ok(Value::Int(144)));
    let call = r.e(ExprKind::Call { callee: sum_to, args: vec![r.int(100)] }, r.t_int);
    assert_eq!(r.eval(&call), Ok(Value::Int(30)));
    let call = r.e(ExprKind::Call { callee: sum_to, args: vec![r.int(3)] }, r.t_int);
    assert_eq!(r.eval(&call), Ok(Value::Int(5)));
    // ein Fault in der Tiefe kommt als Fault heraus, nicht als Panik
    let boom = r.e(ExprKind::Call { callee: sq, args: vec![r.int(i64::MAX)] }, r.t_int);
    assert_eq!(r.fault(&boom), FaultKind::Arithmetic(ArithKind::Overflow));
}

/// Umgebung mit Maschinenvariablen fuer Blockinstanzen und Anweisungen.
struct Vars {
    values: Vec<Value>,
    types: Vec<TypeId>,
}

impl Outer for Vars {
    fn var(&self, v: VarId) -> Result<&Value, Trap> {
        Ok(&self.values[v.index()])
    }
    fn var_mut(&mut self, v: VarId) -> Result<&mut Value, Trap> {
        Ok(&mut self.values[v.index()])
    }
    fn var_type(&self, v: VarId) -> Result<TypeId, Trap> {
        Ok(self.types[v.index()])
    }
}

#[test]
fn block_instances_keep_state_between_steps() {
    let mut r = Rig::new(FloatWidth::F64);
    // block acc(gain: int): var total: int = gain; step(x: int) -> int: total = total + x; return total
    // VarId-Raum der Methode: 0 gain, 1 total, 2 x
    let step = Fn {
        name: "acc.step".into(),
        params: vec![FnParam { name: "x".into(), ty: r.t_int, inout: false, default: None, span: Span::default() }],
        ret: Some(r.t_int),
        locals: vec![VarDef {
            name: "x".into(),
            ty: r.t_int,
            init: None,
            scope: VarScope::Param,
            public: false,
            span: Span::default(),
        }],
        body: Block::new(vec![
            Stmt::new(
                StmtKind::Assign {
                    target: Place::Var(VarId(1)),
                    value: r.bin(BinaryOp::Add, var(VarId(1), r.t_int), var(VarId(2), r.t_int), r.t_int),
                },
                Span::default(),
            ),
            Stmt::new(StmtKind::Return(var(VarId(1), r.t_int)), Span::default()),
        ]),
        cost: None,
        stack: None,
        origin: None,
        span: Span::default(),
    };
    r.p.fns.push(step);
    r.p.blocks.push(BlockDef {
        name: "acc".into(),
        params: vec![FnParam { name: "gain".into(), ty: r.t_int, inout: false, default: None, span: Span::default() }],
        state_vars: vec![VarDef {
            name: "total".into(),
            ty: r.t_int,
            init: Some(var(VarId(0), r.t_int)),
            scope: VarScope::Local,
            public: false,
            span: Span::default(),
        }],
        step: Some(FnId(0)),
        methods: vec![],
        origin: None,
        span: Span::default(),
    });
    let init = r.e(ExprKind::BlockInit { block: BlockId(0), args: vec![r.int(10)], count: None }, r.t_int);
    let instance = r.eval(&init).expect("Instanz");
    let mut env = Vars { values: vec![instance, Value::Int(0)], types: vec![r.t_int, r.t_int] };
    let loaded = Loaded::borrow(&r.p);
    let mut ctx = Ctx::new(&loaded, &mut env, 0);
    let step = |x: i64| {
        Stmt::new(
            StmtKind::MethodCall {
                target: Some(Place::Var(VarId(1))),
                receiver: Place::Var(VarId(0)),
                method: Method::Step,
                args: vec![r.int(x)],
            },
            Span::default(),
        )
    };
    assert_eq!(ctx.exec(&step(5), Mode::Run), Ok(Out::Normal));
    assert_eq!(ctx.outer.var(VarId(1)), Ok(&Value::Int(15)));
    // step zweimal in einer Aktivierung ist ein Fehler des Verifiers (SC-11), kein Fault
    assert!(matches!(ctx.exec(&step(1), Mode::Run), Err(Trap::Bug(_))));
    if let Value::Block(b) = ctx.outer.var_mut(VarId(0)).unwrap() {
        b.stepped = false;
    }
    assert_eq!(ctx.exec(&step(7), Mode::Run), Ok(Out::Normal));
    assert_eq!(ctx.outer.var(VarId(1)), Ok(&Value::Int(22)));
    let reset = Stmt::new(
        StmtKind::MethodCall { target: None, receiver: Place::Var(VarId(0)), method: Method::Reset, args: vec![] },
        Span::default(),
    );
    assert_eq!(ctx.exec(&reset, Mode::Run), Ok(Out::Normal));
    if let Value::Block(b) = ctx.outer.var_mut(VarId(0)).unwrap() {
        assert_eq!(b.vars, vec![Value::Int(10), Value::Int(10)]);
        assert!(!b.stepped);
    }
}

#[test]
fn vec_push_respects_capacity_and_match_binds_fields() {
    let mut r = Rig::new(FloatWidth::F64);
    let t_vec = r.t_vec;
    let mut env = Vars {
        values: vec![Value::Vec(vec![]), Value::Bool(false), Value::Int(0)],
        types: vec![t_vec, r.t_bool, r.t_int],
    };
    let loaded = Loaded::borrow(&r.p);
    let mut ctx = Ctx::new(&loaded, &mut env, 0);
    let push = |x: i64| {
        Stmt::new(
            StmtKind::MethodCall {
                target: Some(Place::Var(VarId(1))),
                receiver: Place::Var(VarId(0)),
                method: Method::Push,
                args: vec![r.int(x)],
            },
            Span::default(),
        )
    };
    ctx.exec(&push(1), Mode::Run).unwrap();
    ctx.exec(&push(2), Mode::Run).unwrap();
    assert_eq!(ctx.outer.var(VarId(1)), Ok(&Value::Bool(true)));
    ctx.exec(&push(3), Mode::Run).unwrap();
    assert_eq!(ctx.outer.var(VarId(1)), Ok(&Value::Bool(false)), "voll: false, kein Fault");
    assert_eq!(ctx.outer.var(VarId(0)), Ok(&Value::Vec(vec![Value::Int(1), Value::Int(2)])));
    // match ueber T!E mit Bindung (Varianten 0 = OK, 1 = ERR)
    r.p.enums.push(EnumDef {
        name: "E".into(),
        variants: vec![VariantDef { name: "MAGIC".into(), discriminant: 0, fields: vec![], span: Span::default() }],
        layout: None,
        open: false,
        builtin: false,
        span: Span::default(),
    });
    let t_res = r.p.types.intern(Type::Result { ok: r.t_int, err: EnumId(0) });
    let subject = r.e(ExprKind::Ok(Box::new(r.int(41))), t_res);
    let m = Stmt::new(
        StmtKind::Match {
            subject,
            arms: vec![
                Arm {
                    pattern: ArmPattern::Variant { variant: 0, fields: vec![VarId(2)] },
                    body: Block::new(vec![Stmt::new(
                        StmtKind::Assign {
                            target: Place::Var(VarId(2)),
                            value: r.bin(BinaryOp::Add, r.e(ExprKind::Var(VarId(2)), r.t_int), r.int(1), r.t_int),
                        },
                        Span::default(),
                    )]),
                    span: Span::default(),
                },
                Arm { pattern: ArmPattern::Wild, body: Block::default(), span: Span::default() },
            ],
        },
        Span::default(),
    );
    let loaded = Loaded::borrow(&r.p);
    let mut ctx = Ctx::new(&loaded, &mut env, 0);
    assert_eq!(ctx.exec(&m, Mode::Run), Ok(Out::Normal));
    assert_eq!(ctx.outer.var(VarId(2)), Ok(&Value::Int(42)));
}

#[test]
fn formatting_truncates_and_marks_invalid() {
    let r = Rig::new(FloatWidth::F64);
    let mut env = takt_interp::ConstEnv::new(1_000_000);
    let loaded = Loaded::borrow(&r.p);
    let mut ctx = Ctx::new(&loaded, &mut env, 0);
    let f = Format {
        pieces: vec![
            FormatPiece::Text("p=".into()),
            FormatPiece::Expr { expr: r.f64(2.0 / 3.0), spec: Some(".3".into()) },
            FormatPiece::Text(" c=".into()),
            FormatPiece::Expr { expr: r.int(255), spec: Some("hex".into()) },
            FormatPiece::Text(" n=".into()),
            FormatPiece::Expr { expr: r.int(7), spec: Some("04".into()) },
            FormatPiece::Text(" d=".into()),
            FormatPiece::Expr { expr: r.dur(1_500_000), spec: None },
            FormatPiece::Text(" b=".into()),
            FormatPiece::Expr { expr: r.e(ExprKind::Bool(true), r.t_bool), spec: None },
            FormatPiece::Text(" x=".into()),
            FormatPiece::Expr { expr: r.bin(BinaryOp::Div, r.int(1), r.int(0), r.t_int), spec: None },
        ],
        len_max: 64,
    };
    assert_eq!(takt_interp::format::render(&f, &mut ctx), "p=0.667 c=ff n=0007 d=1500 us b=true x=<invalid>");
    let short = Format { pieces: vec![FormatPiece::Text("äöü".into())], len_max: 3 };
    assert_eq!(takt_interp::format::render(&short, &mut ctx), "ä", "Kuerzung an der Zeichengrenze");
    let plain = Format { pieces: vec![FormatPiece::Expr { expr: r.f64(3.0), spec: None }], len_max: 8 };
    assert_eq!(takt_interp::format::render(&plain, &mut ctx), "3.0");
}
