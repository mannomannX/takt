//! Ereignisstroeme in der Runtime (Referenz 8.6, 9.6).
//!
//! **Was hier steht.** Der Puffer eines Stroms mit seinen drei
//! Operationen: einlagern (der Treiber liefert), das Fenster eines
//! Konsumenten lesen, und am Tickende freigeben, was niemand mehr sieht.
//! Die Regel dahinter ist eine einzige — „untersucht heisst konsumiert"
//! (8.6) —, und der Cursor gehoert der Maschine, nicht dem Strom.
//!
//! **Woher der Speicher kommt.** Nicht von hier. Der Kern ist `no_std`
//! und allokiert nicht (12.1), aber die Groessen stehen im Programm
//! (`capacity`, `capacity_bytes`), nicht im Kern. Ein `Ring` leiht sich
//! darum seine Felder: Der Aufrufer legt sie an — der Codegen in `.bss`
//! (11.2), der Testrahmen als Array, der Interpreter auf dem Heap — und
//! reicht sie herein. Const-Generics waeren die Alternative gewesen; sie
//! haetten je Kapazitaet einen eigenen Typ erzeugt, und ein Programm mit
//! zwanzig Stroemen zwanzig Monomorphisierungen der Tickschleife.
//!
//! **Zwei Formen in einer.** 8.6 unterscheidet feste Slots (`u8`,
//! Records, `Edge`) und den Byte-Ring (`line<N>`, `bytes<N>`): Der eine
//! ist optimal bei fester Groesse, der andere spart, wo Zeilen typisch
//! ein Drittel ihrer Hoechstlaenge haben. Beide teilen den
//! Deskriptor-Ring `(seq, t, offset, len)` und unterscheiden sich nur
//! darin, ob `offset` in einen Byte-Ring zeigt oder in ein Slot-Array.
//! Der Code kennt deshalb nur eine Struktur; wer feste Groessen hat,
//! setzt `len` konstant.

/// Ein Deskriptor: ein Element im Ring (8.6).
///
/// Der Inhalt liegt daneben — im Byte-Ring bei variabler Laenge, im
/// Slot-Array bei fester. `offset` zeigt dorthin, `len` sagt wie weit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Desc {
    /// Laufende Nummer, streng steigend je Strom (9.6).
    pub seq: i64,
    /// Zeitstempel in Nanosekunden (Hardware-Aufloesung, 7.5).
    pub t: i64,
    /// Versatz des Inhalts im Byte- oder Slot-Ring.
    pub offset: u32,
    /// Laenge des Inhalts in Byte.
    pub len: u32,
}

/// Was beim Einlagern geschah (9.6, `deliver`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Eingelagert.
    Ok,
    /// Der Puffer war voll und `overflow = fault`: Das Element ist
    /// verworfen, und die Konsumenten faulten bei ihrer naechsten
    /// Aktivierung (8.6).
    Overflow,
    /// `overflow = drop_oldest`: So viele alte Elemente mussten weichen.
    Dropped(u32),
}

/// Der Puffer eines Stroms (8.6).
///
/// Er haelt keinen Speicher, sondern zeigt auf zwei Felder, die der
/// Aufrufer stellt: den Deskriptor-Ring (`capacity` Eintraege) und den
/// Byte-Ring (`capacity_bytes`). Beide sind Ringe im woertlichen Sinn —
/// `head` und `len` bestimmen, welcher Abschnitt gilt.
#[derive(Debug)]
pub struct Ring<'a> {
    descs: &'a mut [Desc],
    bytes: &'a mut [u8],
    /// Index des aeltesten Deskriptors.
    head: usize,
    /// Zahl der belegten Deskriptoren.
    len: usize,
    /// Versatz des aeltesten Bytes.
    byte_head: usize,
    /// Zahl der belegten Bytes.
    byte_len: usize,
    /// Naechste zu vergebende Nummer.
    next_seq: i64,
    /// `s.dropped` (8.6).
    pub dropped: u32,
    /// `s.overflowed` (8.6).
    pub overflowed: u32,
    /// `s.malformed`: was der Rand nicht dekodieren konnte (8.6, 12.6).
    pub malformed: u32,
}

