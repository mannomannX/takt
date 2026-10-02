//! `last_fault` im erzeugten Code (5.3).
//!
//! Eine Maschine, die `last_fault` liest, fuehrt ein Feld aus Art (die Zahl
//! aus [`crate::abi::fault_code`]), Zeile, Tick und Nachricht; eine, die es
//! nicht liest, fuehrt nichts davon. Die Zeile kennt jede Fault-Stelle beim
//! Uebersetzen: Der Block, der die Art ablegt, legt sie in die Ablage der
//! Zeile dazu ([`crate::emit::Module::fault_to`]). `check`, `expect` und
//! `abort` schreiben ihre Nachricht in die Ablage der Nachricht ([`state`],
//! beide in [`crate::arena::fault`]); jeder andere Fault
//! nennt den Namen seiner Art. Der Fault-Pfad uebernimmt beides
//! ([`record`]).
//!
//! **Warum Ablagen in der Arena.** Zwischen der Stelle und dem Fault-Pfad
//! liegen Funktionsgrenzen — eine reine Funktion faultet ihren Aufrufer,
//! eine `loop:`-Funktion kehrt mit dem Fault-Flag zurueck (4.1). Zeile und
//! Nachricht nehmen denselben Weg wie die Art im Flag: ueber die Arena, die
//! jede erzeugte Funktion kennt, damit jede Instanz ihre eigenen hat
//! (12.11).

use takt_mir::Program;
use takt_mir::pattern::Format;

use crate::emit::{Module, Reg};
use crate::expr::{Lowered, NotYet, Vars};
use crate::ty::LlvmType;

/// Die Namen der Arten.
const NAMES: &str = "takt_fault_names";

/// So viele Byte fasst `LastFault.message` (Prelude, `str<128>`).
const TEXT_CAP: u32 = 128;

/// Die Namen der Arten in der Reihenfolge von `FaultKind` im Prelude — der
/// Reihenfolge, die `abi::fault_code` zaehlt.
const KINDS: [&str; 12] = [
    "CHECK_FAILED",
    "EXPECT",
    "TIMEOUT",
    "SENSOR_FAULT",
    "MISSING_VALUE",
    "ARITHMETIC",
    "RANGE",
    "STREAM_OVERFLOW",
    "TIMING",
    "SCHEDULE_OVERFLOW",
    "ABORT",
    "RUNTIME",
];

/// Der laengste Name in [`KINDS`].
const NAME_CAP: u32 = 17;

/// `str<128>`: Laenge und Bytes.
fn text_type() -> LlvmType {
    crate::arena::text_type()
}

/// Ein Eintrag der Namenstabelle: das Praefix eines `str<128>`.
fn name_type() -> LlvmType {
    LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), NAME_CAP)])
}

/// Das Feld `last_fault` im Zustand: Art, Zeile, Tick, Nachricht.
pub fn field_type() -> LlvmType {
    LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Int(32), LlvmType::Int(64), text_type()])
}

/// Die Namenstabelle; nur in einem Modul, dessen Maschinen `last_fault`
/// lesen. Die Ablagen stehen in der Arena ([`crate::arena::fault`]).
pub fn declare(m: &mut Module) {
    let entries: Vec<String> = KINDS
        .iter()
        .map(|k| {
            let pad = "\\00".repeat((NAME_CAP as usize) - k.len());
            format!("{} {{ i32 {}, [{NAME_CAP} x i8] c\"{k}{pad}\" }}", name_type(), k.len())
        })
        .collect();
    m.declare(&format!("@{NAMES} = internal constant [{} x {}] [{}]", KINDS.len(), name_type(), entries.join(", ")));
}

