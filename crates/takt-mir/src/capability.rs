//! Faehigkeiten von Typvariablen (3.12, v1.2).
//!
//! `fn f[type T: numeric]` verlangt vom konkreten Typ ein Praedikat. Sie
//! liegen in der MIR, weil sie Aussagen ueber Typen sind: Sema prueft sie
//! an der Aufrufstelle, der Interpreter sieht dieselben Instanzen.

use crate::types::Type;
use crate::{Program, TypeId};

/// Eine Faehigkeit im Sinne von 3.12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Werttyp fester Groesse ohne Handle.
    Pod,
    /// `pod` mit erklaerter Gleichheit.
    Eq,
    /// Geordnet.
    Ord,
    /// Ganzzahl oder Fliesskomma.
    Numeric,
    /// Ganzzahl.
    Integer,
    /// Fliesskomma.
    Float,
}

impl Capability {
    /// Name in Meldungen und im Instanznamen.
    pub fn name(self) -> &'static str {
        match self {
            Capability::Pod => "pod",
            Capability::Eq => "eq",
            Capability::Ord => "ord",
            Capability::Numeric => "numeric",
            Capability::Integer => "integer",
            Capability::Float => "float",
        }
    }

    /// Welche Typen sie tragen — die zweite Zeile einer Meldung.
    pub fn expects(self) -> &'static str {
        match self {
            Capability::Pod => "Werttypen fester Groesse: Skalare, Enums, Records, Arrays und Puffer daraus",
            Capability::Eq => "POD mit Gleichheit: Ganzzahlen, Enums, Records und Puffer daraus, kein Fliesskomma",
            Capability::Ord => "`int`, `float`, `time` und `bool`",
            Capability::Numeric => "`int` und `float`",
            Capability::Integer => "Ganzzahlen",
            Capability::Float => "Fliesskommazahlen",
        }
    }
}

/// Erfuellt `ty` die Faehigkeit?
pub fn holds(p: &Program, ty: TypeId, cap: Capability) -> bool {
    let Some(t) = p.types.list.get(ty.index()) else { return false };
    match cap {
        Capability::Pod => crate::persist::is_pod(p, ty),
        // „POD mit Gleichheit" ist dieselbe Menge, die 3.9 als
        // `map`-Schluessel zulaesst (Pruefung 57): Fliesskomma bleibt
        // draussen, weil `==` darauf keine brauchbare Zusage ist.
        Capability::Eq => crate::persist::is_pod(p, ty) && !contains_float(p, ty, 0),
        Capability::Ord => matches!(t, Type::Int { .. } | Type::Float { .. } | Type::Duration { .. } | Type::Bool),
        Capability::Numeric => matches!(t, Type::Int { .. } | Type::Float { .. }),
        Capability::Integer => matches!(t, Type::Int { .. }),
        Capability::Float => matches!(t, Type::Float { .. }),
    }
}

/// Traegt der Typ irgendwo ein `float`? (3.9, Pruefung 57.)
pub fn contains_float(p: &Program, ty: TypeId, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    let Some(t) = p.types.list.get(ty.index()) else { return false };
    let any = |list: Vec<TypeId>| list.into_iter().any(|f| contains_float(p, f, depth + 1));
    match t {
        Type::Float { .. } => true,
        Type::Optional(inner) | Type::Array { elem: inner, .. } | Type::Vec { elem: inner, .. } => {
            contains_float(p, *inner, depth + 1)
        }
        Type::Record(r) => p.records.get(r.index()).is_some_and(|d| any(d.fields.iter().map(|f| f.ty).collect())),
        Type::Enum(e) => p
            .enums
            .get(e.index())
            .is_some_and(|d| any(d.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)).collect())),
        Type::Map { key, value, .. } => contains_float(p, *key, depth + 1) || contains_float(p, *value, depth + 1),
        _ => false,
    }
}
