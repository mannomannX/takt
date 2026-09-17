//! Drahtformat der Records (3.7): `R.decode(b)` und `f.encode()`.
//!
//! **Der Plan steht im Typ, nicht im Code.** `FieldDef::offset`,
//! `RecordDef::wire_size` und `WireLayout::endian` hat das Sema beim
//! Registrieren gerechnet und geprueft (Pruefung 46). Der Codegen setzt
//! ihn um — er rechnet ihn nicht nach, sonst gaebe es zwei Stellen, an
//! denen ein Offset entsteht, und die zweite waere die falsche.
//!
//! **Darum kein Schleifencode.** Jedes Feld hat seinen festen Platz; der
//! Codegen rollt sie ab. Das ist nicht nur schneller, es ist auch das,
//! was 11.5 fuer den Stack annimmt: keine Laufvariable, kein Index, keine
//! Schranke, die jemand pruefen muesste.
//!
//! **`decode` faultet nie** (3.7). Ein zu kurzer Puffer, ein verletztes
//! Konstantenfeld oder ein Wert ausserhalb der Range machen es zu `none`
//! — das Programm entscheidet mit `match`, was das bedeutet. Ein Fault
//! waere hier falsch: Ein fremder Rahmen auf dem Bus ist ein
//! Betriebszustand, kein Programmfehler.

use takt_mir::RecordId;
use takt_mir::TypeId;
use takt_mir::program::Program;
use takt_mir::types::{Const, Endian, FloatWidth, IntWidth, Type};

use crate::emit::{Module, Reg};
use crate::expr::NotYet;
use crate::ty::{self, LlvmType};

/// Groesse eines Feldtyps in Bytes; dieselbe Rechnung wie im Sema und im
/// Interpreter (`wire::field_size`).
pub fn field_size(ty: TypeId, p: &Program) -> Option<u32> {
    match p.types.list.get(ty.index())? {
        Type::Bool => Some(1),
        Type::Int { width, .. } => Some(width.bits() / 8),
        Type::Float { width, .. } => Some(match width {
            FloatWidth::F32 => 4,
            FloatWidth::F64 => 8,
        }),
        Type::Bytes { cap } => Some(*cap),
        Type::Array { elem, len } => field_size(*elem, p).map(|n| n * len),
        Type::Enum(e) => Some(p.enums.get(e.index())?.layout.map_or(1, |w| w.bits() / 8)),
        Type::Record(r) => p.records.get(r.index())?.wire_size,
        _ => None,
    }
}

/// Liest `width` Bytes ab `at` aus `buf` als Zahl der Breite `bits`.
///
/// Die Byte-Reihenfolge steht im Typ (3.7). Bei `little` ist es ein
/// gewoehnlicher Ladevorgang, bei `big` kommt `llvm.bswap` dazu — LLVM
/// kennt den Befehl, und ihn von Hand zu bauen waere je Breite eine
/// eigene Folge von Schiebeoperationen.
fn load_int(buf: Reg, at: u32, width: u32, endian: Endian, m: &mut Module) -> Reg {
    let bits = width * 8;
    let ty = LlvmType::Int(bits);
    let ptr = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {at}"));
    // `align 1`: Der Puffer ist ein Bytefeld, und ein Feld darf auf jedem
    // Versatz liegen (3.7 kennt `align` als *Deklaration*, nicht als
    // Zusicherung ueber den Puffer).
    let raw = m.inst(&format!("load {ty}, ptr {ptr}, align 1"));
    if endian == Endian::Big && bits > 8 {
        m.needs_intrinsic(&format!("{ty} @llvm.bswap.{ty}({ty})"));
        return m.inst(&format!("call {ty} @llvm.bswap.{ty}({ty} {raw})"));
    }
    raw
}

/// Schreibt eine Zahl an `at`.
fn store_int(buf: Reg, at: u32, width: u32, endian: Endian, value: &str, m: &mut Module) {
    let bits = width * 8;
    let ty = LlvmType::Int(bits);
    let ptr = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {at}"));
    let out = if endian == Endian::Big && bits > 8 {
        m.needs_intrinsic(&format!("{ty} @llvm.bswap.{ty}({ty})"));
        m.inst(&format!("call {ty} @llvm.bswap.{ty}({ty} {value})")).to_string()
    } else {
        value.to_string()
    };
    m.void_inst(&format!("store {ty} {out}, ptr {ptr}, align 1"));
}

/// Was beim Senken eines Feldes herauskommt.
struct Field {
    /// Der gelesene Wert als Operand.
    value: String,
    /// Sein LLVM-Typ.
    ty: LlvmType,
}

