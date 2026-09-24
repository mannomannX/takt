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

/// Ab dieser Groesse geht ein Aggregat per Zeiger durch die Signatur.
///
/// Zwei Worte passen in die Registerpaare jeder Zielklasse; darueber
/// kopierte der Aufruf ohnehin ueber den Stack.
pub const INDIRECT_MIN: u64 = 16;

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

    /// Geht der Typ als Zeiger durch die Signatur (FB-214)?
    ///
    /// Ein Aggregat als Wert laesst LLVM an jeder Aufrufstelle und in
    /// jedem Rumpf Feld fuer Feld kopieren: `bytes<1024>` als Parameter
    /// und Rueckgabe macht aus 1000 Byte C-Code 70 000 Byte. C und Rust
    /// geben grosse Aggregate darum per Zeiger weiter (`sret`, `byval`),
    /// und Takt tut es jetzt auch.
    pub fn indirect(&self) -> bool {
        matches!(self, LlvmType::Struct(_) | LlvmType::Array(..)) && self.size() > INDIRECT_MIN
    }

    /// Groesse in Bytes, so weit der Codegen sie ohne Datenlayout kennt.
    ///
    /// 11.2 braucht sie fuer die Schwelle der Zeigeruebergabe und fuer
    /// `takt size` (11.5).
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

    /// Ausrichtung in Bytes, wie die Datenlayouts der Ziele sie waehlen:
    /// natuerlich, also die Breite des Skalars, bei Aggregaten die
    /// groesste ihrer Felder (x86-64, aarch64, thumbv7em, riscv32 gleich).
    pub fn align(&self) -> u64 {
        match self {
            LlvmType::Int(n) => u64::from(n.div_ceil(8)).next_power_of_two().min(8),
            LlvmType::F32 => 4,
            LlvmType::F64 | LlvmType::Ptr => 8,
            LlvmType::Array(t, _) => t.align(),
            LlvmType::Struct(fields) => fields.iter().map(LlvmType::align).max().unwrap_or(1),
            LlvmType::Void => 1,
        }
    }

    /// Versatz des `i`-ten Feldes eines Structs bei natuerlicher
    /// Ausrichtung — dieselbe Stelle, die `getelementptr` meint.
    pub fn field_offset(&self, i: usize) -> u64 {
        let LlvmType::Struct(fields) = self else { return 0 };
        let mut at = 0u64;
        for (k, f) in fields.iter().enumerate() {
            at = at.div_ceil(f.align()) * f.align();
            if k == i {
                return at;
            }
            at += f.aligned_size();
        }
        at
    }

    /// Groesse mit Ausrichtung: was `alloca` und der Zustands-Struct im
    /// Speicher belegen — die Zahl, mit der ein Rahmen den Platz reserviert.
    pub fn aligned_size(&self) -> u64 {
        match self {
            LlvmType::Struct(fields) => {
                let mut at: u64 = 0;
                for f in fields {
                    at = at.div_ceil(f.align()) * f.align();
                    at += f.aligned_size();
                }
                at.div_ceil(self.align()) * self.align()
            }
            LlvmType::Array(t, n) => t.aligned_size() * u64::from(*n),
            other => other.size(),
        }
    }
}

/// 11.2: Werte ueber dieser Schwelle werden per Zeiger uebergeben.
pub const BY_POINTER: u64 = 64;

/// Der Typ, in dem eine Variable im Zustand liegt: Bereichsganzzahlen in
/// der schmalsten Breite (3.4), gerechnet wird im Typ aus [`lower`].
pub fn storage(ty: TypeId, p: &Program) -> Option<LlvmType> {
    match p.types.list.get(ty.index())? {
        t @ (Type::Int { .. } | Type::Duration { .. }) => Some(LlvmType::Int(bits(takt_mir::types::storage_width(t)?))),
        _ => lower(ty, p),
    }
}

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
        // 8.9: `[t, pre, post, rate, samples]` in fester Reihenfolge, wie
        // der Interpreter es haelt.
        Type::Capture { elem, len } => LlvmType::Struct(vec![
            LlvmType::Int(64),
            LlvmType::Int(32),
            LlvmType::Int(32),
            LlvmType::F64,
            LlvmType::Array(Box::new(lower(*elem, p)?), *len),
        ]),
        // 8.9: `samples<T, N>` liefert je Tick "ein beschraenktes Array";
        // die Reduktionen rechnen darauf. Der Interpreter haelt es ebenso
        // (`Value::Samples` neben `Value::Array`), und die Zahl der
        // gelieferten Samples steht in der Qualitaet, nicht im Typ.
        Type::Samples { elem, len } => LlvmType::Array(Box::new(lower(*elem, p)?), *len),
        // `T?` (3.8): Wert und Gueltigkeitsflag. Das Flag steht hinten,
        // damit der Wert an derselben Stelle liegt wie ohne Wrapper.
        Type::Optional(inner) => LlvmType::Struct(vec![lower(*inner, p)?, LlvmType::Int(1)]),
        // `T!E` (3.8): Wert, Fehlerdiskriminante, Flag. Der Fehler ist
        // ein Enum ohne Felder, also eine Zahl.
        Type::Result { ok, .. } => LlvmType::Struct(vec![lower(*ok, p)?, LlvmType::Int(32), LlvmType::Int(1)]),
        // Ein Handle traegt keinen Wert: Der Slot eines Jobs ist statisch
        // (`Layout::job_slots`), sein Zustand liegt im Abbild (4.5).
        Type::Handle(_) => LlvmType::Int(8),
        Type::Bytes { cap } | Type::Str { cap } => {
            LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), *cap)])
        }
        // Dieselbe Form wie `bytes<N>`, mit dem Elementtyp statt `i8`;
        // `push`, `append` und der Index laufen ueber `collection::layout_of`.
        Type::Vec { elem, cap } => {
            LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(lower(*elem, p)?), *cap)])
        }
        // `map<K, V, N>` (3.9): `N` Slots zu je `1 + K + V` Byte — die Form,
        // die `takt_native::map` sondiert und die `persist` kopiert (5.9).
        Type::Map { key, value, cap } => {
            let k = takt_mir::bytes::max_size(p, *key).ok()?;
            let v = takt_mir::bytes::max_size(p, *value).ok()?;
            LlvmType::Array(Box::new(LlvmType::Int(8)), (1 + k + v) * *cap)
        }
        // `line<N>` ist `str<N>` plus `.truncated` (3.9): Nur dort hat
        // ein *anderer* — der Treiberrand — die Laenge begrenzt, und das
        // Programm koennte es sonst nicht merken (754).
        // `mat<R, C>` (3.11): Zeilen aus Elementen in der Breite von `float`;
        // die Einheiten sind Sache der Sema.
        Type::Mat { rows, cols, .. } => {
            LlvmType::Array(Box::new(LlvmType::Array(Box::new(float(p.config.float_width)), *cols)), *rows)
        }
        Type::Line { cap } => LlvmType::Struct(vec![
            LlvmType::Int(32),
            LlvmType::Array(Box::new(LlvmType::Int(8)), *cap),
            LlvmType::Int(1),
        ]),
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
