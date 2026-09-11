//! Ausdruecke nach LLVM-IR.
//!
//! **Die Totalitaet aus 4.1 ist hier sichtbar.** Ein `Checked`-Knoten der
//! MIR wird zu einem Vergleich und einem Sprung in den Fault-Trampolin;
//! wo M3 bewiesen hat, dass die Pruefung entfallen kann, steht in der MIR
//! kein `Checked`, und der Codegen erzeugt keinen Zweig (plan/m4.md 4.2).
//! Der Codegen entscheidet das nicht selbst — er erzeugt, was dasteht.
//!
//! **Fliesskomma ohne Flags.** Jede `f*`-Zeile entsteht hier, und keine
//! traegt ein Flag; `emit` bietet dafuer keine Moeglichkeit (4.2).

use takt_mir::TypeId;
use takt_mir::expr::{Accessor, BinaryOp, Expr, ExprKind, UnaryOp};
use takt_mir::program::Program;
use takt_mir::types::{IntWidth, Type};

use crate::emit::{Module, float_literal};
use crate::ty::{self, LlvmType};

/// Ein gesenkter Ausdruck: sein Wert als Operand und sein Typ.
///
/// Der Operand ist Text, weil ein Literal in LLVM kein Register belegt —
/// `add i32 %1, 7` ist eine Zeile, nicht zwei.
#[derive(Clone, Debug, PartialEq)]
pub struct Lowered {
    /// Der Operand: `%3`, `7`, `0x3FF0000000000000`.
    pub value: String,
    /// Sein LLVM-Typ.
    pub ty: LlvmType,
}

/// Was der Codegen noch nicht kann.
///
/// Eine ehrliche Meldung statt falschem Code: Schritt 6 deckt Literale,
/// Variablen und die Arithmetik ab. Alles andere nennt sich beim Namen,
/// damit ein Fehlschlag zeigt, was fehlt — und nicht, dass etwas falsch
/// gerechnet wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotYet {
    /// Welcher Knoten.
    pub what: &'static str,
}

/// Wo die Werte der Variablen stehen.
///
/// 11.2 legt sie in den Zustands-Struct der Maschine. Eine Variable zu
/// lesen ist darum ein `getelementptr` und ein `load` — Anweisungen, die
/// in den Modulpuffer gehen. Das Trait bekommt ihn deshalb: Ein Trait,
/// das so taete, als koste ein Variablenzugriff nichts, beschriebe etwas
/// anderes als den erzeugten Code.
pub trait Vars {
    /// Laedt eine Variable und liefert ihren Operanden.
    ///
    /// `None` heisst: Die Variable ist hier nicht erreichbar.
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered>;