/// Liest ein Feld aus dem Puffer (3.7).
fn read_field(buf: Reg, at: u32, ty: TypeId, endian: Endian, p: &Program, m: &mut Module) -> Result<Field, NotYet> {
    let width = field_size(ty, p).ok_or(NotYet { what: "Feldgroesse" })?;
    let target = ty::lower(ty, p).ok_or(NotYet { what: "Feldtyp" })?;
    match p.types.list.get(ty.index()) {
        Some(Type::Bool) => {
            let raw = load_int(buf, at, 1, endian, m);
            let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
            Ok(Field { value: b.to_string(), ty: LlvmType::Int(1) })
        }
        Some(Type::Int { width: w, .. }) => {
            let raw = load_int(buf, at, width, endian, m);
            // Die Breite im Draht ist die des Typs; eine Anpassung
            // brauchte es nur, wenn `repr` sie verengt haette (3.4), und
            // das tut die Analyse fuer Drahtfelder nicht.
            let _ = w;
            Ok(Field { value: raw.to_string(), ty: target })
        }
        Some(Type::Float { width: fw, .. }) => {
            let raw = load_int(buf, at, width, endian, m);
            let as_float = m.inst(&format!(
                "bitcast i{} {raw} to {}",
                width * 8,
                if *fw == FloatWidth::F32 { "float" } else { "double" }
            ));
            Ok(Field { value: as_float.to_string(), ty: target })
        }
        // Ein Enum im Draht ist seine Diskriminante; ob sie eine
        // deklarierte trifft, prueft `validate_enum`.
        Some(Type::Enum(_)) => {
            let raw = load_int(buf, at, width, endian, m);
            let wide = widen(raw.to_string(), width * 8, 32, m);
            Ok(Field { value: wide, ty: LlvmType::Int(32) })
        }
        // Ein Array fester Laenge liegt elementweise hintereinander (3.7).
        Some(Type::Array { elem, len }) => {
            let w = field_size(*elem, p).ok_or(NotYet { what: "Feldgroesse" })?;
            let mut cur = "undef".to_string();
            for i in 0..*len {
                let item = read_field(buf, at + i * w, *elem, endian, p, m)?;
                cur = m.inst(&format!("insertvalue {target} {cur}, {} {}, {i}", item.ty, item.value)).to_string();
            }
            Ok(Field { value: cur, ty: target })
        }
        _ => Err(NotYet { what: "Feld dieses Typs im Drahtformat" }),
    }
}

/// Schreibt ein Feld nach `at` (3.7): ein Array elementweise, alles
/// andere als Zahl seiner Drahtbreite.
#[allow(clippy::too_many_arguments)]
fn write_field(
    data: Reg,
    at: u32,
    ty: TypeId,
    operand: &str,
    field_ty: &LlvmType,
    endian: Endian,
    p: &Program,
    m: &mut Module,
) -> Result<(), NotYet> {
    if let Some(Type::Array { elem, len }) = p.types.list.get(ty.index()) {
        let w = field_size(*elem, p).ok_or(NotYet { what: "Feldgroesse" })?;
        let elem_ty = ty::lower(*elem, p).ok_or(NotYet { what: "Feldtyp" })?;
        for i in 0..*len {
            let item = m.inst(&format!("extractvalue {field_ty} {operand}, {i}")).to_string();
            write_field(data, at + i * w, *elem, &item, &elem_ty, endian, p, m)?;
        }
        return Ok(());
    }
    let width = field_size(ty, p).ok_or(NotYet { what: "Feldgroesse" })?;
    let raw = to_bits(operand, field_ty, width, m);
    store_int(data, at, width, endian, &raw, m);
    Ok(())
}

/// Verbreitert eine Zahl auf `to` Bit, wenn noetig.
fn widen(value: String, from: u32, to: u32, m: &mut Module) -> String {
    if from >= to {
        return value;
    }
    m.inst(&format!("zext i{from} {value} to i{to}")).to_string()
}

