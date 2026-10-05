//! Der nichtfluechtige Speicher hinter `persist var` (5.9).
//!
//! Im Interpreter steht er als Abbildung Typ-Hash → Wert. Die Byte-Form,
//! das Journal und `min_interval` gehoeren der Runtime; 5.9 nennt das
//! Schreiben ausdruecklich „Beobachtung … ausserhalb der Semantik". Fuer
//! die Semantik zaehlt nur, welche Werte s0 vorfindet.

use std::collections::HashMap;

use takt_mir::TypeId;
use takt_mir::program::Program;
use takt_mir::types::Type;

use crate::eval::in_range;
use crate::value::Value;

/// Was beim Laden einer `persist`-Variablen geschah (5.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// Ein gueltiger Wert lag vor.
    Loaded,
    /// Kein Eintrag: der Default gilt, ohne Alert — der erste Start eines
    /// Geraets ist kein Fehler.
    Absent,
    /// Eintrag vorhanden, aber unbrauchbar: Default plus `PersistReset`.
    Reset(Reason),
}

/// Warum ein Eintrag verworfen wurde.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Der Wert passt nicht zur Form des Typs.
    Shape,
    /// Der Wert liegt ausserhalb der deklarierten Range (3.4).
    Range,
}

impl Reason {
    /// Text fuer den Alert.
    pub fn text(self) -> &'static str {
        match self {
            Reason::Shape => "Typ",
            Reason::Range => "Range",
        }
    }
}

/// Inhalt des nichtfluechtigen Speichers, adressiert ueber den Typ-Hash
/// (5.9: „Schluessel = Maschine.Variable plus Typ-Hash").
///
/// Ein Eintrag unter einem Hash, den kein Programm mehr kennt, wird beim
/// Laden nie gefunden — genau die Wirkung, die der Hash haben soll.
#[derive(Clone, Debug, Default)]
pub struct Nvm {
    entries: HashMap<u64, Value>,
    /// Eintraege, deren Bytes keine kanonische Form ihres Typs sind: Sie
    /// sind vorhanden und ungueltig, nicht fehlend (5.9). Die Bytes bleiben,
    /// damit eine Aufzeichnung genau diesen Speicher wiedergibt (12.5).
    undecodable: HashMap<u64, Vec<u8>>,
}

impl Nvm {
    /// Leerer Speicher: jeder Start ist der erste.
    pub fn new() -> Nvm {
        Nvm::default()
    }

    /// Legt einen Wert ab.
    pub fn put(&mut self, type_hash: u64, value: Value) {
        self.undecodable.remove(&type_hash);
        self.entries.insert(type_hash, value);
    }

    /// Liest einen Wert.
    pub fn get(&self, type_hash: u64) -> Option<&Value> {
        self.entries.get(&type_hash)
    }

    /// Leer?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.undecodable.is_empty()
    }

    /// Holt den Wert fuer eine `persist`-Variable und prueft ihn (5.9).
    pub fn load(&self, p: &Program, type_hash: u64, ty: TypeId) -> (Load, Option<Value>) {
        if self.undecodable.contains_key(&type_hash) {
            return (Load::Reset(Reason::Shape), None);
        }
        let Some(v) = self.entries.get(&type_hash) else { return (Load::Absent, None) };
        match validate(p, v, ty) {
            Ok(()) => (Load::Loaded, Some(v.clone())),
            Err(r) => (Load::Reset(r), None),
        }
    }

    /// Der ganze Speicher als Nutzlast in der Form des Journals, nach
    /// Typ-Hash sortiert: s0 fuer den Kopf einer Aufzeichnung (12.5).
    /// `from_program_payload` liest ihn zurueck. Ein Wert, den sein Typ
    /// nicht kodiert, steht ohne Bytes da und laedt wieder als ungueltig.
    pub fn snapshot(&self, p: &Program) -> Vec<u8> {
        let types: HashMap<u64, TypeId> = persist_types(p);
        let mut all: Vec<(u64, Vec<u8>)> = self
            .entries
            .iter()
            .map(|(h, v)| {
                let bytes = types.get(h).and_then(|ty| crate::bytes::encode(p, v, *ty).ok()).unwrap_or_default();
                (*h, bytes)
            })
            .chain(self.undecodable.iter().map(|(h, b)| (*h, b.clone())))
            .collect();
        all.sort_by_key(|(h, _)| *h);
        let mut out = Vec::new();
        for (hash, bytes) in all {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Die Nutzlast eines Journal-Slots aus dem Programmzustand.
    ///
    /// Eintraege stehen nach Typ-Hash sortiert: Der Vergleich in
    /// `Journal::poll` arbeitet byteweise, also muessen gleiche Werte
    /// gleiche Bytes ergeben.
    pub fn payload(p: &Program, values: &[(u64, &Value, TypeId)]) -> Option<Vec<u8>> {
        let mut sorted: Vec<&(u64, &Value, TypeId)> = values.iter().collect();
        sorted.sort_by_key(|(h, _, _)| *h);
        let mut out = Vec::new();
        for (hash, value, ty) in sorted {
            let bytes = crate::bytes::encode(p, value, *ty).ok()?;
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&bytes);
        }
        Some(out)
    }

    /// Liest eine Journal-Nutzlast in den Speicher; die Typen kommen aus
    /// dem Programm.
    pub fn from_program_payload(&mut self, p: &Program, bytes: &[u8]) {
        self.from_payload(p, bytes, &persist_types(p));
    }

    /// Liest eine Journal-Nutzlast in den Speicher.
    ///
    /// Ein Eintrag, dessen Typ-Hash kein Programm mehr kennt, wird
    /// uebersprungen — genau die Wirkung, die der Hash haben soll. Ein
    /// Eintrag, dessen Bytes sich nicht dekodieren lassen (`bool`-Byte 2,
    /// fremde Diskriminante), ist dagegen ungueltig: Default plus
    /// `PersistReset` (5.9), wie im erzeugten Restore. Ein verstuemmelter
    /// Rest beendet das Lesen, ohne das Bisherige zu verwerfen: Der CRC hat
    /// den Eintrag schon bestaetigt.
    pub fn from_payload(&mut self, p: &Program, bytes: &[u8], types: &HashMap<u64, TypeId>) {
        let mut at = 0;
        while at + 12 <= bytes.len() {
            let hash = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap_or([0; 8]));
            let len = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap_or([0; 4])) as usize;
            at += 12;
            let Some(slice) = bytes.get(at..at + len) else { return };
            at += len;
            if let Some(ty) = types.get(&hash) {
                match crate::bytes::decode(p, slice, *ty) {
                    Ok(v) => {
                        self.undecodable.remove(&hash);
                        self.entries.insert(hash, v);
                    }
                    Err(_) => {
                        self.entries.remove(&hash);
                        self.undecodable.insert(hash, slice.to_vec());
                    }
                }
            }
        }
    }
}

