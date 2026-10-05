//! Typen einer `persist`-Variablen (5.9): Der Typ-Hash schreibt die
//! kanonische Form bis zur Tiefe 32 und kappt darunter; dass dort keine
//! Kollision droht, sichert `is_pod`, das tiefere Typen ablehnt.

use takt_mir::TypeId;
use takt_mir::persist::{is_pod, type_hash};
use takt_mir::program::{Config, Program};
use takt_mir::types::{IntWidth, Type};

/// `levels`-fach geschachtelte Arrays um `inner`.
fn nested(p: &mut Program, inner: TypeId, levels: u32) -> TypeId {
    (0..levels).fold(inner, |t, _| p.types.intern(Type::Array { elem: t, len: 1 }))
}

/// SYN-030: Tiefe 32 ist POD und hasht vollstaendig — zwei Typen, die
/// sich erst im innersten Element unterscheiden, haben verschiedene
/// Hashes; Tiefe 33 ist kein POD, also nie `persist`.
#[test]
fn the_type_hash_reaches_every_level_a_persist_type_can_have() {
    let mut p = Program::new(Config::new(1, 1_000_000));
    let int = p.types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
    let byte = p.types.intern(Type::Int { width: IntWidth::U8, unit: None, range: None });
    let (deep_int, deep_byte) = (nested(&mut p, int, 32), nested(&mut p, byte, 32));
    assert!(is_pod(&p, deep_int) && is_pod(&p, deep_byte), "Tiefe 32 ist POD");
    assert_ne!(
        type_hash(&p, "m", "", "v", deep_int),
        type_hash(&p, "m", "", "v", deep_byte),
        "das innerste Element der Tiefe 32 geht in den Hash ein"
    );
    let too_deep = nested(&mut p, int, 33);
    assert!(!is_pod(&p, too_deep), "Tiefe 33 ist kein POD und damit nie `persist`");
}