/// Die Nachricht eines `check`, `expect` oder `abort` nach [`TEXT`]: die
/// Meldung der Anweisung, ohne sie `default`, auf 128 Byte an einer
/// Zeichengrenze gekuerzt (5.3).
pub fn state(
    message: Option<&Format>,
    default: &str,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<(), NotYet> {
    let fallback = Format::text(default);
    let f = message.unwrap_or(&fallback);
    let cap = f.len_max.max(1);
    let ty = LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), cap)]);
    let buffer = m.alloca(&ty);
    crate::format::render(f, buffer, &ty, cap, p, m, vars)?;
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 1, i32 0"));
    let mut n = len.to_string();
    if cap > TEXT_CAP {
        let long = m.inst(&format!("icmp ugt i32 {len}, {TEXT_CAP}"));
        n = m.inst(&format!("select i1 {long}, i32 {TEXT_CAP}, i32 {len}")).to_string();
        // Ein Zeichen in UTF-8 hat hoechstens drei Folgebytes (10xxxxxx);
        // steht an der Schnittstelle eines, faengt das Zeichen davor an.
        for _ in 0..3 {
            let inside = m.inst(&format!("icmp ult i32 {n}, {len}"));
            let at = m.inst(&format!("select i1 {inside}, i32 {n}, i32 0"));
            let at = m.inst(&format!("zext i32 {at} to i64"));
            let b_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i64 {at}"));
            let b = m.inst(&format!("load i8, ptr {b_ptr}"));
            let top = m.inst(&format!("and i8 {b}, -64"));
            let follows = m.inst(&format!("icmp eq i8 {top}, -128"));
            let back = m.inst(&format!("and i1 {inside}, {follows}"));
            let less = m.inst(&format!("sub i32 {n}, 1"));
            n = m.inst(&format!("select i1 {back}, i32 {less}, i32 {n}")).to_string();
        }
    }
    let text = text_type();
    let text_at = crate::arena::at(crate::arena::fault::TEXT, m);
    m.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {text_at}, i8 0, i64 {}, i1 false)", text.aligned_size()));
    m.void_inst(&format!("store i32 {n}, ptr {text_at}"));
    let dst = m.inst(&format!("getelementptr inbounds {text}, ptr {text_at}, i32 0, i32 1, i32 0"));
    let wide = m.inst(&format!("zext i32 {n} to i64"));
    m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {bytes}, i64 {wide}, i1 false)"));
    let stated_at = crate::arena::at(crate::arena::fault::STATED, m);
    m.void_inst(&format!("store i1 true, ptr {stated_at}"));
    Ok(())
}

/// Der Fault-Pfad schreibt `last_fault` (5.3): die Art `code`, die Zeile
/// der Stelle, den Tick und die Nachricht — die der Anweisung, sonst den
/// Namen der Art.
pub fn record(at: &Reg, code: &str, tick_ns: i64, m: &mut Module) {
    let ty = field_type();
    let text = text_type();
    let field = |i: u32, m: &mut Module| m.inst(&format!("getelementptr inbounds {ty}, ptr {at}, i32 0, i32 {i}"));
    let code_ptr = field(0, m);
    m.void_inst(&format!("store i32 {code}, ptr {code_ptr}"));
    let line = m.inst(&format!("load i32, ptr {}", crate::arena::PARAM));
    let line_ptr = field(1, m);
    m.void_inst(&format!("store i32 {line}, ptr {line_ptr}"));
    let now = m.inst(&format!("call i64 @{}(ptr %arena)", crate::abi::Abi::NOW));
    let tick = m.inst(&format!("sdiv i64 {now}, {tick_ns}"));
    let tick_ptr = field(2, m);
    m.void_inst(&format!("store i64 {tick}, ptr {tick_ptr}"));
    let dst = field(3, m);
    m.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {dst}, i8 0, i64 {}, i1 false)", text.aligned_size()));
    let stated_at = crate::arena::at(crate::arena::fault::STATED, m);
    let stated = m.inst(&format!("load i1, ptr {stated_at}"));
    let k = m.next_label();
    let (said, named, done) = (format!("gesagt{k}"), format!("benannt{k}"), format!("vermerkt{k}"));
    m.void_inst(&format!("br i1 {stated}, label %{said}, label %{named}"));
    m.label(&said);
    let text_at = crate::arena::at(crate::arena::fault::TEXT, m);
    m.void_inst(&format!(
        "call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {text_at}, i64 {}, i1 false)",
        text.aligned_size()
    ));
    m.void_inst(&format!("store i1 false, ptr {stated_at}"));
    m.void_inst(&format!("br label %{done}"));
    m.label(&named);
    // Art `code` ist eins plus die Variante, die Nutzlast acht Bit darueber.
    let variant = m.inst(&format!("sub i32 {code}, 1"));
    let variant = m.inst(&format!("and i32 {variant}, 255"));
    let names = format!("[{} x {}]", KINDS.len(), name_type());
    let src = m.inst(&format!("getelementptr inbounds {names}, ptr @{NAMES}, i32 0, i32 {variant}"));
    m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {src}, i64 {}, i1 false)", 4 + NAME_CAP));
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
}

