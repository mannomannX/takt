//! Typen und Einheiten (Referenz 3.1 bis 3.12, 4.5, 7.5, 8.9).
//!
//! Typen sind in `Program::types` interniert; `TypeId` ist die Identitaet.
//! Einheiten sind nominal (3.2): `UnitId` unterscheidet `bar` und `psi`, die
//! Dimension in `UnitDef` entscheidet ueber die Konvertierbarkeit.

use takt_diag::Span;

use crate::ids::{EnumId, RecordId, TypeId, UnitId};

/// Breite eines Integer-Typs (3.1); `int` ist `I64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum IntWidth {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntWidth {
    /// Bitbreite.
    pub fn bits(self) -> u32 {
        match self {
            IntWidth::I8 | IntWidth::U8 => 8,
            IntWidth::I16 | IntWidth::U16 => 16,
            IntWidth::I32 | IntWidth::U32 => 32,
            IntWidth::I64 | IntWidth::U64 => 64,
        }
    }

    /// Vorzeichenbehaftet?
    pub fn signed(self) -> bool {
        matches!(self, IntWidth::I8 | IntWidth::I16 | IntWidth::I32 | IntWidth::I64)
    }
}

/// Breite von `float` (4.2); `system: float` legt sie programmweit fest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[allow(missing_docs)]
pub enum FloatWidth {
    F32,
    #[default]
    F64,
}

/// Konstanter Wert einer Range-Grenze oder eines Konstantenfelds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Const {
    /// Ganzzahl (auch fuer schmale Breiten).
    Int(i64),
    /// Fliesskommazahl in Programmbreite.
    Float(f64),
    /// Dauer in Nanosekunden.
    Duration(i64),
    /// Wahrheitswert.
    Bool(bool),
}

/// Herkunft einer Range (3.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeOrigin {
    /// Vom Programm deklariert (`in a..b`).
    Declared,
    /// Von der Intervallanalyse bewiesen.
    Proven,
}

/// Wertebereich `lo..hi` (3.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    /// Untergrenze (einschliesslich).
    pub lo: Const,
    /// Obergrenze (einschliesslich).
    pub hi: Const,
    /// Herkunft.
    pub origin: RangeOrigin,
}

/// Einheiten einer Matrix (3.11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatUnits {
    /// Uniforme Form `mat<R, C>[U]`; `None` ist dimensionslos.
    Uniform(Option<UnitId>),
    /// Dimensionierte Form `mat[R, C]` mit Einheitentupeln (Hart 1995).
    Dimensioned {
        /// Zeileneinheiten `r_i`.
        rows: Vec<UnitId>,
        /// Spalteneinheiten `c_j`.
        cols: Vec<UnitId>,
    },
}

/// Art eines Handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// Job-Handle (`job v = …`, 4.5): `done`, `result`.
    Job,
    /// Trigger-Handle (7.5): `armed`, `fired`.
    Trigger,
    /// Eine Blockinstanz (5.7): ihr Typ traegt den Block, damit zwei
    /// Instanzen verschiedener Bloecke verschiedene Typen haben (FB-77).
    Block(crate::BlockId),
}

/// Ein Typ (2.2 in plan/mir.md). Kanaltypen (`Stream`, `Samples`) kommen nur
/// in Channels und Maschinen vor (3.1).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Type {
    Bool,
    Int {
        width: IntWidth,
        unit: Option<UnitId>,
        range: Option<Range>,
    },
    Float {
        width: FloatWidth,
        unit: Option<UnitId>,
        range: Option<Range>,
    },
    Duration {
        range: Option<Range>,
    },
    Enum(EnumId),
    Record(RecordId),
    /// `[N] T`
    Array {
        elem: TypeId,
        len: u32,
    },
    /// `bytes<N>`
    Bytes {
        cap: u32,
    },
    /// `vec<T, N>`
    Vec {
        elem: TypeId,
        cap: u32,
    },
    /// `str<N>`
    Str {
        cap: u32,
    },
    /// `line<N>` (3.9, mit `truncated`).
    Line {
        cap: u32,
    },
    /// `samples<T, N>` (8.9).
    Samples {
        elem: TypeId,
        len: u32,
    },
    /// `table<A, B>` (3.9).
    Table {
        key: TypeId,
        value: TypeId,
    },
    /// `mat<R, C>`, `mat<R, C>[U]`, `mat[R, C]`; `vec[R]` ist `mat[R, (1)]`.
    Mat {
        rows: u32,
        cols: u32,
        units: MatUnits,
    },
    /// `map<K, V, N>` (3.9, v1.1).
    Map {
        key: TypeId,
        value: TypeId,
        cap: u32,
    },
    /// `T?` (3.8).
    Optional(TypeId),
    /// `T!E` (3.8).
    Result {
        ok: TypeId,
        err: EnumId,
    },
    /// `stream<E>` (8.6).
    Stream(TypeId),
    /// `capture<T, N>` (8.9, v1.2), nur als Stream-Element.
    Capture {
        elem: TypeId,
        len: u32,
    },
    /// Job- oder Trigger-Handle.
    Handle(HandleKind),
}

