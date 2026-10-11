//! `map<K, V, N>` (3.9): offene Adressierung ueber ein festes Feld von
//! Slots, lineare Sondierung, Entfernen per Rueckwaertsverschiebung,
//! FNV-1a ueber die kanonische Byteform des Schluessels (5.9). Die eine
//! Implementierung fuer Interpreter und erzeugten Code — die
//! Iterationsreihenfolge ist die Slot-Reihenfolge und damit ueberall gleich
//! (Satz 9.4.4).
//!
//! Ein Slot ist `[belegt: u8][Schluessel: K Byte][Wert: V Byte]`, K und V
//! die oberen Schranken der Byteform (`bytes::max_size`); kuerzere Formen
//! sind mit Nullen aufgefuellt, gehasht und verglichen wird der ganze
//! Schluessel. Der Interpreter haelt Werte in seinen Slots und liefert die
//! Bytes je Vergleich; die Sondierung ist dieselbe.

use core::ops::Range;

/// FNV-1a, 32 Bit (3.9).
pub fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in bytes {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Die Slots einer Map, abstrakt: der Interpreter haelt Werte, der native
/// Code Bytes. Ein Schluessel ist ueberall seine aufgefuellte Byteform.
pub trait Slots {
    /// Zahl der Slots `N`.
    fn cap(&self) -> usize;
    /// Ist der Slot belegt?
    fn occupied(&self, i: usize) -> bool;
    /// Der Hash des Schluessels in Slot `i`.
    fn hash_at(&self, i: usize) -> u32;
    /// Traegt Slot `i` genau diesen Schluessel?
    fn matches(&self, i: usize, key: &[u8]) -> bool;
    /// Verschiebt den Eintrag von `from` nach `to` und leert `from`.
    fn move_slot(&mut self, from: usize, to: usize);
    /// Leert den Slot.
    fn clear(&mut self, i: usize);
}

/// Die Hash-Position in `cap` Slots; `None` ohne Slot.
fn home(hash: u32, cap: usize) -> Option<usize> {
    usize::try_from(hash).ok()?.checked_rem(cap)
}

/// Der Slot nach `i`, zyklisch; null ohne Slot.
fn next(i: usize, cap: usize) -> usize {
    i.saturating_add(1).checked_rem(cap).unwrap_or(0)
}

/// Der Slot des Schluessels, wenn er in der Map steht.
pub fn find(s: &impl Slots, hash: u32, key: &[u8]) -> Option<usize> {
    let cap = s.cap();
    let mut i = home(hash, cap)?;
    for _ in 0..cap {
        if !s.occupied(i) {
            return None;
        }
        if s.hash_at(i) == hash && s.matches(i, key) {
            return Some(i);
        }
        i = next(i, cap);
    }
    None
}

/// Der erste freie Slot ab der Hash-Position; `None`, wenn die Map voll ist.
pub fn free_slot(s: &impl Slots, hash: u32) -> Option<usize> {
    let cap = s.cap();
    let mut i = home(hash, cap)?;
    for _ in 0..cap {
        if !s.occupied(i) {
            return Some(i);
        }
        i = next(i, cap);
    }
    None
}

/// Entfernt Slot `i` per Rueckwaertsverschiebung (3.9): Jeder folgende
/// Eintrag, dessen Hash-Position nicht zwischen der Luecke und ihm liegt,
/// rueckt in die Luecke — so bleibt jede Sondierkette ohne Grabstein
/// geschlossen.
pub fn remove_at(s: &mut impl Slots, i: usize) {
    let cap = s.cap();
    if cap == 0 {
        return;
    }
    s.clear(i);
    let mut hole = i;
    let mut j = i;
    for _ in 0..cap {
        j = next(j, cap);
        if !s.occupied(j) {
            return;
        }
        let Some(at) = home(s.hash_at(j), cap) else { return };
        // `at` liegt zyklisch in (hole, j]: der Eintrag bleibt.
        let stays = if hole <= j { at > hole && at <= j } else { at > hole || at <= j };
        if stays {
            continue;
        }
        s.move_slot(j, hole);
        hole = j;
    }
}

/// Eine Map ueber Bytes: `N` Slots zu je `1 + K + V` Byte.
pub struct ByteMap<'a> {
    data: &'a mut [u8],
    cap: usize,
    klen: usize,
    vlen: usize,
}

