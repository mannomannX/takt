//! Das `persist`-Journal (5.9, 12.1, 12.3).
//!
//! 5.9 nennt die Anforderungen: „zwei Slots im Wechsel (ping-pong); jeder
//! Eintrag traegt Sequenznummer und CRC32; ein Schreibvorgang aendert
//! genau einen Slot; beim Start gewinnt der gueltige Eintrag mit der
//! hoeheren Sequenznummer; Schreiben nur mit Sektorgranularitaet und
//! `min_interval`."
//!
//! Die Nutzlast ist hier `&[u8]` und bleibt es: `takt-rt-core` haengt
//! nicht von `takt-mir` ab — „Der Kern laeuft auf dem Target und darf die
//! IR nicht kennen". Wer die Bytes erzeugt, weiss sie zu deuten.
//!
//! Ein Slot traegt *alle* `persist`-Variablen des Programms, nicht eine je
//! Variable: 5.9 spricht im Singular vom Slot und vom Eintrag, und ein
//! Journal je Variable braeuchte je Variable zwei Sektoren.

use takt_native::crc::{crc32_final, crc32_start, crc32_update};

use crate::loopcore::{Nvm, NvmState};

/// Kennung eines beschriebenen Slots: `TKPJ`.
pub const MAGIC: u32 = 0x4A50_4B54;

/// Formatversion des Slots (11.3: Leser akzeptieren aeltere).
pub const VERSION: u16 = 1;

/// Laenge des Slot-Kopfes in Byte.
pub const HEADER: usize = 32;

/// Der Kopf eines Slots.
///
/// Feste Groesse, damit das Lesen ohne Allokation auskommt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    version: u16,
    sequence: u64,
    logic_hash: u64,
    length: u32,
    crc32: u32,
}

impl Header {
    /// Schreibt den Kopf; `crc32` deckt alles ausser sich selbst.
    fn write(&self, into: &mut [u8; HEADER]) {
        into[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        into[4..6].copy_from_slice(&self.version.to_le_bytes());
        into[6..8].copy_from_slice(&[0, 0]);
        into[8..16].copy_from_slice(&self.sequence.to_le_bytes());
        into[16..24].copy_from_slice(&self.logic_hash.to_le_bytes());
        into[24..28].copy_from_slice(&self.length.to_le_bytes());
        into[28..32].copy_from_slice(&self.crc32.to_le_bytes());
    }

    /// Liest den Kopf; `None` bei falschem Magic oder unbekannter Version.
    fn read(from: &[u8]) -> Option<Header> {
        if from.len() < HEADER {
            return None;
        }
        let magic = u32::from_le_bytes(from[0..4].try_into().ok()?);
        if magic != MAGIC {
            return None;
        }
        let version = u16::from_le_bytes(from[4..6].try_into().ok()?);
        if version > VERSION {
            return None;
        }
        Some(Header {
            version,
            sequence: u64::from_le_bytes(from[8..16].try_into().ok()?),
            logic_hash: u64::from_le_bytes(from[16..24].try_into().ok()?),
            length: u32::from_le_bytes(from[24..28].try_into().ok()?),
            crc32: u32::from_le_bytes(from[28..32].try_into().ok()?),
        })
    }
}

/// Der CRC32 ueber Kopf (ohne das CRC-Feld) und Nutzlast.
fn checksum(header: &[u8; HEADER], payload: &[u8]) -> u32 {
    crc32_final(crc32_update(crc32_update(crc32_start(), &header[..28]), payload))
}

/// Was beim Laden herauskam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// Ein gueltiger Eintrag steht im Puffer.
    Found {
        /// So viele Byte Nutzlast.
        length: u32,
        /// Seine Sequenznummer.
        sequence: u64,
    },
    /// Kein gueltiger Eintrag: Erstlauf oder alles verworfen.
    Empty,
}

/// Wie weit ein Schreibvorgang ist.
///
/// Die Nutzlast geht vor dem Kopf aufs Medium. Gegen einen Abbruch
/// schuetzt schon der CRC — er deckt Kopf *und* Nutzlast, ein halber
/// Eintrag faellt also in jeder Reihenfolge durch. Die Reihenfolge ist die
/// zweite Linie: Ohne Kopf fehlt das Magic, und der Slot wird verworfen,
/// bevor irgendetwas gerechnet wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Nichts zu tun.
    Idle,
    /// Der Zielslot wird geloescht.
    Erasing,
    /// Die Nutzlast wird geschrieben.
    Payload,
    /// Der Kopf wird geschrieben; danach gilt der Slot.
    Head,
}

