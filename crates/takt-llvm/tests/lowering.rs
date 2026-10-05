//! Typen und Ausdruecke nach LLVM-IR (11.2).

use takt_llvm::emit::Module;
use takt_llvm::expr::{Lowered, Vars, lower};
use takt_llvm::ty::{self, LlvmType};
use takt_mir::expr::{BinaryOp, Expr, ExprKind, UnaryOp};
use takt_mir::program::{Config, Program};
use takt_mir::types::{FloatWidth, IntWidth, Range, RangeOrigin, Type};
use takt_mir::{TypeId, VarId};

/// Ein Programm mit den Typen, die die Tests brauchen.
fn program() -> (Program, Types) {
    let mut p = Program::new(Config::new(1, 1_000_000));
    let t = Types {
        bool_: push(&mut p, Type::Bool),
        i32_: push(&mut p, Type::Int { width: IntWidth::I32, unit: None, range: None }),
        u32_: push(&mut p, Type::Int { width: IntWidth::U32, unit: None, range: None }),
        i64_: push(&mut p, Type::Int { width: IntWidth::I64, unit: None, range: None }),
        f64_: push(&mut p, Type::Float { width: FloatWidth::F64, unit: None, range: None }),
        f32_: push(&mut p, Type::Float { width: FloatWidth::F32, unit: None, range: None }),
        dur: push(&mut p, Type::Duration { range: None }),
    };
    (p, t)
}

struct Types {
    bool_: TypeId,
    i32_: TypeId,
    u32_: TypeId,
    i64_: TypeId,
    f64_: TypeId,
    f32_: TypeId,
    dur: TypeId,
}

fn push(p: &mut Program, ty: Type) -> TypeId {
    p.types.list.push(ty);
    TypeId(p.types.list.len() as u32 - 1)
}

fn e(kind: ExprKind, ty: TypeId) -> Expr {
    Expr::new(kind, ty, takt_diag::Span::new(0, 0))
}

/// Variablen kennt Schritt 6 noch nicht; die Tests reichen sie durch.
struct NoVars;

impl Vars for NoVars {
    fn var(&self, _: VarId, _: &mut Module) -> Option<Lowered> {
        None
    }
}

/// Senkt einen Ausdruck und liefert die erzeugten Zeilen.
fn lines_of(expr: &Expr, p: &Program) -> Vec<String> {
    let mut m = Module::new("t", "x86_64-unknown-linux-gnu");
    let f64_ = LlvmType::F64;
    m.begin("f", &f64_, &[]);
    let _ = lower(expr, p, &mut m, &NoVars);
    m.end(None);
    m.finish().lines().filter(|l| l.trim_start().starts_with('%')).map(|l| l.trim().to_string()).collect()
}

// --- Typen (11.2) -------------------------------------------------------

#[test]
fn scalar_types_map_to_their_llvm_widths() {
    let (p, t) = program();
    assert_eq!(ty::lower(t.bool_, &p), Some(LlvmType::Int(1)));
    assert_eq!(ty::lower(t.i32_, &p), Some(LlvmType::Int(32)));
    assert_eq!(ty::lower(t.i64_, &p), Some(LlvmType::Int(64)));
    assert_eq!(ty::lower(t.f64_, &p), Some(LlvmType::F64));
    assert_eq!(ty::lower(t.f32_, &p), Some(LlvmType::F32));
}

/// GEN-010: Die uebrigen Breiten und die Huellen um einen Wert, wie
/// Interpreter und Rahmen sie erwarten (11.2, 3.8, 3.9).
#[test]
fn every_width_and_wrapper_has_its_layout() {
    let (mut p, t) = program();
    for (w, bits) in [
        (IntWidth::I8, 8),
        (IntWidth::U8, 8),
        (IntWidth::I16, 16),
        (IntWidth::U16, 16),
        (IntWidth::U32, 32),
        (IntWidth::U64, 64),
    ] {
        let id = push(&mut p, Type::Int { width: w, unit: None, range: None });
        assert_eq!(ty::lower(id, &p), Some(LlvmType::Int(bits)), "{w:?}");
    }
    let bytes = |n| LlvmType::Array(Box::new(LlvmType::Int(8)), n);
    let opt = push(&mut p, Type::Optional(t.f64_));
    assert_eq!(
        ty::lower(opt, &p),
        Some(LlvmType::Struct(vec![LlvmType::F64, LlvmType::Int(1)])),
        "Wert vorn, Flag hinten"
    );
    let b = push(&mut p, Type::Bytes { cap: 12 });
    assert_eq!(ty::lower(b, &p), Some(LlvmType::Struct(vec![LlvmType::Int(32), bytes(12)])));
    let line = push(&mut p, Type::Line { cap: 12 });
    assert_eq!(ty::lower(line, &p), Some(LlvmType::Struct(vec![LlvmType::Int(32), bytes(12), LlvmType::Int(1)])));
    let variants = ["IDLE", "RUN"].into_iter().enumerate().map(|(i, name)| takt_mir::types::VariantDef {
        name: name.into(),
        discriminant: i as i64,
        fields: Vec::new(),
        span: takt_diag::Span::default(),
    });
    p.enums.push(takt_mir::types::EnumDef {
        name: "Mode".into(),
        variants: variants.collect(),
        layout: None,
        open: false,
        builtin: false,
        span: takt_diag::Span::default(),
    });
    let id = takt_mir::EnumId(p.enums.len() as u32 - 1);
    let mode = push(&mut p, Type::Enum(id));
    assert_eq!(ty::lower(mode, &p), Some(LlvmType::Int(32)), "ein Enum ohne Felder ist seine Diskriminante");
}

