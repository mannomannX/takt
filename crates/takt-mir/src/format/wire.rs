//! Drahtschicht von `TAKT-MIR` (grammar/mir-format.md, Abschnitte W1 bis W4):
//! Varints, Schluessel, Knoten aus Feldern, Stringtabelle.

use std::collections::HashMap;
use std::fmt;

/// Drahtart eines Felds; die unteren zwei Bits des Schluessels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    /// Varint (Bool, Ganzzahlen mit Zickzack, Indizes, Stringnummern, Unit-Enums).
    Varint = 0,
    /// 8 Bytes Little-Endian (f64 als Bitmuster).
    Fixed64 = 1,
    /// Laenge als Varint, dann Bytes (Knoten, Bytefolgen).
    Bytes = 2,
}

/// Fehler beim Lesen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// Kein `TAKT-MIR`-Kopf.
    BadMagic,
    /// Formatversion neuer als der Leser.
    UnsupportedVersion(u32),
    /// Datei endet mitten in einem Wert.
    Truncated,
    /// Varint laenger als 10 Bytes.
    BadVarint,
    /// Unbekannte Drahtart.
    BadWire(u8),
    /// Stringnummer ausserhalb der Tabelle.
    BadString(u32),
    /// Kein gueltiges UTF-8.
    BadUtf8,
    /// Pflichtfeld fehlt: Knoten und Feldnummer.
    Missing(&'static str, u32),
    /// Feld mehrfach vorhanden, wo eines erwartet wird.
    Duplicate(&'static str, u32),
    /// Feld mit falscher Drahtart.
    WrongWire(&'static str, u32),
    /// Unbekannte Variante eines Enums.
    BadVariant(&'static str, u64),
    /// Wert ausserhalb des Typs (z. B. u8 > 255).
    OutOfRange(&'static str, u32),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::BadMagic => write!(f, "keine TAKT-MIR-Datei"),
            FormatError::UnsupportedVersion(v) => write!(f, "Formatversion {v} ist neuer als dieser Leser"),
            FormatError::Truncated => write!(f, "Datei unvollstaendig"),
            FormatError::BadVarint => write!(f, "Varint zu lang"),
            FormatError::BadWire(w) => write!(f, "unbekannte Drahtart {w}"),
            FormatError::BadString(i) => write!(f, "Stringnummer {i} ausserhalb der Tabelle"),
            FormatError::BadUtf8 => write!(f, "String ist kein UTF-8"),
            FormatError::Missing(n, t) => write!(f, "{n}: Feld {t} fehlt"),
            FormatError::Duplicate(n, t) => write!(f, "{n}: Feld {t} mehrfach"),
            FormatError::WrongWire(n, t) => write!(f, "{n}: Feld {t} hat die falsche Drahtart"),
            FormatError::BadVariant(n, v) => write!(f, "{n}: unbekannte Variante {v}"),
            FormatError::OutOfRange(n, t) => write!(f, "{n}: Feld {t} ausserhalb des Wertebereichs"),
        }
    }
}

impl std::error::Error for FormatError {}

/// Ergebnis der Leseschicht.
pub type Result<T> = std::result::Result<T, FormatError>;

/// Zickzack-Kodierung vorzeichenbehafteter Zahlen.
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Umkehrung von `zigzag`.
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Schreibt ein Varint (LEB128, hoechstens 10 Bytes).
pub fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

/// Liest ein Varint; `pos` rueckt vor.
pub fn get_varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let b = *buf.get(*pos).ok_or(FormatError::Truncated)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(FormatError::BadVarint)
}

/// `len` Bytes ab `pos`, ohne Ueberlauf der Indexrechnung.
pub fn slice(buf: &[u8], pos: usize, len: u64) -> Result<&[u8]> {
    let len = usize::try_from(len).map_err(|_| FormatError::Truncated)?;
    let end = pos.checked_add(len).ok_or(FormatError::Truncated)?;
    buf.get(pos..end).ok_or(FormatError::Truncated)
}

/// Schreiber: verschachtelte Knoten ueber einen Pufferstapel, Strings
/// einmal in der Tabelle. `logic_only` laesst Bindungen, Metadaten und
/// Positionen weg (Logik-Hash, 11.3).
pub struct Writer {
    stack: Vec<Vec<u8>>,
    strings: Vec<String>,
    string_ids: HashMap<String, u32>,
    /// Nur die Logik schreiben.
    pub logic_only: bool,
}

impl Writer {
    /// Neuer Schreiber.
    pub fn new(logic_only: bool) -> Self {
        Writer { stack: vec![Vec::new()], strings: Vec::new(), string_ids: HashMap::new(), logic_only }
    }

    fn buf(&mut self) -> &mut Vec<u8> {
        self.stack.last_mut().expect("Pufferstapel")
    }

    fn key(&mut self, tag: u32, wire: Wire) {
        put_varint(self.buf(), (u64::from(tag) << 2) | wire as u64);
    }

    /// Varint-Feld.
    pub fn varint(&mut self, tag: u32, v: u64) {
        self.key(tag, Wire::Varint);
        put_varint(self.buf(), v);
    }

    /// Vorzeichenbehaftetes Feld (Zickzack).
    pub fn signed(&mut self, tag: u32, v: i64) {
        self.varint(tag, zigzag(v));
    }

    /// Feld mit 8 festen Bytes.
    pub fn fixed64(&mut self, tag: u32, v: u64) {
        self.key(tag, Wire::Fixed64);
        self.buf().extend_from_slice(&v.to_le_bytes());
    }

    /// Bytefeld.
    pub fn bytes(&mut self, tag: u32, b: &[u8]) {
        self.key(tag, Wire::Bytes);
        put_varint(self.buf(), b.len() as u64);
        self.buf().extend_from_slice(b);
    }

    /// Stringfeld: Nummer in der Tabelle.
    pub fn string(&mut self, tag: u32, s: &str) {
        let id = match self.string_ids.get(s) {
            Some(&id) => id,
            None => {
                let id = self.strings.len() as u32;
                self.strings.push(s.to_string());
                self.string_ids.insert(s.to_string(), id);
                id
            }
        };
        self.varint(tag, u64::from(id));
    }

    /// Beginnt einen Knoten als Feld `tag`; `end` schliesst ihn.
    pub fn begin(&mut self) {
        self.stack.push(Vec::new());
    }

    /// Schliesst den mit `begin` eroeffneten Knoten und schreibt ihn als Feld.
    pub fn end(&mut self, tag: u32) {
        let node = self.stack.pop().expect("begin vor end");
        self.bytes(tag, &node);
    }

    /// Liefert Stringtabelle und Rumpf.
    pub fn finish(mut self) -> (Vec<String>, Vec<u8>) {
        assert_eq!(self.stack.len(), 1, "offene Knoten");
        (self.strings, self.stack.pop().expect("Rumpf"))
    }
}

/// Ein gelesenes Feld.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raw<'a> {
    /// Varint.
    Varint(u64),
    /// 8 Bytes.
    Fixed64(u64),
    /// Bytes (Knoten oder Bytefolge).
    Bytes(&'a [u8]),
}

/// Ein Knoten: seine Felder in Dateireihenfolge.
#[derive(Clone, Debug)]
pub struct Node<'a> {
    /// Name des Knotentyps fuer Fehlermeldungen.
    pub name: &'static str,
    fields: Vec<(u32, Raw<'a>)>,
}

impl<'a> Node<'a> {
    /// Zerlegt Bytes in Felder; unbekannte Nummern bleiben einfach unbenutzt.
    pub fn parse(name: &'static str, buf: &'a [u8]) -> Result<Self> {
        let mut pos = 0;
        let mut fields = Vec::new();
        while pos < buf.len() {
            let key = get_varint(buf, &mut pos)?;
            let tag = (key >> 2) as u32;
            let raw = match (key & 3) as u8 {
                0 => Raw::Varint(get_varint(buf, &mut pos)?),
                1 => {
                    let bytes = slice(buf, pos, 8)?;
                    pos += 8;
                    Raw::Fixed64(u64::from_le_bytes(bytes.try_into().expect("8 Bytes")))
                }
                2 => {
                    let len = get_varint(buf, &mut pos)?;
                    let bytes = slice(buf, pos, len)?;
                    pos += bytes.len();
                    Raw::Bytes(bytes)
                }
                w => return Err(FormatError::BadWire(w)),
            };
            fields.push((tag, raw));
        }
        Ok(Node { name, fields })
    }

    /// Alle Werte eines Felds.
    pub fn all(&self, tag: u32) -> impl Iterator<Item = Raw<'a>> + '_ {
        self.fields.iter().filter(move |(t, _)| *t == tag).map(|(_, r)| *r)
    }

    /// Genau ein Wert.
    pub fn one(&self, tag: u32) -> Result<Raw<'a>> {
        let mut it = self.all(tag);
        let first = it.next().ok_or(FormatError::Missing(self.name, tag))?;
        if it.next().is_some() {
            return Err(FormatError::Duplicate(self.name, tag));
        }
        Ok(first)
    }

    /// Hoechstens ein Wert.
    pub fn opt(&self, tag: u32) -> Result<Option<Raw<'a>>> {
        let mut it = self.all(tag);
        let first = it.next();
        if first.is_some() && it.next().is_some() {
            return Err(FormatError::Duplicate(self.name, tag));
        }
        Ok(first)
    }
}

/// Leser: Stringtabelle des Kopfes.
pub struct Reader {
    strings: Vec<String>,
}

impl Reader {
    /// Leser mit Stringtabelle.
    pub fn new(strings: Vec<String>) -> Self {
        Reader { strings }
    }

    /// String zu einer Nummer.
    pub fn string(&self, id: u64) -> Result<&str> {
        let i = u32::try_from(id).map_err(|_| FormatError::BadString(u32::MAX))?;
        self.strings.get(i as usize).map(String::as_str).ok_or(FormatError::BadString(i))
    }
}