/// Das Journal ueber einem NVM-Geraet.
pub struct Journal<N: Nvm> {
    nvm: N,
    logic_hash: u64,
    min_interval_ns: i64,
    /// Der Slot, in den als Naechstes geschrieben wird.
    next_slot: u8,
    sequence: u64,
    phase: Phase,
    /// Wann zuletzt ein Schreibvorgang begann.
    last_write_ns: i64,
    /// Ob ueberhaupt schon einmal geschrieben wurde.
    ever_written: bool,
    /// Gescheiterte Vorgaenge, fuer die Telemetrie.
    failures: u32,
    /// Abgeschlossene Vorgaenge.
    writes: u32,
    /// Der Kopf des laufenden Vorgangs, bis die Nutzlast steht.
    head: [u8; HEADER],
    /// Laenge der Nutzlast des laufenden Vorgangs.
    pending_len: usize,
    /// Ob der letzte Vorgang scheiterte.
    failed_last: bool,
    /// Ob [`Journal::load`] gelaufen ist.
    loaded: bool,
}

impl<N: Nvm> Journal<N> {
    /// Neues Journal.
    ///
    /// `logic_hash` unterscheidet Eintraege verschiedener Programme auf
    /// derselben Hardware (Testaufbau, A/B-Image): Zwei Programme koennen
    /// gleich benannte und gleich typisierte Variablen haben, dann waere
    /// der Typ-Hash identisch und die Bedeutung nicht.
    pub fn new(nvm: N, logic_hash: u64, min_interval_ns: i64) -> Journal<N> {
        Journal {
            nvm,
            logic_hash,
            min_interval_ns,
            next_slot: 0,
            sequence: 0,
            phase: Phase::Idle,
            last_write_ns: 0,
            ever_written: false,
            failures: 0,
            writes: 0,
            head: [0; HEADER],
            pending_len: 0,
            failed_last: false,
            loaded: false,
        }
    }

    /// Wie viele Schreibvorgaenge abgeschlossen wurden.
    pub fn writes(&self) -> u32 {
        self.writes
    }

    /// Wie viele gescheitert sind.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Das Geraet zurueck (Tests, Diagnose).
    pub fn into_inner(self) -> N {
        self.nvm
    }

    /// Zugriff auf das Geraet (Tests, Diagnose).
    pub fn device_mut(&mut self) -> &mut N {
        &mut self.nvm
    }

    /// Liest den gueltigen Eintrag mit der hoechsten Sequenznummer (5.9).
    ///
    /// Laeuft vor dem ersten Tick und darf blockieren. Jeder Fehler —
    /// falsches Magic, unbekannte Version, CRC, fremder `logic_hash`,
    /// Laenge ueber dem Puffer — bedeutet nur, dass dieser Slot nicht
    /// gilt. Das Journal faultet nie.
    ///
    /// **Vor dem ersten `poll` noetig.** Erst das Laden sagt, welcher Slot
    /// belegt ist; ohne es schriebe der erste Vorgang blind nach Slot 0
    /// und zerstoerte womoeglich den einzigen gueltigen Eintrag.
    pub fn load(&mut self, into: &mut [u8]) -> Loaded {
        let mut best: Option<(u64, u32, u8)> = None;
        for slot in 0..2u8 {
            let Some((header, length)) = self.read_slot(slot, into) else { continue };
            if best.is_none_or(|(seq, _, _)| header.sequence > seq) {
                best = Some((header.sequence, length, slot));
            }
        }
        self.loaded = true;
        let Some((sequence, length, slot)) = best else {
            return Loaded::Empty;
        };
        // Der Puffer traegt jetzt vielleicht den anderen Slot; noch einmal
        // den Gewinner lesen.
        if self.read_slot(slot, into).is_none() {
            return Loaded::Empty;
        }
        self.sequence = sequence;
        self.next_slot = 1 - slot;
        self.ever_written = true;
        Loaded::Found { length, sequence }
    }