/// Variante eines Enums (3.7).
#[derive(Clone, Debug, PartialEq)]
pub struct VariantDef {
    /// Name.
    pub name: String,
    /// Diskriminante (explizit oder fortlaufend vergeben).
    pub discriminant: i64,
    /// Felder einer Summentyp-Variante.
    pub fields: Vec<FieldDef>,
    /// Position.
    pub span: Span,
}

/// Enum oder Summentyp (3.7, 2.5).
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDef {
    /// Name.
    pub name: String,
    /// Varianten in Deklarationsreihenfolge.
    pub variants: Vec<VariantDef>,
    /// Drahtbreite `layout u8` (3.7).
    pub layout: Option<IntWidth>,
    /// `open`: `match` verlangt `case _` (2.5).
    pub open: bool,
    /// Eingebaut (`FaultKind`, `Quality`, `JobErr`, …).
    pub builtin: bool,
    /// Position.
    pub span: Span,
}

/// Bitfeld in einem Traegerfeld (3.7).
#[derive(Clone, Debug, PartialEq)]
pub struct BitfieldDef {
    /// Name.
    pub name: String,
    /// `bool` oder Integer-Typ.
    pub ty: TypeId,
    /// Erste Bitposition.
    pub lo: u8,
    /// Letzte Bitposition (einschliesslich).
    pub hi: u8,
    /// Zugriffsart (3.7, v1.2); Standard `rw`.
    pub access: Access,
    /// `active_low`: der logische Wert ist invertiert.
    pub active_low: bool,
    /// Position.
    pub span: Span,
}

/// Zugriffsart eines Bitfelds (3.7, v1.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Access {
    #[default]
    Rw,
    Ro,
    Wo,
    /// *write one to clear*: Schreiben setzt nur dieses Bit auf 1.
    W1c,
    /// *write zero to clear*: Schreiben setzt nur dieses Bit auf 0.
    W0c,
    /// Reserviert: weder lesbar noch schreibbar.
    Rsvd,
}

impl Access {
    /// Name in Meldungen.
    pub fn name(self) -> &'static str {
        match self {
            Access::Rw => "rw",
            Access::Ro => "ro",
            Access::Wo => "wo",
            Access::W1c => "w1c",
            Access::W0c => "w0c",
            Access::Rsvd => "rsvd",
        }
    }

    /// Ist das Feld lesbar? (3.7)
    pub fn readable(self) -> bool {
        !matches!(self, Access::Wo | Access::Rsvd)
    }

    /// Ist das Feld schreibbar?
    pub fn writable(self) -> bool {
        !matches!(self, Access::Ro | Access::Rsvd)
    }

    /// Schreibt es ohne Lese-Modifiziere-Schreibe? (`w1c`, `w0c`)
    pub fn clear_only(self) -> bool {
        matches!(self, Access::W1c | Access::W0c)
    }
}

/// Feld eines Records oder einer Variante (3.7).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDef {
    /// Name; Padding heisst `_`.
    pub name: String,
    /// Typ.
    pub ty: TypeId,
    /// Konstantenfeld: `decode` prueft, `encode` setzt.
    pub const_value: Option<Const>,
    /// `offset = N` im Drahtformat.
    pub offset: Option<u32>,
    /// `with len = feld`: Index des Laengenfelds (v1.1).
    pub len_field: Option<u32>,
    /// Bitfelder eines Traegerfelds.
    pub bits: Vec<BitfieldDef>,
    /// Position.
    pub span: Span,
}

