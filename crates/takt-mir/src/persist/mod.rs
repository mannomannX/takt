//! Der Typ-Hash einer `persist`-Variablen (5.9, Pruefung 23).
//!
//! Ein gespeicherter Wert ueberlebt Firmware-Grenzen; der Hash entscheidet,
//! ob er danach noch bedeutet, was er bedeutete (5.9: ungueltiger Typ-Hash
//! ergibt den Default plus Alert `PersistReset`).
//!
//! Es fliesst alles ein, was die Bedeutung eines gespeicherten Bytes
//! aendert: Typstruktur, Breiten, Einheiten, Ranges, Feld- und
//! Variantennamen, Diskriminanten — dazu der Schluessel (Maschine, Scope,
//! Variable). Lieber einmal zu viel verworfen als zu wenig: ein
//! faelschlich behaltener Wert wird still falsch gelesen, ein faelschlich
//! verworfener meldet sich.
//!
//! SHA-256 statt des FNV-1a aus 3.9: hier zaehlt Kollisionsfreiheit ueber
//! Firmware-Staende, nicht Geschwindigkeit.

use crate::program::Program;
use crate::types::{Const, Range, Type};
use crate::{EnumId, RecordId, TypeId, UnitId};

/// POD im Sinne von 5.9: „Skalare, Records, Arrays, Enums; keine Streams,
/// Bloecke oder Optionale."
///
/// `map<K, V, N>` ist erlaubt, wenn K und V POD sind (3.9).
pub fn is_pod(p: &Program, ty: TypeId) -> bool {
    pod_at(p, ty, 0)
}

/// Hat das Programm ueberhaupt `persist`-Variablen?
pub fn any(p: &Program) -> bool {
    p.machines.iter().any(|m| !m.persist.is_empty())
}

/// Der Schreibabstand des Programms in Nanosekunden (5.9).
///
/// Ein Slot traegt alle Variablen, also gilt das Minimum ueber die
/// deklarierten `min_interval`: Die engste Zusage bindet. Ohne jede
/// Deklaration entscheidet das Ziel (`nvm_min_interval` in 8.10), und
/// `None` heisst genau das.
pub fn min_interval_ns(p: &Program) -> Option<i64> {
    p.machines.iter().flat_map(|m| m.persist.iter()).filter_map(|pv| pv.min_interval).min()
}

/// Was ein Journal-Schreibvorgang den Tick kostet (12.3, Pruefung 32).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalCost {
    /// Ein ganzer Schreibvorgang in Nanosekunden.
    pub write_ns: i64,
    /// Die laengste Phase; so lange haelt `Nvm::begin_*` hoechstens.
    pub phase_ns: i64,
    /// Perioden zu T₀, die ein Schreibvorgang kostet.
    pub periods: u64,
}

/// Die Kosten auf einem blockierenden Ziel; `None` ohne `persist`, ohne
/// Blockieren oder ohne Zeiten.
pub fn journal_cost(p: &Program, nvm: &crate::hardware::NvmGeometry) -> Option<JournalCost> {
    if !any(p) {
        return None;
    }
    let write_ns = nvm.blocking_write_ns()?;
    let tick = p.config.tick.max(1);
    Some(JournalCost {
        write_ns,
        phase_ns: nvm.blocking_phase_ns()?,
        periods: u64::try_from(write_ns.saturating_add(tick - 1) / tick).unwrap_or(0),
    })
}

/// Obere Schranke der Journal-Nutzlast in Byte (11.5).
///
/// Je Eintrag acht Byte Typ-Hash, vier Byte Laenge und die kodierte Form.
/// `None`, wenn ein Typ keine Byte-Form hat — SC-23 hat das dann schon
/// gemeldet.
pub fn max_payload(p: &Program) -> Option<u32> {
    let mut total = 0u32;
    for m in &p.machines {
        for pv in &m.persist {
            let ty = m.vars.get(pv.var.index())?.ty;
            total = total.checked_add(12)?.checked_add(crate::bytes::max_size(p, ty).ok()?)?;
        }
    }
    Some(total)
}

