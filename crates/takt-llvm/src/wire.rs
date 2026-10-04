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
//! **Felder mit `len`** (FB-413). Ein `bytes<N> with len = n` ist im Draht
//! `n` Byte lang; die Felder danach liegen um die Luecke `N - n` frueher
//! als im Plan, der jedes Feld mit seiner Obergrenze rechnet. Nur fuer
//! solche Records rechnet der Code die Luecke zur Laufzeit mit; die
//! uebrigen bleiben bei festen Plaetzen.
//!
//! **`decode` faultet nie** (3.7). Ein zu kurzer Puffer, ein verletztes
//! Konstantenfeld, eine unbekannte Diskriminante oder ein Wert ausserhalb
//! der Range machen es zu `none`, wie im Interpreter (FB-425)
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

/// Liest ein Feld aus dem Puffer (3.7). Was der Wert erfuellen muss, damit
/// `decode` ihn annimmt, kommt nach `valid`.
fn read_field(
    buf: Reg,
    at: u32,
    ty: TypeId,
    endian: Endian,
    p: &Program,
    m: &mut Module,
    valid: &mut Vec<String>,
) -> Result<Field, NotYet> {
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
        // Ein Enum im Draht ist seine Diskriminante; sie muss eine
        // deklarierte treffen.
        Some(Type::Enum(e)) => {
            let raw = load_int(buf, at, width, endian, m);
            let wide = widen(raw.to_string(), width * 8, 32, m);
            let mut hit = "false".to_string();
            for v in &p.enums.get(e.index()).ok_or(NotYet { what: "Enum" })?.variants {
                let eq = m.inst(&format!("icmp eq i32 {wide}, {}", v.discriminant));
                hit = m.inst(&format!("or i1 {hit}, {eq}")).to_string();
            }
            valid.push(hit);
            Ok(Field { value: wide, ty: LlvmType::Int(32) })
        }
        // Ein Array fester Laenge liegt elementweise hintereinander (3.7).
        Some(Type::Array { elem, len }) => {
            let w = field_size(*elem, p).ok_or(NotYet { what: "Feldgroesse" })?;
            let mut cur = "undef".to_string();
            for i in 0..*len {
                let item = read_field(buf, at + i * w, *elem, endian, p, m, valid)?;
                cur = m.inst(&format!("insertvalue {target} {cur}, {} {}, {i}", item.ty, item.value)).to_string();
            }
            Ok(Field { value: cur, ty: target })
        }
        // Ein `bytes<N>` belegt im Draht seine Kapazitaet und hat gelesen
        // die Laenge N, wie im Interpreter.
        Some(Type::Bytes { cap }) => {
            let ptr = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {at}"));
            let data = m.inst(&format!("load [{cap} x i8], ptr {ptr}, align 1"));
            let with_len = m.inst(&format!("insertvalue {target} undef, i32 {cap}, 0"));
            let v = m.inst(&format!("insertvalue {target} {with_len}, [{cap} x i8] {data}, 1"));
            Ok(Field { value: v.to_string(), ty: target })
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
    // Ein `bytes<N>` mit seiner Laenge, der Rest des Feldes mit Nullen.
    if let Some(Type::Bytes { cap }) = p.types.list.get(ty.index()) {
        let tmp = m.alloca(field_ty);
        m.write(field_ty, operand, &tmp.to_string());
        let len_ptr = m.inst(&format!("getelementptr inbounds {field_ty}, ptr {tmp}, i32 0, i32 0"));
        let len = m.inst(&format!("load i32, ptr {len_ptr}"));
        let short = m.inst(&format!("icmp ult i32 {len}, {cap}"));
        let n = m.inst(&format!("select i1 {short}, i32 {len}, i32 {cap}"));
        let n64 = m.inst(&format!("zext i32 {n} to i64"));
        let slot = m.inst(&format!("getelementptr inbounds i8, ptr {data}, i64 {at}"));
        let src = m.inst(&format!("getelementptr inbounds {field_ty}, ptr {tmp}, i32 0, i32 1"));
        m.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {slot}, i8 0, i64 {cap}, i1 false)"));
        m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {slot}, ptr {src}, i64 {n64}, i1 false)"));
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
    let gaps = Gaps::of(def, p)?;

    let end_at = format!("decode{label}_ende");
    let zu_kurz = format!("decode{label}_kurz");
    // 3.7: zu kurzer Puffer ergibt `none` — mit Feldern variabler Laenge
    // zuerst gegen die kleinste Laenge, am Ende gegen die tatsaechliche.
    let long_enough = m.inst(&format!("icmp uge i32 {len}, {}", gaps.least(size)));
    let go_on = format!("decode{label}_felder");
    m.void_inst(&format!("br i1 {long_enough}, label %{go_on}, label %{zu_kurz}"));
    m.label(&go_on);
    // Kein Feld liest ueber den Puffer hinaus: Mit Feldern variabler Laenge
    // liegt er in einer genullten Kopie der groessten Laenge.
    let buf = if gaps.variable() {
        let copy = m.alloca(&format!("[{size} x i8]"));
        m.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {copy}, i8 0, i64 {size}, i1 false)"));
        let have = m.inst(&format!("zext i32 {len} to i64"));
        let fits = m.inst(&format!("icmp ult i64 {have}, {size}"));
        let n = m.inst(&format!("select i1 {fits}, i64 {have}, i64 {size}"));
        m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {copy}, ptr {buf}, i64 {n}, i1 false)"));
        copy
    } else {
        buf
    };

    let mut value = "undef".to_string();
    let mut checks: Vec<(String, String)> = Vec::new();
    let mut shift = "0".to_string();
    let mut read: Vec<Field> = Vec::with_capacity(def.fields.len());
    let mut valid: Vec<String> = Vec::new();
    for (i, f) in def.fields.iter().enumerate() {
        let at = f.offset.ok_or(NotYet { what: "Feld ohne Versatz" })?;
        let field = match f.len_field {
            Some(j) => {
                let n = read.get(j as usize).ok_or(NotYet { what: "Laengenfeld" })?;
                let (field, ok, next) = read_counted(buf, at, &shift, n, f.ty, p, m)?;
                checks.push((ok, m.block().to_string()));
                shift = next;
                field
            }
            None if shift == "0" => read_field(buf, at, f.ty, layout.endian, p, m, &mut valid)?,
            None => {
                let ptr = field_ptr(buf, at, &shift, m);
                read_field(ptr, 0, f.ty, layout.endian, p, m, &mut valid)?
            }
        };
        // Ein Konstantenfeld muss den deklarierten Wert tragen (3.7).
        if let Some(want) = &f.const_value {
            let expected = const_operand(want).ok_or(NotYet { what: "Konstantenfeld dieses Typs" })?;
            let ok = m.inst(&format!("icmp eq {} {}, {expected}", field.ty, field.value));
            checks.push((ok.to_string(), m.block().to_string()));
        }
        // Eine Range-Verletzung macht `decode` zu `none` (3.7).
        if let Some(ok) = in_range(&field, f.ty, p, m)? {
            valid.push(ok);
        }
        for ok in valid.drain(..) {
            checks.push((ok, m.block().to_string()));
        }
        value = m.inst(&format!("insertvalue {inner} {value}, {} {}, {i}", field.ty, field.value)).to_string();
        read.push(field);
    }
    if gaps.variable() {
        let have = m.inst(&format!("zext i32 {len} to i64"));
        let need = gaps.length(&shift, m);
        let ok = m.inst(&format!("icmp uge i64 {have}, {need}"));
        checks.push((ok.to_string(), m.block().to_string()));
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
    let gaps = Gaps::of(def, p)?;
    // Das Ergebnis ist ein `bytes<SIZE>`: Laenge und Daten (3.9).
    let buf = m.alloca(want);
    let len_ptr = m.inst(&format!("getelementptr inbounds {want}, ptr {buf}, i32 0, i32 0"));
    let data = m.inst(&format!("getelementptr inbounds {want}, ptr {buf}, i32 0, i32 1"));
    if gaps.variable() {
        m.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {data}, i8 0, i64 {size}, i1 false)"));
    }
    // Die Laenge jedes Feldes mit `len` steht vorab fest; `encode` setzt
    // damit sein Laengenfeld (3.7).
    let mut counted: Vec<(usize, u32, String, Reg)> = Vec::new();
    for (i, f) in def.fields.iter().enumerate() {
        if let Some(j) = f.len_field {
            let field_ty = ty::lower(f.ty, p).ok_or(NotYet { what: "Feldtyp" })?;
            let Some(Type::Bytes { cap }) = p.types.list.get(f.ty.index()) else {
                return Err(NotYet { what: "`len` an einem Feld ohne `bytes<N>`" });
            };
            let operand = m.inst(&format!("extractvalue {} {}, {i}", value.ty, value.value));
            let tmp = m.alloca(&field_ty);
            m.write(&field_ty, &operand.to_string(), &tmp.to_string());
            let at = m.inst(&format!("getelementptr inbounds {field_ty}, ptr {tmp}, i32 0, i32 0"));
            let len = m.inst(&format!("load i32, ptr {at}"));
            let wide = m.inst(&format!("zext i32 {len} to i64"));
            let short = m.inst(&format!("icmp ult i64 {wide}, {cap}"));
            let n = m.inst(&format!("select i1 {short}, i64 {wide}, i64 {cap}"));
            counted.push((j as usize, *cap, n.to_string(), tmp));
        }
    }
    let mut shift = "0".to_string();
    let mut next = counted.iter();
    for (i, f) in def.fields.iter().enumerate() {
        let at = f.offset.ok_or(NotYet { what: "Feld ohne Versatz" })?;
        let field_ty = ty::lower(f.ty, p).ok_or(NotYet { what: "Feldtyp" })?;
        let (base, at) = match shift.as_str() {
            "0" => (data, at),
            _ => (field_ptr(data, at, &shift, m), 0),
        };
        if f.len_field.is_some() {
            let (_, cap, n, tmp) = next.next().ok_or(NotYet { what: "Feld mit `len`" })?;
            let src = m.inst(&format!("getelementptr inbounds {field_ty}, ptr {tmp}, i32 0, i32 1"));
            let dst = m.inst(&format!("getelementptr inbounds i8, ptr {base}, i64 {at}"));
            m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {src}, i64 {n}, i1 false)"));
            let gap = m.inst(&format!("sub i64 {cap}, {n}"));
            shift = m.inst(&format!("add i64 {shift}, {gap}")).to_string();
            continue;
        }
        // Ein Konstantenfeld traegt seinen deklarierten Wert, ein
        // Laengenfeld die Laenge seines Feldes, nicht den des Records (3.7).
        let operand = match (&f.const_value, counted.iter().find(|(j, ..)| *j == i)) {
            (Some(c), _) => const_operand(c).ok_or(NotYet { what: "Konstantenfeld dieses Typs" })?,
            (None, Some((_, _, n, _))) => match &field_ty {
                LlvmType::Int(64) => n.clone(),
                LlvmType::Int(bits) => m.inst(&format!("trunc i64 {n} to i{bits}")).to_string(),
                _ => return Err(NotYet { what: "Laengenfeld ohne Ganzzahltyp" }),
            },
            (None, None) => m.inst(&format!("extractvalue {} {}, {i}", value.ty, value.value)).to_string(),
        };
        write_field(base, at, f.ty, &operand, &field_ty, layout.endian, p, m)?;
    }
    let length = match gaps.variable() {
        true => {
            let total = gaps.length(&shift, m);
            m.inst(&format!("trunc i64 {total} to i32")).to_string()
        }
        false => size.to_string(),
    };
    m.void_inst(&format!("store i32 {length}, ptr {len_ptr}"));
    let loaded = m.inst(&format!("load {want}, ptr {buf}"));
    Ok(crate::expr::Lowered { value: loaded.to_string(), ty: want.clone() })
}