    /// Liest einen Slot und prueft ihn; `into` traegt danach die Nutzlast.
    fn read_slot(&mut self, slot: u8, into: &mut [u8]) -> Option<(Header, u32)> {
        let mut head = [0u8; HEADER];
        if !self.nvm.read(slot, 0, &mut head) {
            return None;
        }
        let header = Header::read(&head)?;
        if header.logic_hash != self.logic_hash {
            return None;
        }
        let length = header.length as usize;
        if length > into.len() || HEADER + length > self.nvm.slot_size() as usize {
            return None;
        }
        let payload = into.get_mut(..length)?;
        if !self.nvm.read(slot, HEADER as u32, payload) {
            return None;
        }
        if checksum(&head, payload) != header.crc32 {
            return None;
        }
        Some((header, header.length))
    }

    /// Treibt einen laufenden Vorgang voran und beginnt bei Bedarf einen
    /// neuen. Blockiert nie.
    ///
    /// `current` ist die kanonische Form aller `persist`-Variablen,
    /// `stored` der zuletzt geschriebene Stand — der Vergleich entscheidet,
    /// ob ueberhaupt etwas zu tun ist (5.9: „geaenderte Werte").
    pub fn poll(&mut self, now: i64, current: &[u8], stored: &mut [u8], stored_len: &mut usize) {
        if !self.loaded {
            return;
        }
        if self.phase == Phase::Idle {
            if self.differs(current, stored, *stored_len)
                && (!self.ever_written || now - self.last_write_ns >= self.min_interval_ns)
            {
                self.last_write_ns = now;
                self.begin(current, stored, stored_len);
            }
            return;
        }
        self.advance(stored, stored_len);
    }

    /// Schreibt ausstehende Aenderungen sofort (5.9: vor `reboot`,
    /// `boot_jump` und Deep Sleep). Blockiert, weil danach nichts mehr
    /// laeuft, worauf Ruecksicht zu nehmen waere.
    ///
    /// `min_interval` gilt hier nicht: Es begrenzt den Verschleiss im
    /// Dauerbetrieb, nicht den letzten Schreibvorgang vor dem Aus.
    pub fn flush(&mut self, current: &[u8], stored: &mut [u8], stored_len: &mut usize) -> bool {
        if !self.loaded {
            return false;
        }
        while self.phase != Phase::Idle {
            self.advance(stored, stored_len);
        }
        if self.failed_last {
            return false;
        }
        if !self.differs(current, stored, *stored_len) {
            return true;
        }
        self.begin(current, stored, stored_len);
        while self.phase != Phase::Idle {
            self.advance(stored, stored_len);
        }
        !self.failed_last
    }

    /// Ein Schritt des Automaten.
    fn advance(&mut self, stored: &[u8], stored_len: &mut usize) {
        match (self.phase, self.nvm.poll()) {
            (_, NvmState::Failed) => self.give_up(stored_len),
            (Phase::Erasing, NvmState::Done) => {
                if self.nvm.begin_write(self.next_slot, HEADER as u32, &stored[..self.pending_len]) {
                    self.phase = Phase::Payload;
                } else {
                    self.give_up(stored_len);
                }
            }
            (Phase::Payload, NvmState::Done) => {
                let head = self.head;
                if self.nvm.begin_write(self.next_slot, 0, &head) {
                    self.phase = Phase::Head;
                } else {
                    self.give_up(stored_len);
                }
            }
            (Phase::Head, NvmState::Done) => self.finish(),
            _ => {}
        }
    }

    fn differs(&self, current: &[u8], stored: &[u8], stored_len: usize) -> bool {
        current.len() != stored_len || current != &stored[..stored_len.min(stored.len())]
    }

    /// Beginnt einen Vorgang: den Stand nach `stored` kopieren, Kopf
    /// rechnen, Slot loeschen.
    ///
    /// Geschrieben wird aus `stored`, nicht aus `current`: Der aendert sich
    /// weiter, waehrend das Geraet arbeitet, und der CRC muss zu den Bytes
    /// passen, die wirklich ankommen.
    fn begin(&mut self, current: &[u8], stored: &mut [u8], stored_len: &mut usize) {
        let n = current.len().min(stored.len());
        stored[..n].copy_from_slice(&current[..n]);
        *stored_len = n;
        let header = Header {
            version: VERSION,
            sequence: self.sequence.wrapping_add(1),
            logic_hash: self.logic_hash,
            length: n as u32,
            crc32: 0,
        };
        let mut head = [0u8; HEADER];
        header.write(&mut head);
        let crc = checksum(&head, &stored[..n]);
        Header { crc32: crc, ..header }.write(&mut head);
        self.head = head;
        self.pending_len = n;
        self.failed_last = false;
        if self.nvm.begin_erase(self.next_slot) {
            self.phase = Phase::Erasing;
        } else {
            self.give_up(stored_len);
        }
    }