/// `last_fault` als Wert des Records `LastFault` (Prelude): Art mit
/// Nutzlast, Nachricht, Zeile, Tick. Vor dem ersten Fault ist die Art null
/// — `CHECK_FAILED` ohne Nutzlast wie im Interpreter —, der Rest leer.
pub fn value(at: &Reg, p: &Program, m: &mut Module) -> Result<Lowered, NotYet> {
    let record = p.records.iter().find(|r| r.name == "LastFault").ok_or(NotYet { what: "Record `LastFault`" })?;
    let lower = |i: usize| {
        record.fields.get(i).and_then(|f| crate::ty::lower(f.ty, p)).ok_or(NotYet { what: "Feld von `LastFault`" })
    };
    let (kind_ty, text_ty, line_ty, tick_ty) = (lower(0)?, lower(1)?, lower(2)?, lower(3)?);
    if text_ty != text_type() || line_ty != LlvmType::Int(64) || tick_ty != LlvmType::Int(64) {
        return Err(NotYet { what: "`LastFault` mit anderer Gestalt als `str<128>`, `int`, `int`" });
    }
    let LlvmType::Struct(parts) = &kind_ty else { return Err(NotYet { what: "`FaultKind` ohne Nutzlast" }) };
    let Some(LlvmType::Array(_, width)) = parts.get(1) else { return Err(NotYet { what: "`FaultKind` ohne Faecher" }) };
    ordinal("FaultKind", KINDS.len(), p)?;
    ordinal("ArithKind", 1, p)?;
    ordinal("RuntimeKind", 1, p)?;

    let ty = field_type();
    let field = |i: u32, m: &mut Module| m.inst(&format!("getelementptr inbounds {ty}, ptr {at}, i32 0, i32 {i}"));
    let code_ptr = field(0, m);
    let code = m.inst(&format!("load i32, ptr {code_ptr}"));
    let none = m.inst(&format!("icmp eq i32 {code}, 0"));
    let variant = m.inst(&format!("sub i32 {code}, 1"));
    let variant = m.inst(&format!("and i32 {variant}, 255"));
    let variant = m.inst(&format!("select i1 {none}, i32 0, i32 {variant}"));
    let payload = m.inst(&format!("lshr i32 {code}, 8"));
    let payload = m.inst(&format!("zext i32 {payload} to i64"));
    let arr = LlvmType::Array(Box::new(LlvmType::Int(64)), *width);
    let slots = m.inst(&format!("insertvalue {arr} zeroinitializer, i64 {payload}, 0"));
    let kind = m.inst(&format!("insertvalue {kind_ty} undef, i32 {variant}, 0"));
    let kind = m.inst(&format!("insertvalue {kind_ty} {kind}, {arr} {slots}, 1"));

    let text_ptr = field(3, m);
    let message = m.inst(&format!("load {text_ty}, ptr {text_ptr}"));
    let line_ptr = field(1, m);
    let line = m.inst(&format!("load i32, ptr {line_ptr}"));
    let line = m.inst(&format!("zext i32 {line} to i64"));
    let tick_ptr = field(2, m);
    let tick = m.inst(&format!("load i64, ptr {tick_ptr}"));

    let want = LlvmType::Struct(vec![kind_ty.clone(), text_ty.clone(), line_ty, tick_ty]);
    let v = m.inst(&format!("insertvalue {want} undef, {kind_ty} {kind}, 0"));
    let v = m.inst(&format!("insertvalue {want} {v}, {text_ty} {message}, 1"));
    let v = m.inst(&format!("insertvalue {want} {v}, i64 {line}, 2"));
    let v = m.inst(&format!("insertvalue {want} {v}, i64 {tick}, 3"));
    Ok(Lowered { value: v.to_string(), ty: want })
}

/// Traegt jede der ersten `n` Varianten des Prelude-Enums `name` ihre
/// Nummer als Diskriminante? Nur dann ist die Zahl aus `abi::fault_code`
/// unmittelbar der Wert.
fn ordinal(name: &str, n: usize, p: &Program) -> Result<(), NotYet> {
    let e = p.enums.iter().find(|e| e.name == name).ok_or(NotYet { what: "Enum der Fault-Arten" })?;
    let ok = e.variants.len() >= n && e.variants.iter().enumerate().all(|(i, v)| v.discriminant == i as i64);
    ok.then_some(()).ok_or(NotYet { what: "Fault-Art mit eigener Diskriminante" })
}