/// Byte-Reihenfolge des Drahtformats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Endian {
    Little,
    Big,
}

/// Drahtformat eines Records (3.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireLayout {
    /// Byte-Reihenfolge.
    pub endian: Endian,
    /// `align = N`.
    pub align: Option<u32>,
}

/// Record (3.7); `Edge` und die opaken Native-Kontexte sind eingebaut.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordDef {
    /// Name.
    pub name: String,
    /// Felder in Deklarationsreihenfolge.
    pub fields: Vec<FieldDef>,
    /// Drahtformat; ohne `layout` keine Byte-Repraesentation.
    pub layout: Option<WireLayout>,
    /// Gesamtlaenge im Drahtformat nach `align` (3.7); nur mit `layout`.
    /// Der Plan entsteht einmal im Sema und ist danach die einzige Quelle
    /// fuer Interpreter und Codegen.
    pub wire_size: Option<u32>,
    /// Eingebaut.
    pub builtin: bool,
    /// Position.
    pub span: Span,
}

/// Rationale Zahl, exakt (3.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    /// Zaehler.
    pub num: i64,
    /// Nenner, positiv.
    pub den: u64,
}

impl Rational {
    /// Ganze Zahl.
    pub const fn int(n: i64) -> Self {
        Rational { num: n, den: 1 }
    }
}

/// Zahl der Basisdimensionen: m, kg, s, A, K, mol, cd (3.2).
pub const BASE_DIMENSIONS: usize = 7;

/// Einheit (3.2): Name, Dimensionsvektor, Faktor zur Basiseinheit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitDef {
    /// Name.
    pub name: String,
    /// Exponenten der Basisdimensionen; dimensionslose Zaehlgroessen (`pct`, `B`) sind null.
    pub dimension: [i8; BASE_DIMENSIONS],
    /// Faktor zur Basiseinheit.
    pub factor: Rational,
    /// Verschiebung affiner Einheiten (`degC` = K − 273.15).
    pub affine_offset: Option<Rational>,
    /// Vordefiniert (SI, `bar`, `psi`, `pct`, `B`, …).
    pub predefined: bool,
    /// Position; leer bei vordefinierten Einheiten.
    pub span: Span,
}

/// Internierte Typen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeTable {
    /// Typen; `TypeId` ist der Index.
    pub list: Vec<Type>,
}

impl TypeTable {
    /// Liefert den Index eines Typs, legt ihn bei Bedarf an.
    pub fn intern(&mut self, ty: Type) -> TypeId {
        if let Some(i) = self.list.iter().position(|t| *t == ty) {
            return TypeId(i as u32);
        }
        self.list.push(ty);
        TypeId(self.list.len() as u32 - 1)
    }

    /// Typ zu einem Index.
    pub fn get(&self, id: TypeId) -> &Type {
        &self.list[id.index()]
    }
}

/// Die schmalste Breite, die eine deklarierte Range fasst (3.4,
/// Darstellungsverengung): `int in 0..1_000_000` liegt in vier Byte.
/// Schmale Typen behalten ihre Breite, eine Range ohne Ganzzahlgrenzen
/// die deklarierte.
pub fn storage_width(ty: &Type) -> Option<IntWidth> {
    let (width, range) = match ty {
        Type::Int { width, range, .. } => (*width, *range),
        Type::Duration { range } => (IntWidth::I64, *range),
        _ => return None,
    };
    let Some(Range { lo: Const::Int(lo) | Const::Duration(lo), hi: Const::Int(hi) | Const::Duration(hi), .. }) = range
    else {
        return Some(width);
    };
    if width.bits() < 64 {
        return Some(width);
    }
    let fits = |bits: u32| lo >= -(1i64 << (bits - 1)) && hi < (1i64 << (bits - 1));
    Some(if fits(8) {
        IntWidth::I8
    } else if fits(16) {
        IntWidth::I16
    } else if fits(32) {
        IntWidth::I32
    } else {
        IntWidth::I64
    })
}