/// 3.2: Dauern sind Nanosekunden in `i64`, nicht ein eigener Typ.
#[test]
fn a_duration_is_an_i64_of_nanoseconds() {
    let (p, t) = program();
    assert_eq!(ty::lower(t.dur, &p), Some(LlvmType::Int(64)));
}

/// LLVM kennt kein Vorzeichen im Typ; `u32` und `i32` sind beide `i32`.
/// Der Unterschied steckt in der Operation.
#[test]
fn signedness_lives_in_the_operation_not_the_type() {
    let (p, t) = program();
    assert_eq!(ty::lower(t.u32_, &p), ty::lower(t.i32_, &p));
    assert!(ty::signed(IntWidth::I32));
    assert!(!ty::signed(IntWidth::U32));
}

#[test]
fn an_array_maps_to_an_llvm_array() {
    let (mut p, t) = program();
    let arr = push(&mut p, Type::Array { elem: t.f64_, len: 16 });
    assert_eq!(ty::lower(arr, &p), Some(LlvmType::Array(Box::new(LlvmType::F64), 16)));
}

/// 11.2: Grosse Werte gehen per Zeiger. Die Schwelle des Codegens ist
/// `INDIRECT_MIN` (zwei Worte, FB-214); ein Aggregat genau darauf geht als
/// Wert, eines darueber als `ptr` in die Signatur einer Funktion.
#[test]
fn the_pointer_threshold_follows_the_reference() {
    let (mut p, t) = program();
    let at = push(&mut p, Type::Array { elem: t.f64_, len: (ty::INDIRECT_MIN / 8) as u32 });
    let above = push(&mut p, Type::Array { elem: t.f64_, len: (ty::INDIRECT_MIN / 8) as u32 + 1 });
    let (at, above) = (ty::lower(at, &p).expect("Array"), ty::lower(above, &p).expect("Array"));
    assert_eq!(at.size(), ty::INDIRECT_MIN);
    assert!(!at.indirect() && above.indirect());
    let sig = |param: LlvmType| {
        takt_llvm::fns::Signature { declared: vec![param], ret: LlvmType::Int(1), sret: false }.declare("f")
    };
    assert_eq!(sig(at.clone()), format!("declare i1 @f({at})"));
    assert_eq!(sig(above), "declare i1 @f(ptr readonly)");
}

/// Ein Typ, den der Codegen noch nicht kennt, meldet sich als solcher —
/// statt still etwas Falsches zu erzeugen.
#[test]
fn an_unsupported_type_says_so() {
    let (mut p, t) = program();
    let table = push(&mut p, Type::Table { key: t.f64_, value: t.f64_ });
    assert_eq!(ty::lower(table, &p), None);
}

// --- Ausdruecke ---------------------------------------------------------

#[test]
fn float_arithmetic_uses_the_f_instructions() {
    let (p, t) = program();
    let sum = e(
        ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(e(ExprKind::Float(1.0), t.f64_)),
            rhs: Box::new(e(ExprKind::Float(2.0), t.f64_)),
        },
        t.f64_,
    );
    let lines = lines_of(&sum, &p);
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("fadd double"), "{}", lines[0]);
    assert!(lines[0].contains("0x3FF0000000000000"), "Literale als Bitmuster: {}", lines[0]);
}

/// 4.1: Ganzzahlueberlauf ist ein Fault, kein undefiniertes Verhalten.
/// `nsw`/`nuw` waeren hier falsch — sie erlaubten LLVM, die Pruefung
/// wegzuoptimieren, die den Ueberlauf fangen soll.
#[test]
fn integer_arithmetic_carries_no_wrap_flags() {
    let (p, t) = program();
    let sum = e(
        ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(e(ExprKind::Int(1), t.i32_)),
            rhs: Box::new(e(ExprKind::Int(2), t.i32_)),
        },
        t.i32_,
    );
    let lines = lines_of(&sum, &p);
    assert!(!lines[0].contains("nsw"), "{}", lines[0]);
    assert!(!lines[0].contains("nuw"), "{}", lines[0]);
    assert!(lines[0].contains("add i32 1, 2"), "{}", lines[0]);
}