/// Liegt ein gelesenes Feld in der Range seines Typs (3.7)? `None`, wenn
/// der Typ keine traegt.
fn in_range(field: &Field, ty: TypeId, p: &Program, m: &mut Module) -> Result<Option<String>, NotYet> {
    let (lo, hi, signed) = match p.types.list.get(ty.index()) {
        Some(Type::Int { range: Some(r), width, .. }) => match (r.lo, r.hi) {
            (Const::Int(lo), Const::Int(hi)) => (lo.to_string(), hi.to_string(), Some(width.signed())),
            _ => return Err(NotYet { what: "Range eines Ganzzahlfelds" }),
        },
        Some(Type::Float { range: Some(r), .. }) => match (r.lo, r.hi) {
            (Const::Float(lo), Const::Float(hi)) => {
                (crate::emit::float_literal(lo, &field.ty), crate::emit::float_literal(hi, &field.ty), None)
            }
            _ => return Err(NotYet { what: "Range eines Gleitkommafelds" }),
        },
        _ => return Ok(None),
    };
    let (ge, le) = match signed {
        Some(true) => ("icmp sge", "icmp sle"),
        Some(false) => ("icmp uge", "icmp ule"),
        None => ("fcmp oge", "fcmp ole"),
    };
    let above = m.inst(&format!("{ge} {} {}, {lo}", field.ty, field.value));
    let below = m.inst(&format!("{le} {} {}, {hi}", field.ty, field.value));
    Ok(Some(m.inst(&format!("and i1 {above}, {below}")).to_string()))
}