impl<'a> Ring<'a> {
    /// Ein leerer Ring ueber geliehenen Feldern.
    ///
    /// `descs.len()` ist `capacity`, `bytes.len()` ist `capacity_bytes`.
    /// Ein Strom fester Elementgroesse gibt `bytes` in der Groesse
    /// `capacity * sizeof(T)`; die Rechnung ist dieselbe.
    pub fn new(descs: &'a mut [Desc], bytes: &'a mut [u8]) -> Ring<'a> {
        Ring {
            descs,
            bytes,
            head: 0,
            len: 0,
            byte_head: 0,
            byte_len: 0,
            next_seq: 0,
            dropped: 0,
            overflowed: 0,
            malformed: 0,
        }
    }

    /// Kapazitaet in Elementen (`CAP`).
    pub fn capacity(&self) -> usize {
        self.descs.len()
    }

    /// Kapazitaet in Byte (`CAPB`).
    pub fn capacity_bytes(&self) -> usize {
        self.bytes.len()
    }

    /// Elemente im Puffer.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Ist der Puffer leer?
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Die naechste Nummer: der Cursor nach `s.skip()` (9.6).
    pub fn end(&self) -> i64 {
        self.next_seq
    }

    /// Lagert ein Element ein (9.6, `deliver`).
    ///
    /// Passt es nicht, entscheidet `drop_oldest`: Entweder weichen die
    /// aeltesten Elemente (mit Alert, 8.6), oder das neue wird verworfen
    /// und die Konsumenten faulten. Nie ein undefinierter Zustand — das
    /// ist die Zusage aus 8.6.
    pub fn push(&mut self, t: i64, value: &[u8], drop_oldest: bool) -> Delivery {
        let n = value.len();
        // Ein Element, das allein die Byteschranke sprengt, passt auch in
        // den geleerten Puffer nicht: Es ist ein Ueberlauf, kein
        // Verwerfen — sonst raeumte der Ring sich leer und naehme es
        // trotzdem nicht.
        if n > self.bytes.len() || self.descs.is_empty() {
            self.overflowed += 1;
            return Delivery::Overflow;
        }
        if !self.fits(n) {
            if !drop_oldest {
                self.overflowed += 1;
                return Delivery::Overflow;
            }
            let mut weg = 0;
            while !self.fits(n) && self.len > 0 {
                self.pop_front();
                weg += 1;
            }
            self.dropped += weg;
            self.append(t, value);
            return Delivery::Dropped(weg);
        }
        self.append(t, value);
        Delivery::Ok
    }

    /// Passt ein Element dieser Laenge noch hinein?
    fn fits(&self, n: usize) -> bool {
        self.len < self.descs.len() && self.byte_len + n <= self.bytes.len()
    }

    /// Haengt ein Element an; der Aufrufer hat den Platz geprueft.
    fn append(&mut self, t: i64, value: &[u8]) {
        let offset = (self.byte_head + self.byte_len) % self.bytes.len().max(1);
        for (i, b) in value.iter().enumerate() {
            let at = (offset + i) % self.bytes.len().max(1);
            self.bytes[at] = *b;
        }
        let slot = (self.head + self.len) % self.descs.len();
        self.descs[slot] = Desc { seq: self.next_seq, t, offset: offset as u32, len: value.len() as u32 };
        self.next_seq += 1;
        self.len += 1;
        self.byte_len += value.len();
    }

    /// Entfernt das aelteste Element.
    fn pop_front(&mut self) {
        if self.len == 0 {
            return;
        }
        let d = self.descs[self.head];
        self.head = (self.head + 1) % self.descs.len();
        self.len -= 1;
        self.byte_head = (self.byte_head + d.len as usize) % self.bytes.len().max(1);
        self.byte_len -= d.len as usize;
    }

    /// Der Deskriptor an Position `i` des Fensters eines Konsumenten.
    ///
    /// Das Fenster sind die Elemente ab `cursor` (9.6); `i` zaehlt darin
    /// ab null. `None` heisst: so weit reicht es nicht.
    pub fn at(&self, cursor: i64, i: usize) -> Option<Desc> {
        let ab = self.ab(cursor)?;
        if ab + i >= self.len {
            return None;
        }
        Some(self.descs[(self.head + ab + i) % self.descs.len()])
    }

    /// Zahl der Elemente im Fenster (`s.count`, 8.6).
    ///
    /// Liest ohne zu untersuchen und konsumiert darum nichts.
    pub fn count(&self, cursor: i64) -> usize {
        self.ab(cursor).map_or(0, |ab| self.len - ab)
    }

    /// Der Inhalt eines Elements in den Puffer des Aufrufers.
    ///
    /// Liefert die Zahl der geschriebenen Bytes. Der Inhalt kann im Ring
    /// umbrechen, darum wird er kopiert und nicht geliehen — ein Slice
    /// ueber die Naht hinweg gibt es nicht.
    pub fn read(&self, d: Desc, out: &mut [u8]) -> usize {
        let n = (d.len as usize).min(out.len());
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            *slot = self.bytes[(d.offset as usize + i) % self.bytes.len().max(1)];
        }
        n
    }

    /// Position des Cursors im Puffer, oder `None`, wenn er dahinter
    /// liegt (das Fenster ist dann leer).
    fn ab(&self, cursor: i64) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let erste = self.descs[self.head].seq;
        if cursor <= erste {
            return Some(0);
        }
        let ab = (cursor - erste) as usize;
        if ab >= self.len { None } else { Some(ab) }
    }

    /// Gibt frei, was kein Konsument mehr sehen kann (9.6, Eviction).
    ///
    /// `min_cursor` ist das Minimum ueber alle Konsumenten. Ein Element
    /// verlaesst den Puffer erst, wenn alle daran vorbei sind — das ist
    /// der Grund, warum mehrere Maschinen denselben Strom verschieden
    /// auslegen koennen, ohne einen Verteiler (8.6).
    pub fn evict(&mut self, min_cursor: i64) {
        while self.len > 0 && self.descs[self.head].seq < min_cursor {
            self.pop_front();
        }
    }
}