fn pod_at(p: &Program, ty: TypeId, depth: u32) -> bool {
    if depth > 32 {
        return false;
    }
    let Some(t) = p.types.list.get(ty.index()) else { return false };
    match t {
        Type::Bool | Type::Int { .. } | Type::Float { .. } | Type::Duration { .. } => true,
        // 3.7 nennt `vec` und `bytes` gemeinsam als POD mit Standardwert;
        // beide sind laengenbeschraenkte Puffer fester Kapazitaet.
        Type::Bytes { .. } | Type::Str { .. } => true,
        Type::Vec { elem, .. } => pod_at(p, *elem, depth + 1),
        Type::Enum(e) => p
            .enums
            .get(e.index())
            .is_some_and(|d| d.variants.iter().all(|v| v.fields.iter().all(|f| pod_at(p, f.ty, depth + 1)))),
        Type::Record(r) => {
            p.records.get(r.index()).is_some_and(|d| d.fields.iter().all(|f| pod_at(p, f.ty, depth + 1)))
        }
        Type::Array { elem, .. } => pod_at(p, *elem, depth + 1),
        Type::Map { key, value, .. } => pod_at(p, *key, depth + 1) && pod_at(p, *value, depth + 1),
        // `line<N>` bleibt draussen: Es traegt mit `truncated` ein
        // Zustandsbit, das 3.7 bei den POD-Standardwerten nicht nennt.
        _ => false,
    }
}

/// Rechnet den Typ-Hash einer `persist`-Variablen (5.9).
///
/// `scope` ist der Name der gescopten Instanz oder leer.
pub fn type_hash(p: &Program, machine: &str, scope: &str, var: &str, ty: TypeId) -> u64 {
    let mut out = Vec::new();
    for part in [machine, scope, var] {
        out.extend_from_slice(part.as_bytes());
        out.push(0);
    }
    write_type(p, ty, &mut out, 0);
    let full = crate::hash::sha256(&out);
    u64::from_le_bytes(full.0[..8].try_into().unwrap_or([0; 8]))
}

/// Kanonische Form eines Typs; `depth` deckelt geschachtelte Typen.
fn write_type(p: &Program, ty: TypeId, out: &mut Vec<u8>, depth: u32) {
    if depth > 32 {
        out.push(0xFF);
        return;
    }
    let Some(t) = p.types.list.get(ty.index()) else {
        out.push(0xFE);
        return;
    };
    match t {
        Type::Bool => out.push(1),
        Type::Int { width, unit, range } => {
            out.push(2);
            out.push(width.bits() as u8);
            out.push(u8::from(width.signed()));
            write_unit(p, *unit, out);
            write_range(range, out);
        }
        Type::Float { width, unit, range } => {
            out.push(3);
            out.push(if *width == crate::types::FloatWidth::F32 { 32 } else { 64 });
            write_unit(p, *unit, out);
            write_range(range, out);
        }
        Type::Duration { range } => {
            out.push(4);
            write_range(range, out);
        }
        Type::Enum(e) => {
            out.push(5);
            write_enum(p, *e, out, depth);
        }
        Type::Record(r) => {
            out.push(6);
            write_record(p, *r, out, depth);
        }
        Type::Array { elem, len } => {
            out.push(7);
            out.extend_from_slice(&len.to_le_bytes());
            write_type(p, *elem, out, depth + 1);
        }
        Type::Bytes { cap } => {
            out.push(8);
            out.extend_from_slice(&cap.to_le_bytes());
        }
        Type::Str { cap } => {
            out.push(9);
            out.extend_from_slice(&cap.to_le_bytes());
        }
        Type::Vec { elem, cap } => {
            out.push(10);
            out.extend_from_slice(&cap.to_le_bytes());
            write_type(p, *elem, out, depth + 1);
        }
        Type::Map { key, value, cap } => {
            out.push(11);
            out.extend_from_slice(&cap.to_le_bytes());
            write_type(p, *key, out, depth + 1);
            write_type(p, *value, out, depth + 1);
        }
        // Kein POD; SC-23 lehnt das ab, bevor ein solcher Typ je einen
        // Schluessel braucht. Der Diskriminant genuegt hier: Das
        // `Debug`-Format traegt `TypeId`-Nummern, und die haengen an der
        // Internierungsreihenfolge, also am Rest des Programms.
        Type::Line { .. } => out.push(12),
        Type::Samples { .. } => out.push(13),
        Type::Table { .. } => out.push(14),
        Type::Mat { .. } => out.push(15),
        Type::Optional(_) => out.push(16),
        Type::Result { .. } => out.push(17),
        Type::Stream(_) => out.push(18),
        Type::Capture { .. } => out.push(19),
        Type::Handle(_) => out.push(20),
    }
}