/// Was die Felder variabler Laenge eines Records fuer seinen Plan
/// bedeuten (3.7, FB-413).
struct Gaps {
    /// Das Ende des letzten Feldes im Plan, vor `align`.
    end: u32,
    /// Die Summe der Obergrenzen aller Felder mit `len`.
    caps: u32,
    /// `align` des Layouts.
    align: Option<u32>,
}

impl Gaps {
    fn of(def: &takt_mir::types::RecordDef, p: &Program) -> Result<Gaps, NotYet> {
        let (mut end, mut caps) = (0, 0);
        for f in &def.fields {
            let size = field_size(f.ty, p).ok_or(NotYet { what: "Feldgroesse" })?;
            end = end.max(f.offset.ok_or(NotYet { what: "Feld ohne Versatz" })? + size);
            if f.len_field.is_some() {
                caps += size;
            }
        }
        Ok(Gaps { end, caps, align: def.layout.as_ref().and_then(|l| l.align) })
    }

    fn variable(&self) -> bool {
        self.caps > 0
    }

    /// Die kleinste Laenge eines Rahmens: jedes Feld mit `len` leer.
    fn least(&self, size: u32) -> u32 {
        if self.variable() { self.end - self.caps } else { size }
    }

    /// Die Laenge eines Rahmens hinter einer Luecke `shift`: das Ende des
    /// letzten Feldes, nach `align` aufgerundet.
    fn length(&self, shift: &str, m: &mut Module) -> String {
        let end = m.inst(&format!("sub i64 {}, {shift}", self.end));
        match self.align {
            Some(a) if a > 1 => {
                let up = m.inst(&format!("add i64 {end}, {}", a - 1));
                m.inst(&format!("and i64 {up}, {}", -i64::from(a))).to_string()
            }
            _ => end.to_string(),
        }
    }
}