/// Senkt `R.decode(b)` (3.7).
///
/// Das Ergebnis ist ein `R?`: der Record und ein Gueltigkeitsflag. Jede
/// Pruefung, die scheitert, springt zum gemeinsamen Ausgang mit `false` —
/// ein `phi` am Ende sammelt die Wege ein, statt das Flag in einem Slot
/// zu fuehren.
pub fn decode(
    buf: Reg,
    len: &str,
    record: RecordId,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    label: u32,
) -> Result<crate::expr::Lowered, NotYet> {
    let def = p.records.get(record.index()).ok_or(NotYet { what: "Record" })?;
    let layout = def.layout.as_ref().ok_or(NotYet { what: "Record ohne `layout`" })?;
    let size = def.wire_size.ok_or(NotYet { what: "Record ohne Drahtgroesse" })?;
    let LlvmType::Struct(wrapper) = want else { return Err(NotYet { what: "`decode` ohne `R?`" }) };
    let inner = wrapper.first().cloned().ok_or(NotYet { what: "Record-Typ" })?;

    let end_at = format!("decode{label}_ende");
    let zu_kurz = format!("decode{label}_kurz");
    // 3.7: zu kurzer Puffer ergibt `none`.
    let long_enough = m.inst(&format!("icmp uge i32 {len}, {size}"));
    let go_on = format!("decode{label}_felder");
    m.void_inst(&format!("br i1 {long_enough}, label %{go_on}, label %{zu_kurz}"));
    m.label(&go_on);

    let mut value = "undef".to_string();
    let mut checks: Vec<(String, String)> = Vec::new();
    for (i, f) in def.fields.iter().enumerate() {
        let at = f.offset.ok_or(NotYet { what: "Feld ohne Versatz" })?;
        let field = read_field(buf, at, f.ty, layout.endian, p, m)?;
        // Ein Konstantenfeld muss den deklarierten Wert tragen (3.7).
        if let Some(want) = &f.const_value {
            let expected = const_operand(want).ok_or(NotYet { what: "Konstantenfeld dieses Typs" })?;
            let ok = m.inst(&format!("icmp eq {} {}, {expected}", field.ty, field.value));
            checks.push((ok.to_string(), m.block().to_string()));
        }
        value = m.inst(&format!("insertvalue {inner} {value}, {} {}, {i}", field.ty, field.value)).to_string();
    }

    // Die Pruefungen werden zu einem Flag verknuepft; ein `and` je
    // Pruefung statt eines Sprungs, weil alle Felder ohnehin gelesen sind
    // und ein Zweig nichts spart.
    let mut ok = "true".to_string();
    for (c, _) in &checks {
        ok = m.inst(&format!("and i1 {ok}, {c}")).to_string();
    }
    let gut = m.block().to_string();
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&zu_kurz);
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&end_at);

    // `phi` sammelt die beiden Wege: der gelesene Record mit seinem Flag,
    // oder `none`.
    let record_value = m.inst(&format!("phi {inner} [ {value}, %{gut} ], [ zeroinitializer, %{zu_kurz} ]"));
    let flag = m.inst(&format!("phi i1 [ {ok}, %{gut} ], [ false, %{zu_kurz} ]"));
    let mut out = m.inst(&format!("insertvalue {want} undef, {inner} {record_value}, 0")).to_string();
    out = m.inst(&format!("insertvalue {want} {out}, i1 {flag}, 1")).to_string();
    Ok(crate::expr::Lowered { value: out, ty: want.clone() })
}

/// Eine Konstante als LLVM-Operand.
fn const_operand(c: &Const) -> Option<String> {
    Some(match c {
        Const::Int(i) => i.to_string(),
        Const::Bool(b) => b.to_string(),
        Const::Duration(d) => d.to_string(),
        Const::Float(f) => crate::emit::float_literal(*f, &LlvmType::F64),
    })
}

/// Senkt `f.encode()` (3.7): der Record als Bytes seiner deklarierten
/// Laenge. Konstantenfelder werden dabei gesetzt.
pub fn encode(
    value: &crate::expr::Lowered,
    record: RecordId,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
) -> Result<crate::expr::Lowered, NotYet> {
    let def = p.records.get(record.index()).ok_or(NotYet { what: "Record" })?;
    let layout = def.layout.as_ref().ok_or(NotYet { what: "Record ohne `layout`" })?;
    let size = def.wire_size.ok_or(NotYet { what: "Record ohne Drahtgroesse" })?;
    // Das Ergebnis ist ein `bytes<SIZE>`: Laenge und Daten (3.9).
    let buf = m.inst(&format!("alloca {want}"));
    let len_ptr = m.inst(&format!("getelementptr inbounds {want}, ptr {buf}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {size}, ptr {len_ptr}"));
    let data = m.inst(&format!("getelementptr inbounds {want}, ptr {buf}, i32 0, i32 1"));
    for (i, f) in def.fields.iter().enumerate() {
        let at = f.offset.ok_or(NotYet { what: "Feld ohne Versatz" })?;
        let field_ty = ty::lower(f.ty, p).ok_or(NotYet { what: "Feldtyp" })?;
        // Ein Konstantenfeld traegt seinen deklarierten Wert, nicht den
        // des Records (3.7).
        let operand = match &f.const_value {
            Some(c) => const_operand(c).ok_or(NotYet { what: "Konstantenfeld dieses Typs" })?,
            None => m.inst(&format!("extractvalue {} {}, {i}", value.ty, value.value)).to_string(),
        };
        write_field(data, at, f.ty, &operand, &field_ty, layout.endian, p, m)?;
    }
    let loaded = m.inst(&format!("load {want}, ptr {buf}"));
    Ok(crate::expr::Lowered { value: loaded.to_string(), ty: want.clone() })
}

/// Ein Feldwert als Zahl seiner Drahtbreite.
fn to_bits(value: &str, ty: &LlvmType, width: u32, m: &mut Module) -> String {
    let bits = width * 8;
    match ty {
        LlvmType::F32 => m.inst(&format!("bitcast float {value} to i32")).to_string(),
        LlvmType::F64 => m.inst(&format!("bitcast double {value} to i64")).to_string(),
        LlvmType::Int(1) => m.inst(&format!("zext i1 {value} to i{bits}")).to_string(),
        LlvmType::Int(n) if *n > bits => m.inst(&format!("trunc i{n} {value} to i{bits}")).to_string(),
        LlvmType::Int(n) if *n < bits => m.inst(&format!("zext i{n} {value} to i{bits}")).to_string(),
        _ => value.to_string(),
    }
}

/// Die Breite eines Integertyps in Bit; wie im Sema.
pub fn int_bits(w: IntWidth) -> u32 {
    w.bits()
}