impl<'a> ByteMap<'a> {
    /// Legt die Sicht ueber `data` an; `data` traegt mindestens `cap * (1 + klen + vlen)` Byte.
    pub fn new(data: &'a mut [u8], cap: usize, klen: usize, vlen: usize) -> ByteMap<'a> {
        let cap = cap.min(data.len().checked_div(slot_size(klen, vlen)).unwrap_or(0));
        ByteMap { data, cap, klen, vlen }
    }

    /// Wo Slot `i` in `data` liegt; `None`, wenn er nicht ganz hineinpasst.
    fn span(&self, i: usize) -> Option<Range<usize>> {
        let size = slot_size(self.klen, self.vlen);
        let at = i.checked_mul(size)?;
        let end = at.checked_add(size)?;
        (end <= self.data.len()).then_some(at..end)
    }

    /// Schluessel und Wert in Slot `i`.
    fn parts(&self, i: usize) -> Option<(&[u8], &[u8])> {
        let (_, rest) = self.data.get(self.span(i)?)?.split_first()?;
        rest.split_at_checked(self.klen)
    }

    fn key_at(&self, i: usize) -> Option<&[u8]> {
        self.parts(i).map(|(k, _)| k)
    }

    fn value_at(&self, i: usize) -> Option<&[u8]> {
        self.parts(i).map(|(_, v)| v)
    }

    /// Belegt Slot `i`; `false`, wenn es ihn nicht gibt.
    fn write(&mut self, i: usize, key: &[u8], value: &[u8]) -> bool {
        let klen = self.klen;
        let Some(slot) = self.span(i).and_then(|at| self.data.get_mut(at)) else { return false };
        let Some((occupied, rest)) = slot.split_first_mut() else { return false };
        let Some((k, v)) = rest.split_at_mut_checked(klen) else { return false };
        *occupied = 1;
        pad(k, key);
        pad(v, value);
        true
    }

    /// Der Hash eines Schluessels, wie ihn diese Map rechnet: ueber die auf
    /// `K` Byte aufgefuellte Form.
    pub fn hash(&self, key: &[u8]) -> u32 {
        let mut h = 0x811c_9dc5u32;
        for i in 0..self.klen {
            h ^= u32::from(key.get(i).copied().unwrap_or(0));
            h = h.wrapping_mul(0x0100_0193);
        }
        h
    }

    /// `insert(k, v) -> bool`: ersetzt einen vorhandenen Wert; `false`,
    /// wenn die Map voll ist (kein Fault, 3.9).
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> bool {
        let h = self.hash(key);
        let i = match find(self, h, key) {
            Some(i) => i,
            None => match free_slot(self, h) {
                Some(i) => i,
                None => return false,
            },
        };
        self.write(i, key, value)
    }

    /// `get(k) -> V?`: der Wert in seiner aufgefuellten Form.
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        let i = find(self, self.hash(key), key)?;
        self.value_at(i)
    }

    /// `remove(k) -> bool`.
    pub fn remove(&mut self, key: &[u8]) -> bool {
        let Some(i) = find(self, self.hash(key), key) else { return false };
        remove_at(self, i);
        true
    }

    /// `len`: die belegten Slots.
    pub fn len(&self) -> usize {
        (0..self.cap).filter(|&i| self.occupied(i)).count()
    }

    /// Ohne Eintrag?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Schluessel und Wert in Slot `i`, wenn er belegt ist — die
    /// Iterationsreihenfolge von `for (k, v) in m` (3.9).
    pub fn entry(&self, i: usize) -> Option<(&[u8], &[u8])> {
        if i >= self.cap || !self.occupied(i) {
            return None;
        }
        self.parts(i)
    }
}

impl Slots for ByteMap<'_> {
    fn cap(&self) -> usize {
        self.cap
    }

    fn occupied(&self, i: usize) -> bool {
        self.span(i).and_then(|at| self.data.get(at.start)).is_some_and(|b| *b != 0)
    }

    fn hash_at(&self, i: usize) -> u32 {
        fnv1a(self.key_at(i).unwrap_or_default())
    }

    fn matches(&self, i: usize, key: &[u8]) -> bool {
        self.key_at(i)
            .is_some_and(|stored| stored.iter().enumerate().all(|(n, b)| *b == key.get(n).copied().unwrap_or(0)))
    }

    fn move_slot(&mut self, from: usize, to: usize) {
        let (Some(src), Some(dst)) = (self.span(from), self.span(to)) else { return };
        let at = src.start;
        self.data.copy_within(src, dst.start);
        if let Some(b) = self.data.get_mut(at) {
            *b = 0;
        }
    }

    fn clear(&mut self, i: usize) {
        if let Some(b) = self.span(i).and_then(|at| self.data.get_mut(at.start)) {
            *b = 0;
        }
    }
}

/// Die Groesse eines Slots: das Byte fuer belegt, Schluessel und Wert.
fn slot_size(klen: usize, vlen: usize) -> usize {
    klen.saturating_add(vlen).saturating_add(1)
}

/// Schreibt `src` an den Anfang von `dst` und fuellt mit Nullen auf; was
/// nicht passt, faellt weg.
fn pad(dst: &mut [u8], src: &[u8]) {
    dst.fill(0);
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s;
    }
}
