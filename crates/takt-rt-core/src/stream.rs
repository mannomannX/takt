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
    pub offset: usize,
    /// Laenge des Inhalts in Byte.
    pub len: usize,
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
            self.overflowed = self.overflowed.saturating_add(1);
            return Delivery::Overflow;
        }
        if !self.fits(n) {
            if !drop_oldest {
                self.overflowed = self.overflowed.saturating_add(1);
                return Delivery::Overflow;
            }
            let mut weg: u32 = 0;
            while !self.fits(n) && self.len > 0 {
                self.pop_front();
                weg = weg.saturating_add(1);
            }
            self.dropped = self.dropped.saturating_add(weg);
            self.append(t, value);
            return Delivery::Dropped(weg);
        }
        self.append(t, value);
        Delivery::Ok
    }

    /// Passt ein Element dieser Laenge noch hinein?
    fn fits(&self, n: usize) -> bool {
        self.len < self.descs.len() && self.byte_len.saturating_add(n) <= self.bytes.len()
    }

    /// Haengt ein Element an; der Aufrufer hat den Platz geprueft.
    fn append(&mut self, t: i64, value: &[u8]) {
        let offset = ring(self.byte_head, self.byte_len, self.bytes.len());
        for (i, b) in value.iter().enumerate() {
            if let Some(slot) = self.bytes.get_mut(ring(offset, i, self.bytes.len())) {
                *slot = *b;
            }
        }
        if let Some(slot) = self.descs.get_mut(ring(self.head, self.len, self.descs.len())) {
            *slot = Desc { seq: self.next_seq, t, offset, len: value.len() };
        }
        self.next_seq = self.next_seq.saturating_add(1);
        self.len = self.len.saturating_add(1);
        self.byte_len = self.byte_len.saturating_add(value.len());
    }

    /// Entfernt das aelteste Element.
    fn pop_front(&mut self) {
        let Some(d) = self.oldest() else { return };
        self.head = ring(self.head, 1, self.descs.len());
        self.len = self.len.saturating_sub(1);
        self.byte_head = ring(self.byte_head, d.len, self.bytes.len());
        self.byte_len = self.byte_len.saturating_sub(d.len);
    }

    /// Der aelteste Deskriptor, wenn der Puffer nicht leer ist.
    fn oldest(&self) -> Option<Desc> {
        if self.len == 0 {
            return None;
        }
        self.descs.get(self.head).copied()
    }

    /// Der Deskriptor an Position `i` des Fensters eines Konsumenten.
    ///
    /// Das Fenster sind die Elemente ab `cursor` (9.6); `i` zaehlt darin
    /// ab null. `None` heisst: so weit reicht es nicht.
    pub fn at(&self, cursor: i64, i: usize) -> Option<Desc> {
        let pos = self.ab(cursor)?.checked_add(i).filter(|&p| p < self.len)?;
        self.descs.get(ring(self.head, pos, self.descs.len())).copied()
    }

    /// Zahl der Elemente im Fenster (`s.count`, 8.6).
    ///
    /// Liest ohne zu untersuchen und konsumiert darum nichts.
    pub fn count(&self, cursor: i64) -> usize {
        self.ab(cursor).map_or(0, |ab| self.len.saturating_sub(ab))
    }

    /// Der Inhalt eines Elements in den Puffer des Aufrufers.
    ///
    /// Liefert die Zahl der geschriebenen Bytes. Der Inhalt kann im Ring
    /// umbrechen, darum wird er kopiert und nicht geliehen — ein Slice
    /// ueber die Naht hinweg gibt es nicht.
    pub fn read(&self, d: Desc, out: &mut [u8]) -> usize {
        let n = d.len.min(out.len());
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            if let Some(b) = self.bytes.get(ring(d.offset, i, self.bytes.len())) {
                *slot = *b;
            }
        }
        n
    }

    /// Position des Cursors im Puffer, oder `None`, wenn er dahinter
    /// liegt (das Fenster ist dann leer).
    fn ab(&self, cursor: i64) -> Option<usize> {
        let erste = self.oldest()?.seq;
        if cursor <= erste {
            return Some(0);
        }
        let ab = usize::try_from(cursor.saturating_sub(erste)).ok()?;
        (ab < self.len).then_some(ab)
    }

    /// Gibt frei, was kein Konsument mehr sehen kann (9.6, Eviction).
    ///
    /// `min_cursor` ist das Minimum ueber alle Konsumenten. Ein Element
    /// verlaesst den Puffer erst, wenn alle daran vorbei sind — das ist
    /// der Grund, warum mehrere Maschinen denselben Strom verschieden
    /// auslegen koennen, ohne einen Verteiler (8.6).
    pub fn evict(&mut self, min_cursor: i64) {
        while self.oldest().is_some_and(|d| d.seq < min_cursor) {
            self.pop_front();
        }
    }
}

/// Die Position `a + b` in einem Ring der Laenge `m`; ein leerer Ring hat
/// nur die Position null. Die Summe laeuft nicht ueber: Beide Summanden
/// liegen unter der Laenge eines Slice.
fn ring(a: usize, b: usize, m: usize) -> usize {
    a.saturating_add(b).checked_rem(m).unwrap_or(0)
}
