//! Typen der MIR nach LLVM-Typen (11.2).
//!
//! **Die Darstellungsverengung aus M3 entscheidet die Breite.** 4.2 des
//! Plans nennt `Expr::repr` als das, was der Codegen erbt: Wo die
//! Intervallanalyse bewiesen hat, dass ein Wert in `i32` passt, wird die
//! LLVM-Breite `i32` — ohne sie waere alles `i64`.

use core::fmt;

use takt_mir::TypeId;
use takt_mir::program::Program;
use takt_mir::types::{FloatWidth, IntWidth, Type};

/// Ein LLVM-Typ, so weit der Codegen ihn braucht.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LlvmType {
    /// `i1` bis `i64`; das Vorzeichen steckt in der *Operation*, nicht im
    /// Typ — LLVM kennt nur Breiten, und `sdiv`/`udiv` unterscheiden sich.
    Int(u32),
    /// `float`
    F32,
    /// `double`
    F64,
    /// `[N x T]`
    Array(Box<LlvmType>, u32),
    /// `{ T, ... }`
    Struct(Vec<LlvmType>),
    /// `ptr` (opaque, seit LLVM 15).
    Ptr,
    /// `void`
    Void,
}

impl fmt::Display for LlvmType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlvmType::Int(n) => write!(f, "i{n}"),
            LlvmType::F32 => write!(f, "float"),
            LlvmType::F64 => write!(f, "double"),
            LlvmType::Array(t, n) => write!(f, "[{n} x {t}]"),
            LlvmType::Struct(fields) => {
                write!(f, "{{ ")?;
                for (i, t) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{t}")?;
                }
                write!(f, " }}")
            }
            LlvmType::Ptr => write!(f, "ptr"),
            LlvmType::Void => write!(f, "void"),
        }
    }
}

impl LlvmType {
    /// Ist der Typ ein Fliesskommatyp? Davon haengt ab, ob `fadd` oder
    /// `add` erzeugt wird.
    pub fn is_float(&self) -> bool {
        matches!(self, LlvmType::F32 | LlvmType::F64)
    }

    /// Groesse in Bytes, so weit der Codegen sie ohne Datenlayout kennt.
    ///
    /// 11.2 braucht sie fuer die Schwelle der Zeigeruebergabe (Default
    /// 64 Byte) und fuer `takt size` (11.5).
    pub fn size(&self) -> u64 {
        match self {
            LlvmType::Int(n) => u64::from(n.div_ceil(8)),
            LlvmType::F32 => 4,
            LlvmType::F64 => 8,
            LlvmType::Array(t, n) => t.size() * u64::from(*n),
            // Ohne Ausrichtung: Die genaue Groesse kennt erst das
            // Datenlayout des Targets. `takt size` rechnet eigenstaendig
            // (11.5); hier genuegt die Schwelle aus 11.2.
            LlvmType::Struct(fields) => fields.iter().map(LlvmType::size).sum(),
            LlvmType::Ptr => 8,
            LlvmType::Void => 0,
        }
    }
}

/// 11.2: Werte ueber dieser Schwelle werden per Zeiger uebergeben.
pub const BY_POINTER: u64 = 64;

/// Der LLVM-Typ zu einem MIR-Typ.
///
/// `None` heisst: Der Typ ist im Codegen noch nicht abgebildet. Das ist
/// eine ehrliche Auskunft und kein Fehler — Schritt 6 deckt die Skalare
/// und die zusammengesetzten Typen fester Groesse ab; Stroeme, Tabellen
/// und Matrizen kommen mit den Schritten, die sie brauchen.
pub fn lower(ty: TypeId, p: &Program) -> Option<LlvmType> {
    Some(match p.types.list.get(ty.index())? {
        Type::Bool => LlvmType::Int(1),
        Type::Int { width, .. } => LlvmType::Int(bits(*width)),
        Type::Float { width, .. } => float(*width),
        // Dauern sind Nanosekunden (3.2), immer i64.
        Type::Duration { .. } => LlvmType::Int(64),
        // Eine Enum-Variante ohne Felder ist ihre Diskriminante (11.2);
        // mit Feldern braucht sie ein Struct, das erst der Musterabgleich
        // fuellt.
        Type::Enum(id) => {
            let e = p.enums.get(id.index())?;
            if e.variants.iter().all(|v| v.fields.is_empty()) {
                LlvmType::Int(32)
            } else {
                return None;
            }
        }
        Type::Record(id) => {
            let r = p.records.get(id.index())?;
            let mut fields = Vec::with_capacity(r.fields.len());
            for f in &r.fields {
                fields.push(lower(f.ty, p)?);
            }
            LlvmType::Struct(fields)
        }
        Type::Array { elem, len } => LlvmType::Array(Box::new(lower(*elem, p)?), *len),
        // `T?` (3.8): Wert und Gueltigkeitsflag. Das Flag steht hinten,
        // damit der Wert an derselben Stelle liegt wie ohne Wrapper.
        Type::Optional(inner) => LlvmType::Struct(vec![lower(*inner, p)?, LlvmType::Int(1)]),
        // `T!E` (3.8): Wert, Fehlerdiskriminante, Flag. Der Fehler ist
        // ein Enum ohne Felder, also eine Zahl.
        Type::Result { ok, .. } => LlvmType::Struct(vec![lower(*ok, p)?, LlvmType::Int(32), LlvmType::Int(1)]),
        Type::Bytes { cap } | Type::Str { cap } => {
            LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), *cap)])
        }
        _ => return None,
    })
}

/// Die Bitbreite eines Integertyps.
pub fn bits(w: IntWidth) -> u32 {
    match w {
        IntWidth::I8 | IntWidth::U8 => 8,
        IntWidth::I16 | IntWidth::U16 => 16,
        IntWidth::I32 | IntWidth::U32 => 32,
        IntWidth::I64 | IntWidth::U64 => 64,
    }
}

/// Ist die Breite vorzeichenbehaftet? LLVM kennt das nicht im Typ, aber
/// `sdiv` und `udiv` unterscheiden sich.
pub fn signed(w: IntWidth) -> bool {
    matches!(w, IntWidth::I8 | IntWidth::I16 | IntWidth::I32 | IntWidth::I64)
}

/// Der LLVM-Typ einer Fliesskommabreite.
pub fn float(w: FloatWidth) -> LlvmType {
    match w {
        FloatWidth::F32 => LlvmType::F32,
        FloatWidth::F64 => LlvmType::F64,
    }
}
