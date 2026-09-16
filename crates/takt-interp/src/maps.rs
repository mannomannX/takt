//! `map<K, V, N>` im Interpreter (3.9): Die Slots tragen Werte, die
//! Sondierlogik steht in `takt_native::map` — dieselbe wie im erzeugten
//! Code. Ein Schluessel ist fuer Hash und Vergleich seine auf `K` Byte
//! aufgefuellte kanonische Byteform (5.9).

use takt_mir::TypeId;
use takt_mir::program::Program;
use takt_native::map::{self as native, Slots};

use crate::value::{EvalResult, Trap, Value};

/// Ein Slot: leer oder `(Schluessel, Wert)`.
pub type Slot = Option<(Value, Value)>;

struct ValueSlots<'a> {
    p: &'a Program,
    key: TypeId,
    klen: usize,
    slots: &'a mut [Slot],
}

/// Die aufgefuellte Byteform eines Schluessels.
fn key_bytes(p: &Program, key: TypeId, klen: usize, v: &Value) -> EvalResult<Vec<u8>> {
    let mut b =
        crate::bytes::encode(p, v, key).map_err(|e| Trap::Bug(format!("map-Schluessel ohne Byteform ({e:?})")))?;
    b.resize(klen, 0);
    Ok(b)
}

impl Slots for ValueSlots<'_> {
    fn cap(&self) -> usize {
        self.slots.len()
    }

    fn occupied(&self, i: usize) -> bool {
        self.slots.get(i).is_some_and(Option::is_some)
    }

    fn hash_at(&self, i: usize) -> u32 {
        match self.slots.get(i) {
            Some(Some((k, _))) => key_bytes(self.p, self.key, self.klen, k).map(|b| native::fnv1a(&b)).unwrap_or(0),
            _ => 0,
        }
    }

    fn matches(&self, i: usize, key: &[u8]) -> bool {
        match self.slots.get(i) {
            Some(Some((k, _))) => key_bytes(self.p, self.key, self.klen, k).is_ok_and(|b| b == key),
            _ => false,
        }
    }

    fn move_slot(&mut self, from: usize, to: usize) {
        let entry = self.slots[from].take();
        self.slots[to] = entry;
    }

    fn clear(&mut self, i: usize) {
        self.slots[i] = None;
    }
}

fn klen(p: &Program, key: TypeId) -> EvalResult<usize> {
    takt_mir::bytes::max_size(p, key)
        .map(|n| n as usize)
        .map_err(|e| Trap::Bug(format!("map-Schluessel ohne Byteform ({e:?})")))
}

/// `m.insert(k, v) -> bool`: ersetzt einen vorhandenen Wert, `false` bei
/// voller Map (3.9).
pub fn insert(p: &Program, key_ty: TypeId, slots: &mut [Slot], key: Value, value: Value) -> EvalResult<bool> {
    let klen = klen(p, key_ty)?;
    let bytes = key_bytes(p, key_ty, klen, &key)?;
    let hash = native::fnv1a(&bytes);
    let s = ValueSlots { p, key: key_ty, klen, slots };
    let i = match native::find(&s, hash, &bytes) {
        Some(i) => i,
        None => match native::free_slot(&s, hash) {
            Some(i) => i,
            None => return Ok(false),
        },
    };
    s.slots[i] = Some((key, value));
    Ok(true)
}

/// `m.get(k) -> V?`.
pub fn get(p: &Program, key_ty: TypeId, slots: &mut [Slot], key: &Value) -> EvalResult<Value> {
    let klen = klen(p, key_ty)?;
    let bytes = key_bytes(p, key_ty, klen, key)?;
    let s = ValueSlots { p, key: key_ty, klen, slots };
    let found = native::find(&s, native::fnv1a(&bytes), &bytes);
    Ok(Value::Optional(found.and_then(|i| s.slots[i].as_ref()).map(|(_, v)| Box::new(v.clone()))))
}

/// `m.remove(k) -> bool`: Rueckwaertsverschiebung, keine Grabsteine.
pub fn remove(p: &Program, key_ty: TypeId, slots: &mut [Slot], key: &Value) -> EvalResult<bool> {
    let klen = klen(p, key_ty)?;
    let bytes = key_bytes(p, key_ty, klen, key)?;
    let mut s = ValueSlots { p, key: key_ty, klen, slots };
    let Some(i) = native::find(&s, native::fnv1a(&bytes), &bytes) else { return Ok(false) };
    native::remove_at(&mut s, i);
    Ok(true)
}