    /// Laedt den Wert eines Input-Channels aus dem Prozessabbild.
    ///
    /// 11.2 gibt der Schrittfunktion dafuer den Zeiger `i`. Die Qualitaet
    /// steht daneben (3.5); `dominated` sagt, ob das Programm sie schon
    /// geprueft hat — ist es nicht dominiert, steht in der MIR ein
    /// `Checked`-Knoten darum herum (4.1), und der erzeugt den Zweig.
    fn input(&self, _channel: takt_mir::ChannelId, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Laedt einen Parameter (8.4).
    ///
    /// Parameter sind zur Laufzeit konstant, `tunable` ausgenommen; beide
    /// stehen im Parametervektor des Laufs, nicht im Zustand der Maschine.
    fn param(&self, _id: takt_mir::ParamId, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Laedt den Latch eines eigenen Outputs (`latch(o)`, 9.2).
    fn output(&self, _channel: takt_mir::ChannelId, _m: &mut Module) -> Option<Lowered> {
        None
    }
}

/// Senkt einen Ausdruck und liefert seinen Operanden.
pub fn lower(e: &Expr, p: &Program, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    let want = ty::lower(e.ty, p).ok_or(NotYet { what: "Typ" })?;
    match &e.kind {
        ExprKind::Bool(b) => Ok(Lowered { value: i32::from(*b).to_string(), ty: want }),
        ExprKind::Int(n) => Ok(Lowered { value: n.to_string(), ty: want }),
        ExprKind::Duration(d) => Ok(Lowered { value: d.to_string(), ty: want }),
        ExprKind::Float(f) => Ok(Lowered { value: float_literal(*f, &want), ty: want }),
        ExprKind::Var(id) => vars.var(*id, m).ok_or(NotYet { what: "unbekannte Variable" }),
        ExprKind::Input { channel, .. } => vars.input(*channel, m).ok_or(NotYet { what: "Input" }),
        ExprKind::Param(id) => vars.param(*id, m).ok_or(NotYet { what: "Parameter" }),
        ExprKind::Output(channel) => vars.output(*channel, m).ok_or(NotYet { what: "Output-Latch" }),
        ExprKind::Unary { op, expr } => unary(*op, expr, &want, p, m, vars),
        ExprKind::Binary { op, lhs, rhs } => binary(*op, lhs, rhs, &want, p, m, vars),
        ExprKind::Cond { cond, then, otherwise } => cond_expr(cond, then, otherwise, &want, p, m, vars),
        ExprKind::Accessor { base, accessor: which, args } => access(base, *which, args, &want, p, m, vars),
        ExprKind::Checked { expr, .. } => {
            // Schritt 6 senkt den Wert; der Trampolin, in den die Pruefung
            // springt, entsteht mit der Maschine (Schritt 7). Bis dahin
            // waere ein erzeugter Zweig ohne Ziel schlechter als keiner.
            lower(expr, p, m, vars)
        }
        other => Err(NotYet { what: node_name(other) }),
    }
}

/// `a if c else b` (3.8): in LLVM ein `select`.
///
/// `select` statt Verzweigung, weil beide Seiten total sind (4.1) — es
/// gibt keinen Seiteneffekt, den ein Sprung sparen koennte, und `select`
/// bleibt ein Basisblock. Wo eine Seite einen Fault ausloesen kann, steht
/// in der MIR ein `Checked`-Knoten *darin*, und der erzeugt seinen Zweig
/// selbst.
fn cond_expr(
    cond: &Expr,
    then: &Expr,
    otherwise: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let c = lower(cond, p, m, vars)?;
    let a = lower(then, p, m, vars)?;
    let b = lower(otherwise, p, m, vars)?;
    let r = m.inst(&format!("select i1 {}, {} {}, {} {}", c.value, a.ty, a.value, b.ty, b.value));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Zugriffe, die reine Rechnung sind (3.10).
///
/// `.bit`, `.bits` und `.with_bit` stehen hier, weil sie sich in
/// Schiebeoperationen uebersetzen. Die Qualitaetszugriffe (`.valid`,
/// `.age`, …) brauchen dagegen das Prozessabbild und dessen Aufbau; sie
/// gehoeren zu den Inputs und kommen mit ihnen.
fn access(
    base: &Expr,
    which: Accessor,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(base, p, m, vars)?;
    let arg = |i: usize, m: &mut Module| -> Result<Lowered, NotYet> {
        let e = args.get(i).ok_or(NotYet { what: "Argument fehlt" })?;
        lower(e, p, m, vars)
    };
    match which {
        // `x.bit(i)` = (x >> i) & 1, als `bool`.
        Accessor::Bit => {
            let i = arg(0, m)?;
            let sh = m.inst(&format!("lshr {} {}, {}", x.ty, x.value, i.value));
            let one = m.inst(&format!("and {} {sh}, 1", x.ty));
            let b = m.inst(&format!("icmp ne {} {one}, 0", x.ty));
            Ok(Lowered { value: b.to_string(), ty: LlvmType::Int(1) })
        }
        // `x.with_bit(i, b)`: setzen oder loeschen, ohne Verzweigung.
        Accessor::WithBit => {
            let (i, b) = (arg(0, m)?, arg(1, m)?);
            let mask = m.inst(&format!("shl {} 1, {}", x.ty, i.value));
            let set = m.inst(&format!("or {} {}, {mask}", x.ty, x.value));
            let inv = m.inst(&format!("xor {} {mask}, -1", x.ty));
            let clear = m.inst(&format!("and {} {}, {inv}", x.ty, x.value));
            let r = m.inst(&format!("select i1 {}, {} {set}, {} {clear}", b.value, x.ty, x.ty));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        _ => Err(NotYet { what: "Zugriff (`.valid`, `.age`, ...)" }),
    }
}

/// Ein einstelliger Operator.
fn unary(
    op: UnaryOp,
    expr: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(expr, p, m, vars)?;
    let value = match op {
        // LLVM hat kein `fneg`-Flag-Problem: `fneg` ist exakt (nur das
        // Vorzeichenbit) und traegt darum ohnehin nie ein Flag.
        UnaryOp::Neg if x.ty.is_float() => m.inst(&format!("fneg {} {}", x.ty, x.value)),
        UnaryOp::Neg => m.inst(&format!("sub {} 0, {}", x.ty, x.value)),
        // `not` ist logisch und nur auf `bool` definiert, `~` bitweise.
        UnaryOp::Not => m.inst(&format!("xor i1 {}, true", x.value)),
        UnaryOp::BitNot => m.inst(&format!("xor {} {}, -1", x.ty, x.value)),
    };
    Ok(Lowered { value: value.to_string(), ty: want.clone() })
}

/// Ein zweistelliger Operator.
fn binary(
    op: BinaryOp,
    lhs: &Expr,
    rhs: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    // `and`/`or` sind in Takt nicht kurzschluessig verschieden von `&`/`|`
    // auf `bool`: Beide Operanden sind total (4.1), also darf beides eine
    // Instruktion sein. Ein Kurzschluss waere hier eine Verhaltensaenderung
    // ohne Gewinn — es gibt keine Seiteneffekte, die er spaeren koennte.
    let a = lower(lhs, p, m, vars)?;
    let b = lower(rhs, p, m, vars)?;
    let float = a.ty.is_float();
    let signed = int_is_signed(lhs.ty, p);

    let text = match op {
        BinaryOp::Add if float => format!("fadd {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Sub if float => format!("fsub {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Mul if float => format!("fmul {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div if float => format!("fdiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem if float => format!("frem {} {}, {}", a.ty, a.value, b.value),
        // Ganzzahlueberlauf ist ein Fault (4.1), kein Wraparound — und der
        // `Checked`-Knoten darueber prueft ihn. `nsw`/`nuw` waeren hier
        // falsch: Sie erklaeren den Ueberlauf fuer undefiniert, und dann
        // duerfte LLVM die Pruefung wegoptimieren, die ihn fangen soll.
        BinaryOp::Add => format!("add {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Sub => format!("sub {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Mul => format!("mul {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div if signed => format!("sdiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div => format!("udiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem if signed => format!("srem {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem => format!("urem {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitAnd | BinaryOp::And => format!("and {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitOr | BinaryOp::Or => format!("or {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitXor => format!("xor {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Shl => format!("shl {} {}, {}", a.ty, a.value, b.value),
        // 3.10: `>>` auf vorzeichenbehafteten Werten ist arithmetisch.
        BinaryOp::Shr if signed => format!("ashr {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Shr => format!("lshr {} {}, {}", a.ty, a.value, b.value),
        // Vergleiche: `o`-Praefix heisst „geordnet", also falsch bei NaN.
        // NaN kann in Takt nicht entstehen (4.1, `finite()`), aber die
        // geordnete Form ist die, die ohne diese Zusage richtig bleibt.
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne => {
            compare(op, &a, &b, float, signed)
        }
    };
    let r = m.inst(&text);
    let out = if matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne) {
        LlvmType::Int(1)
    } else {
        want.clone()
    };
    Ok(Lowered { value: r.to_string(), ty: out })
}

/// Ein Vergleich.
fn compare(op: BinaryOp, a: &Lowered, b: &Lowered, float: bool, signed: bool) -> String {
    if float {
        let cc = match op {
            BinaryOp::Lt => "olt",
            BinaryOp::Le => "ole",
            BinaryOp::Gt => "ogt",
            BinaryOp::Ge => "oge",
            BinaryOp::Eq => "oeq",
            _ => "one",
        };
        return format!("fcmp {cc} {} {}, {}", a.ty, a.value, b.value);
    }
    let cc = match (op, signed) {
        (BinaryOp::Lt, true) => "slt",
        (BinaryOp::Lt, false) => "ult",
        (BinaryOp::Le, true) => "sle",
        (BinaryOp::Le, false) => "ule",
        (BinaryOp::Gt, true) => "sgt",
        (BinaryOp::Gt, false) => "ugt",
        (BinaryOp::Ge, true) => "sge",
        (BinaryOp::Ge, false) => "uge",
        (BinaryOp::Eq, _) => "eq",
        _ => "ne",
    };
    format!("icmp {cc} {} {}, {}", a.ty, a.value, b.value)
}

/// Ist der Typ eine vorzeichenbehaftete Ganzzahl?
///
/// `bool` und die Dauern zaehlen als vorzeichenbehaftet: Dauern sind
/// Nanosekunden in `i64` und duerfen negativ sein (3.2).
fn int_is_signed(ty: TypeId, p: &Program) -> bool {
    match p.types.list.get(ty.index()) {
        Some(Type::Int { width, .. }) => ty::signed(*width),
        Some(Type::Duration { .. }) => true,
        _ => true,
    }
}

/// Die Breite eines Integertyps, wenn es einer ist.
pub fn int_width(ty: TypeId, p: &Program) -> Option<IntWidth> {
    match p.types.list.get(ty.index())? {
        Type::Int { width, .. } => Some(*width),
        _ => None,
    }
}

/// Der Name eines MIR-Knotens fuer die Meldung.
///
/// Eine Meldung, die nur „Ausdruck" sagt, zeigt nicht, was fehlt — und
/// genau danach fragt, wer den Codegen weiterbaut.
fn node_name(e: &ExprKind) -> &'static str {
    match e {
        ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Duration(_) => "Literal",
        ExprKind::Str(_) => "Zeichenkette",
        ExprKind::None => "`none`",
        ExprKind::Default => "`default`",
        ExprKind::Variant { .. } => "Variante",
        ExprKind::Record { .. } => "Record",
        ExprKind::Array(_) => "Array-Literal",
        ExprKind::Tuple(_, _) => "Stuetzstelle",
        ExprKind::BlockInit { .. } => "Blockinstanz",
        ExprKind::Command(_) => "Command",
        ExprKind::Published { .. } => "Psi",
        ExprKind::StateOf(_) => "`m.state`",
        ExprKind::Signal { .. } => "Signal",
        ExprKind::Builtin(_) => "eingebauter Bezeichner",
        ExprKind::Field { .. } => "Feldzugriff",
        ExprKind::Index { .. } => "Index",
        ExprKind::Accessor { .. } => "Zugriff (`.valid`, `.age`, ...)",
        ExprKind::Call { .. } => "Funktionsaufruf",
        ExprKind::NativeCall { .. } => "native Funktion",
        ExprKind::Convert { .. } => "Konversion",
        ExprKind::Cast { .. } => "`as`",
        ExprKind::Cond { .. } => "Bedingter Ausdruck",
        ExprKind::Slice { .. } => "Teilbereich",
        ExprKind::Matches { .. } => "`matches`",
        ExprKind::MatOp { .. } => "Matrixoperation",
        ExprKind::Decode { .. } => "`decode`",
        ExprKind::Format(_) => "Format-String",
        ExprKind::Stream(_) => "Strom",
        ExprKind::Lift(_) => "Anhebung",
        ExprKind::Ok(_) | ExprKind::Err(_) => "`ok`/`err`",
        _ => "Ausdruck",
    }
}