    fn finish(&mut self) {
        self.sequence = self.sequence.wrapping_add(1);
        self.next_slot = 1 - self.next_slot;
        self.ever_written = true;
        self.writes = self.writes.saturating_add(1);
        self.phase = Phase::Idle;
    }

    /// Ein gescheiterter Vorgang laesst `stored` ungueltig: Der naechste
    /// Vergleich sieht eine Aenderung und schreibt erneut.
    fn give_up(&mut self, stored_len: &mut usize) {
        *stored_len = 0;
        self.failures = self.failures.saturating_add(1);
        self.failed_last = true;
        self.phase = Phase::Idle;
    }
}

/// Journal plus die zwei Puffer, die es braucht: der aktuelle Stand und
/// der zuletzt geschriebene (5.9: „geaenderte Werte"). Beide kommen vom
/// Aufrufer — `takt size` kennt ihre Groesse, der Kern allokiert nicht.
pub struct Persist<'a, N: Nvm> {
    journal: Journal<N>,
    current: &'a mut [u8],
    stored: &'a mut [u8],
    stored_len: usize,
}

impl<'a, N: Nvm> Persist<'a, N> {
    /// Buendelt Journal und Puffer.
    pub fn new(journal: Journal<N>, current: &'a mut [u8], stored: &'a mut [u8]) -> Persist<'a, N> {
        Persist { journal, current, stored, stored_len: 0 }
    }

    /// Laedt den gueltigen Eintrag und reicht ihn ans Programm (5.9).
    ///
    /// Vor dem ersten Tick: nach den Defaults, vor `enter:`. Liefert, was
    /// das Journal fand; wie viele Eintraege das Programm annahm, sagt
    /// `applied`.
    pub fn load(&mut self, program: &mut impl crate::loopcore::Program) -> (Loaded, usize) {
        let found = self.journal.load(self.current);
        let applied = match found {
            Loaded::Found { length, .. } => {
                let n = length as usize;
                self.stored[..n].copy_from_slice(&self.current[..n]);
                self.stored_len = n;
                program.persist_restore(&self.current[..n])
            }
            Loaded::Empty => 0,
        };
        (found, applied)
    }

    /// Ein Schritt des Journals; nie blockierend.
    pub fn poll(&mut self, now: i64, program: &mut impl crate::loopcore::Program) {
        let n = program.persist_snapshot(self.current);
        let (current, stored) = (&*self.current, &mut *self.stored);
        self.journal.poll(now, &current[..n], stored, &mut self.stored_len);
    }

    /// Schreibt ausstehende Aenderungen synchron (5.9: vor `reboot`,
    /// `boot_jump` und Deep Sleep).
    pub fn flush(&mut self, program: &mut impl crate::loopcore::Program) -> bool {
        let n = program.persist_snapshot(self.current);
        let (current, stored) = (&*self.current, &mut *self.stored);
        self.journal.flush(&current[..n], stored, &mut self.stored_len)
    }

    /// Das Journal darunter.
    pub fn journal(&self) -> &Journal<N> {
        &self.journal
    }

    /// Journal und Geraet zurueck.
    pub fn into_journal(self) -> Journal<N> {
        self.journal
    }
}

/// Ein NVM im Speicher, fuer Tests (8.11).
///
/// `cut_at` erfuellt, was 8.11 als `CUT_AT_BYTE` beschreibt: „bricht einen
/// Programmiervorgang mitten im Sektor ab und laesst den Rest unbestimmt."
/// Damit ist die Stromausfallsicherheit aus 5.9 pruefbar, ohne dass das
/// Takt-Flash-Modell (M6) schon existiert.
///
/// `N` ist die Slotgroesse; ohne Allokation, wie der Rest des Crates.
pub struct FakeNvm<const N: usize> {
    slots: [[u8; N]; 2],
    /// Wie viele `poll`-Aufrufe ein Vorgang braucht; bildet Sektorzeiten ab.
    latency: u32,
    busy: u32,
    /// Was beim Fertigwerden zu tun ist.
    job: Job<N>,
    /// Nach so vielen geschriebenen Bytes bricht der Strom ab.
    cut_at: Option<u32>,
    written: u32,
    cut: bool,
}