/// Name, Dimension und Faktor der Einheit — nicht ihre Nummer, die haengt
/// an der Deklarationsreihenfolge.
///
/// Der Faktor gehoert dazu: Wird `unit spam = 2 m` zu `= 3 m`, bedeutet ein
/// gespeichertes `5 spam` danach 15 m statt 10 m (3.2).
fn write_unit(p: &Program, unit: Option<UnitId>, out: &mut Vec<u8>) {
    let Some(u) = unit.and_then(|u| p.units.get(u.index())) else {
        out.push(0);
        return;
    };
    out.extend_from_slice(u.name.as_bytes());
    out.push(0);
    for d in u.dimension {
        out.push(d as u8);
    }
    out.extend_from_slice(&u.factor.num.to_le_bytes());
    out.extend_from_slice(&u.factor.den.to_le_bytes());
    match u.affine_offset {
        Some(o) => {
            out.push(1);
            out.extend_from_slice(&o.num.to_le_bytes());
            out.extend_from_slice(&o.den.to_le_bytes());
        }
        None => out.push(0),
    }
}

/// Die Grenzen einer Range; sie gehoeren zur Bedeutung (3.4). Die Herkunft
/// nicht: `Proven` und `Declared` beschreiben denselben Wertebereich.
fn write_range(range: &Option<Range>, out: &mut Vec<u8>) {
    let Some(r) = range else {
        out.push(0);
        return;
    };
    out.push(1);
    for c in [r.lo, r.hi] {
        match c {
            Const::Int(v) => {
                out.push(1);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Const::Float(v) => {
                out.push(2);
                out.extend_from_slice(&v.to_bits().to_le_bytes());
            }
            Const::Duration(v) => {
                out.push(3);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Const::Bool(v) => {
                out.push(4);
                out.push(u8::from(v));
            }
        }
    }
}

/// Name, Drahtbreite, Varianten und Diskriminanten eines Enums.
///
/// Die Variantenzahl steht vor der Liste: Ohne sie kann eine geschachtelte
/// Variante die Bytes der naechsten Variante des aeusseren Enums schlucken,
/// und zwei verschiedene Typen bekommen denselben Schluessel.
fn write_enum(p: &Program, e: EnumId, out: &mut Vec<u8>, depth: u32) {
    let Some(def) = p.enums.get(e.index()) else { return };
    out.extend_from_slice(def.name.as_bytes());
    out.push(0);
    // `layout u8` ist die Byte-Breite des gespeicherten Werts (3.7).
    out.push(def.layout.map_or(0, |w| w.bits() as u8));
    out.extend_from_slice(&(def.variants.len() as u32).to_le_bytes());
    for v in &def.variants {
        out.extend_from_slice(v.name.as_bytes());
        out.push(0);
        out.extend_from_slice(&v.discriminant.to_le_bytes());
        out.extend_from_slice(&(v.fields.len() as u32).to_le_bytes());
        for f in &v.fields {
            write_type(p, f.ty, out, depth + 1);
        }
    }
}

/// Name und Felder eines Records, in Deklarationsreihenfolge.
fn write_record(p: &Program, r: RecordId, out: &mut Vec<u8>, depth: u32) {
    let Some(def) = p.records.get(r.index()) else { return };
    out.extend_from_slice(def.name.as_bytes());
    out.push(0);
    out.extend_from_slice(&(def.fields.len() as u32).to_le_bytes());
    for f in &def.fields {
        out.extend_from_slice(f.name.as_bytes());
        out.push(0);
        write_type(p, f.ty, out, depth + 1);
    }
}