/// Typ-Hash und Typ jeder `persist`-Variablen des Programms.
fn persist_types(p: &Program) -> HashMap<u64, TypeId> {
    p.machines.iter().flat_map(|m| m.persist.iter().map(|pv| (pv.type_hash, m.vars[pv.var.index()].ty))).collect()
}

/// Passt ein gespeicherter Wert zu seinem Typ? Nur POD-Typen; alles andere
/// hat SC-23 schon abgelehnt.
fn validate(p: &Program, v: &Value, ty: TypeId) -> Result<(), Reason> {
    match (p.types.get(ty), v) {
        (Type::Bool, Value::Bool(_)) => Ok(()),
        (Type::Int { width, range, .. }, Value::Int(_) | Value::UInt(_)) => {
            let signed = matches!(v, Value::Int(_));
            if signed != width.signed() {
                return Err(Reason::Shape);
            }
            // Die Breite ist die erste Schranke: `u8` ohne `in a..b` traegt
            // `range: None`, und ein Wert daneben waere sonst ein Fault im
            // Tick 0 statt eines Alerts (3.1, 5.9).
            let x = match v {
                Value::Int(n) => i128::from(*n),
                Value::UInt(n) => i128::from(*n),
                _ => return Err(Reason::Shape),
            };
            let (lo, hi) = crate::arith::bounds(*width);
            if x < lo || x > hi {
                return Err(Reason::Range);
            }
            ranged(v, range)
        }
        (Type::Float { width, range, .. }, Value::F32(_) | Value::F64(_)) => {
            let is32 = matches!(v, Value::F32(_));
            if is32 != (*width == takt_mir::types::FloatWidth::F32) {
                return Err(Reason::Shape);
            }
            // 5.9: NaN und Inf sind keine gueltige Byteform, auch ohne Range;
            // sonst kaeme ein nicht endlicher Wert in die Sprache (4.1).
            let finite = match v {
                Value::F32(x) => x.is_finite(),
                Value::F64(x) => x.is_finite(),
                _ => false,
            };
            if !finite {
                return Err(Reason::Range);
            }
            ranged(v, range)
        }
        (Type::Duration { range }, Value::Duration(_)) => ranged(v, range),
        (Type::Enum(id), Value::Enum { variant, fields }) => {
            let def = p.enums.get(id.index()).ok_or(Reason::Shape)?;
            let var = def.variants.get(*variant as usize).ok_or(Reason::Shape)?;
            if var.fields.len() != fields.len() {
                return Err(Reason::Shape);
            }
            for (f, value) in var.fields.iter().zip(fields) {
                validate(p, value, f.ty)?;
            }
            Ok(())
        }
        (Type::Record(id), Value::Record(fields)) => {
            let def = p.records.get(id.index()).ok_or(Reason::Shape)?;
            if def.fields.len() != fields.len() {
                return Err(Reason::Shape);
            }
            for (f, value) in def.fields.iter().zip(fields) {
                validate(p, value, f.ty)?;
            }
            Ok(())
        }
        (Type::Array { elem, len }, Value::Array(items)) => {
            if items.len() != *len as usize {
                return Err(Reason::Shape);
            }
            items.iter().try_for_each(|i| validate(p, i, *elem))
        }
        (Type::Vec { elem, cap }, Value::Vec(items)) => {
            if items.len() > *cap as usize {
                return Err(Reason::Shape);
            }
            items.iter().try_for_each(|i| validate(p, i, *elem))
        }
        (Type::Bytes { cap }, Value::Bytes(b)) => (b.len() <= *cap as usize).then_some(()).ok_or(Reason::Shape),
        (Type::Str { cap }, Value::Str(s)) => (s.chars().count() <= *cap as usize).then_some(()).ok_or(Reason::Shape),
        (Type::Map { key, value, cap }, Value::Map(slots)) => {
            if slots.len() != *cap as usize {
                return Err(Reason::Shape);
            }
            slots.iter().flatten().try_for_each(|(k, val)| validate(p, k, *key).and_then(|()| validate(p, val, *value)))
        }
        _ => Err(Reason::Shape),
    }
}

fn ranged(v: &Value, range: &Option<takt_mir::types::Range>) -> Result<(), Reason> {
    match range {
        Some(r) if !in_range(v, r) => Err(Reason::Range),
        _ => Ok(()),
    }
}