/// Der laufende Vorgang einer [`FakeNvm`].
enum Job<const N: usize> {
    None,
    Erase(u8),
    Write { slot: u8, offset: u32, len: usize, bytes: [u8; N] },
}

impl<const N: usize> FakeNvm<N> {
    /// Zwei geloeschte Slots.
    pub fn new() -> FakeNvm<N> {
        FakeNvm { slots: [[0xFF; N]; 2], latency: 1, busy: 0, job: Job::None, cut_at: None, written: 0, cut: false }
    }

    /// Wie viele `poll`-Aufrufe ein Vorgang dauert.
    pub fn with_latency(mut self, polls: u32) -> FakeNvm<N> {
        self.latency = polls.max(1);
        self
    }

    /// Bricht nach `n` geschriebenen Bytes ab (8.11: `CUT_AT_BYTE`).
    pub fn cut_at(&mut self, n: u32) {
        self.cut_at = Some(n);
        self.written = 0;
        self.cut = false;
    }

    /// Strom wieder da: Der Inhalt bleibt, wie der Abbruch ihn liess.
    pub fn power_on(&mut self) {
        self.cut_at = None;
        self.cut = false;
        self.written = 0;
        self.busy = 0;
        self.job = Job::None;
    }

    /// Wie viele Bytes seit dem letzten [`FakeNvm::cut_at`] geschrieben wurden.
    pub fn bytes_written(&self) -> u32 {
        self.written
    }

    /// Ob der Strom abgebrochen ist.
    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// Der rohe Inhalt eines Slots.
    pub fn slot(&self, slot: u8) -> &[u8; N] {
        &self.slots[slot as usize]
    }

    /// Schreibt ein Byte, sofern der Strom noch da ist.
    fn put(&mut self, slot: u8, at: usize, byte: u8) {
        if self.cut {
            return;
        }
        if self.cut_at.is_some_and(|n| self.written >= n) {
            self.cut = true;
            return;
        }
        self.written = self.written.saturating_add(1);
        if let Some(cell) = self.slots[slot as usize].get_mut(at) {
            *cell = byte;
        }
    }
}

impl<const N: usize> Default for FakeNvm<N> {
    fn default() -> FakeNvm<N> {
        FakeNvm::new()
    }
}

impl<const N: usize> Nvm for FakeNvm<N> {
    fn slot_size(&self) -> u32 {
        N as u32
    }

    fn begin_erase(&mut self, slot: u8) -> bool {
        if self.busy > 0 || slot > 1 {
            return false;
        }
        self.job = Job::Erase(slot);
        self.busy = self.latency;
        true
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        if self.busy > 0 || slot > 1 || bytes.len() > N {
            return false;
        }
        let mut buf = [0u8; N];
        buf[..bytes.len()].copy_from_slice(bytes);
        self.job = Job::Write { slot, offset, len: bytes.len(), bytes: buf };
        self.busy = self.latency;
        true
    }

    fn poll(&mut self) -> NvmState {
        // Ohne Strom wird kein Vorgang fertig; er bleibt haengen, bis der
        // Aufrufer aufgibt oder neu startet.
        if self.cut {
            return NvmState::Busy;
        }
        if self.busy == 0 {
            return NvmState::Idle;
        }
        self.busy -= 1;
        if self.busy > 0 {
            return NvmState::Busy;
        }
        match core::mem::replace(&mut self.job, Job::None) {
            Job::None => {}
            Job::Erase(slot) => {
                for at in 0..N {
                    self.put(slot, at, 0xFF);
                }
            }
            Job::Write { slot, offset, len, bytes } => {
                for (i, b) in bytes.iter().enumerate().take(len) {
                    self.put(slot, offset as usize + i, *b);
                }
            }
        }
        if self.cut { NvmState::Busy } else { NvmState::Done }
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        let Some(src) = self.slots.get(slot as usize) else { return false };
        let at = offset as usize;
        let Some(slice) = src.get(at..at + into.len()) else { return false };
        into.copy_from_slice(slice);
        true
    }
}