#[test]
fn signed_and_unsigned_division_differ() {
    let (p, t) = program();
    for (ty, want) in [(t.i32_, "sdiv"), (t.u32_, "udiv")] {
        let q = e(
            ExprKind::Binary {
                op: BinaryOp::Div,
                lhs: Box::new(e(ExprKind::Int(6), ty)),
                rhs: Box::new(e(ExprKind::Int(3), ty)),
            },
            ty,
        );
        assert!(lines_of(&q, &p)[0].contains(want), "{ty:?}");
    }
}

/// 3.10: `>>` auf vorzeichenbehafteten Werten ist arithmetisch.
#[test]
fn a_signed_shift_right_is_arithmetic() {
    let (p, t) = program();
    for (ty, want) in [(t.i32_, "ashr"), (t.u32_, "lshr")] {
        let sh = e(
            ExprKind::Binary {
                op: BinaryOp::Shr,
                lhs: Box::new(e(ExprKind::Int(8), ty)),
                rhs: Box::new(e(ExprKind::Int(1), ty)),
            },
            ty,
        );
        assert!(lines_of(&sh, &p)[0].contains(want));
    }
}

/// Geordnete Vergleiche: falsch bei NaN. NaN kann in Takt nicht entstehen
/// (4.1), aber die geordnete Form bleibt auch ohne diese Zusage richtig.
#[test]
fn float_comparisons_are_ordered() {
    let (p, t) = program();
    let lt = e(
        ExprKind::Binary {
            op: BinaryOp::Lt,
            lhs: Box::new(e(ExprKind::Float(1.0), t.f64_)),
            rhs: Box::new(e(ExprKind::Float(2.0), t.f64_)),
        },
        t.bool_,
    );
    assert!(lines_of(&lt, &p)[0].contains("fcmp olt double"));
}

#[test]
fn negation_of_a_float_is_fneg() {
    let (p, t) = program();
    let neg = e(ExprKind::Unary { op: UnaryOp::Neg, expr: Box::new(e(ExprKind::Float(1.0), t.f64_)) }, t.f64_);
    assert!(lines_of(&neg, &p)[0].contains("fneg double"));
}

/// `not` ist logisch, `~` bitweise — zwei Operatoren, zwei Instruktionen.
#[test]
fn logical_not_and_bitwise_not_are_different() {
    let (p, t) = program();
    let not = e(ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(e(ExprKind::Bool(true), t.bool_)) }, t.bool_);
    assert!(lines_of(&not, &p)[0].contains("xor i1"));
    let bitnot = e(ExprKind::Unary { op: UnaryOp::BitNot, expr: Box::new(e(ExprKind::Int(1), t.i32_)) }, t.i32_);
    assert!(lines_of(&bitnot, &p)[0].contains("xor i32"));
}

/// Ein Ausdruck, den Schritt 6 nicht kennt, meldet sich — er erzeugt
/// nicht still etwas anderes.
#[test]
fn an_unsupported_expression_says_so() {
    let (mut p, _) = program();
    let mut m = Module::new("t", "x86_64-unknown-linux-gnu");
    m.begin("f", &LlvmType::Void, &[]);
    // Ein Formatstring als Wert entsteht nur in `send` und Meldungen an Ort
    // und Stelle (8.8); als freier Ausdruck meldet sich der Knoten, statt
    // still etwas anderes zu erzeugen.
    let text = push(&mut p, Type::Str { cap: 8 });
    let format = e(ExprKind::Format(takt_mir::pattern::Format::text("x")), text);
    let got = lower(&format, &p, &mut m, &NoVars);
    m.end(None);
    let Err(err) = got else { panic!("ein Formatstring als Wert ist nicht gesenkt") };
    assert_eq!(err.what, "Format-String");
}

/// Die Ranges aus M3 senken die Breite nicht von selbst — der Codegen
/// erzeugt, was in der MIR steht (`Expr::repr`), und erfindet nichts.
#[test]
fn a_declared_range_does_not_change_the_type_by_itself() {
    let (mut p, _) = program();
    let narrow = push(
        &mut p,
        Type::Int {
            width: IntWidth::I64,
            unit: None,
            range: Some(Range {
                lo: takt_mir::types::Const::Int(0),
                hi: takt_mir::types::Const::Int(10),
                origin: RangeOrigin::Declared,
            }),
        },
    );
    assert_eq!(ty::lower(narrow, &p), Some(LlvmType::Int(64)), "die Verengung steht in `repr`, nicht im Typ");
}
