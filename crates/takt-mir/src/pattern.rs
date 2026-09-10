//! Muster, Formatstrings und Adressen (Referenz 8.7, 3.9, 8.1).

use crate::expr::Expr;
use crate::ids::RecordId;

/// Art eines Platzhalters `{name:kind}` (8.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    /// `[+-]?[0-9]{1,19}` → `int`.
    Int,
    /// `(0x)?[0-9a-fA-F]{1,16}` → `int`.
    Hex,
    /// Dezimalzahl mit Exponent → `float`.
    Float,
    /// `[A-Za-z0-9_]{1,64}` → `str<64>`.
    Word,
    /// Beliebige Zeichen, hoechstens N → `str<N>`.
    Str(u32),
}

/// Baustein eines Musterliterals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternPiece {
    /// Literaler Text.
    Text(String),
    /// Platzhalter mit Bindung.
    Capture {
        /// Name der Bindung (`m.name`).
        name: String,
        /// Art.
        kind: CaptureKind,
    },
    /// `{_}`: beliebiger Text ohne Bindung.
    Any,
}

/// Vorkompilierter Automat eines Musters (11.2); Annotation aus M2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dfa {
    /// Alphabetklasse je Byte.
    pub classes: Vec<u8>,
    /// Zahl der Klassen.
    pub class_count: u32,
    /// Uebergangstabelle `states × class_count`.
    pub table: Vec<u32>,
    /// Akzeptierende Zustaende.
    pub accept: Vec<u32>,
}

/// Muster (8.7).
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    /// Musterliteral aus `subtext::pattern_text`.
    Text {
        /// Bausteine.
        pieces: Vec<PatternPiece>,
        /// Automat, sobald M2 ihn erzeugt.
        dfa: Option<Dfa>,
    },
    /// Record-Muster `CanFrame(id = 0x7E8)`: Konjunktion von Feldgleichheiten.
    Record {
        /// Recordtyp.
        record: RecordId,
        /// Feldindex und konstanter Ausdruck.
        fields: Vec<(u32, Expr)>,
    },
}

/// Baustein eines Formatstrings (3.9).
#[derive(Clone, Debug, PartialEq)]
pub enum FormatPiece {
    /// Text.
    Text(String),
    /// `{e}` beziehungsweise `{e:spec}`.
    Expr {
        /// Ausdruck.
        expr: Expr,
        /// `hex`, `.3`, `08`.
        spec: Option<String>,
    },
}

/// Formatstring in Meldungen und `send` (3.9, 16).
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    /// Bausteine.
    pub pieces: Vec<FormatPiece>,
    /// Statisch bekannte Hoechstlaenge in Bytes.
    pub len_max: u32,
}

impl Format {
    /// Reiner Text ohne Platzhalter.
    pub fn text(s: &str) -> Self {
        Format { pieces: vec![FormatPiece::Text(s.to_string())], len_max: s.len() as u32 }
    }
}

/// Segment einer Hardware-Adresse (8.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressSegment {
    /// Name.
    pub name: String,
    /// `[a:b]` fuer Channel-Arrays.
    pub range: Option<(u32, u32)>,
}

/// Adresse `"daq1/ai0"` aus `subtext::address_text` (8.1, 8.10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// Segmente.
    pub segments: Vec<AddressSegment>,
}

impl Address {
    /// Adresse aus `/`-getrennten Namen ohne Bereiche.
    pub fn simple(path: &str) -> Self {
        Address { segments: path.split('/').map(|s| AddressSegment { name: s.to_string(), range: None }).collect() }
    }
}