/// Der Platz eines Feldes, dessen Plan-Versatz `at` ist, hinter einer
/// Luecke `shift` der Felder variabler Laenge davor.
fn field_ptr(buf: Reg, at: u32, shift: &str, m: &mut Module) -> Reg {
    if shift == "0" {
        return m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {at}"));
    }
    let place = m.inst(&format!("sub i64 {at}, {shift}"));
    m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {place}"))
}

/// Liest ein `bytes<N> with len = n` (FB-413): `n` aus dem schon gelesenen
/// Laengenfeld, hoechstens N, sonst ist der Rahmen fremd. Ergibt das Feld,
/// die Pruefung `n <= N` und die neue Luecke.
fn read_counted(
    buf: Reg,
    at: u32,
    shift: &str,
    count: &Field,
    ty: TypeId,
    p: &Program,
    m: &mut Module,
) -> Result<(Field, String, String), NotYet> {
    let Some(Type::Bytes { cap }) = p.types.list.get(ty.index()) else {
        return Err(NotYet { what: "`len` an einem Feld ohne `bytes<N>`" });
    };
    let target = ty::lower(ty, p).ok_or(NotYet { what: "Feldtyp" })?;
    let LlvmType::Int(bits) = count.ty else { return Err(NotYet { what: "Laengenfeld ohne Ganzzahltyp" }) };
    // Eine negative Laenge wird gross und faellt durch die Pruefung.
    let n = match bits {
        64 => count.value.clone(),
        _ => m.inst(&format!("sext i{bits} {} to i64", count.value)).to_string(),
    };
    let ok = m.inst(&format!("icmp ule i64 {n}, {cap}"));
    let n = m.inst(&format!("select i1 {ok}, i64 {n}, i64 0"));
    let from = field_ptr(buf, at, shift, m);
    let tmp = m.alloca(&target);
    m.write(&target, "zeroinitializer", &tmp.to_string());
    let len_at = m.inst(&format!("getelementptr inbounds {target}, ptr {tmp}, i32 0, i32 0"));
    let len = m.inst(&format!("trunc i64 {n} to i32"));
    m.void_inst(&format!("store i32 {len}, ptr {len_at}"));
    let data = m.inst(&format!("getelementptr inbounds {target}, ptr {tmp}, i32 0, i32 1"));
    m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {data}, ptr {from}, i64 {n}, i1 false)"));
    let value = m.inst(&format!("load {target}, ptr {tmp}"));
    let gap = m.inst(&format!("sub i64 {cap}, {n}"));
    let next = m.inst(&format!("add i64 {shift}, {gap}"));
    Ok((Field { value: value.to_string(), ty: target }, ok.to_string(), next.to_string()))
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
