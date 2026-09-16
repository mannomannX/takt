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

/// Der Slot des Schluessels, wenn er in der Map steht.
pub fn find(s: &impl Slots, hash: u32, key: &[u8]) -> Option<usize> {
    let cap = s.cap();
    if cap == 0 {
        return None;
    }
    let mut i = (hash as usize) % cap;
    for _ in 0..cap {
        if !s.occupied(i) {
            return None;
        }
        if s.hash_at(i) == hash && s.matches(i, key) {
            return Some(i);
        }
        i = (i + 1) % cap;
    }
    None
}

/// Der erste freie Slot ab der Hash-Position; `None`, wenn die Map voll ist.
pub fn free_slot(s: &impl Slots, hash: u32) -> Option<usize> {
    let cap = s.cap();
    if cap == 0 {
        return None;
    }
    let mut i = (hash as usize) % cap;
    for _ in 0..cap {
        if !s.occupied(i) {
            return Some(i);
        }
        i = (i + 1) % cap;
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
        j = (j + 1) % cap;
        if !s.occupied(j) {
            return;
        }
        let home = (s.hash_at(j) as usize) % cap;
        // `home` liegt zyklisch in (hole, j]: der Eintrag bleibt.
        let stays = if hole <= j { home > hole && home <= j } else { home > hole || home <= j };
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
        let cap = cap.min(data.len() / (1 + klen + vlen).max(1));
        ByteMap { data, cap, klen, vlen }
    }

    fn slot(&self, i: usize) -> usize {
        i * (1 + self.klen + self.vlen)
    }

    fn key_at(&self, i: usize) -> &[u8] {
        let at = self.slot(i) + 1;
        &self.data[at..at + self.klen]
    }

    fn value_at(&self, i: usize) -> &[u8] {
        let at = self.slot(i) + 1 + self.klen;
        &self.data[at..at + self.vlen]
    }

    fn write(&mut self, i: usize, key: &[u8], value: &[u8]) {
        let at = self.slot(i);
        self.data[at] = 1;
        let k = &mut self.data[at + 1..at + 1 + self.klen];
        k.fill(0);
        let n = key.len().min(k.len());
        k[..n].copy_from_slice(&key[..n]);
        let at = at + 1 + self.klen;
        let v = &mut self.data[at..at + self.vlen];
        v.fill(0);
        let n = value.len().min(v.len());
        v[..n].copy_from_slice(&value[..n]);
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
        self.write(i, key, value);
        true
    }

    /// `get(k) -> V?`: der Wert in seiner aufgefuellten Form.
    pub fn get(&self, key: &[u8]) -> Option<&[u8]> {
        let i = find(self, self.hash(key), key)?;
        Some(self.value_at(i))
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
        (i < self.cap && self.occupied(i)).then(|| (self.key_at(i), self.value_at(i)))
    }
}

impl Slots for ByteMap<'_> {
    fn cap(&self) -> usize {
        self.cap
    }

    fn occupied(&self, i: usize) -> bool {
        self.data.get(self.slot(i)).is_some_and(|b| *b != 0)
    }

    fn hash_at(&self, i: usize) -> u32 {
        fnv1a(self.key_at(i))
    }

    fn matches(&self, i: usize, key: &[u8]) -> bool {
        let stored = self.key_at(i);
        stored.iter().enumerate().all(|(n, b)| *b == key.get(n).copied().unwrap_or(0))
    }

    fn move_slot(&mut self, from: usize, to: usize) {
        let size = 1 + self.klen + self.vlen;
        let (a, b) = (self.slot(from), self.slot(to));
        self.data.copy_within(a..a + size, b);
        self.data[a] = 0;
    }

    fn clear(&mut self, i: usize) {
        let at = self.slot(i);
        self.data[at] = 0;
    }
}
