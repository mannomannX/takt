//! Das `persist`-Journal (Referenz 5.9).
//!
//! Die Anforderungen der Reihe nach: Ping-Pong, Sequenznummer, CRC32,
//! „beim Start gewinnt der gueltige Eintrag mit der hoeheren
//! Sequenznummer", `min_interval` — und die Stromausfallsicherheit, die
//! 8.11 mit `CUT_AT_BYTE` prueft.

use takt_native::crc::{crc32_final, crc32_start, crc32_update};
use takt_rt_core::journal::{FakeNvm, HEADER, Journal, Loaded, MAGIC, Persist};
use takt_rt_core::loopcore::{Nvm, NvmState};

/// Slotgroesse der Tests; gross genug fuer Kopf und Nutzlast.
const SLOT: usize = 128;

const HASH: u64 = 0xABCD_1234_5678_9EF0;
const SECOND: i64 = 1_000_000_000;

/// Ein Journal ueber einem leeren Geraet, bereits geladen.
fn journal(min_interval_ns: i64) -> Journal<FakeNvm<SLOT>> {
    let mut j = Journal::new(FakeNvm::new(), HASH, min_interval_ns);
    j.load(&mut [0u8; SLOT]);
    j
}

/// Ein Journal ueber einem vorhandenen Geraet; laedt und liefert das
/// Ergebnis mit.
fn reopen(nvm: FakeNvm<SLOT>, hash: u64, into: &mut [u8]) -> (Journal<FakeNvm<SLOT>>, Loaded) {
    let mut j = Journal::new(nvm, hash, 0);
    let got = j.load(into);
    (j, got)
}

/// Ein geladenes Journal, in dem `old` schon steht.
fn with_old(old: &[u8]) -> (Journal<FakeNvm<SLOT>>, usize) {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, old, &mut stored, &mut len);
    let nvm = j.into_inner();
    let mut buf = [0u8; SLOT];
    let (j, _) = reopen(nvm, HASH, &mut buf);
    (j, old.len())
}

/// Ein Vergleichspuffer, der `payload` schon traegt.
fn buffer_with(payload: &[u8]) -> ([u8; SLOT], usize) {
    let mut buf = [0u8; SLOT];
    buf[..payload.len()].copy_from_slice(payload);
    (buf, payload.len())
}

/// Schreibt `payload`, bis der Vorgang durch ist.
fn write(j: &mut Journal<FakeNvm<SLOT>>, now: i64, payload: &[u8], stored: &mut [u8], len: &mut usize) {
    let before = j.writes();
    for _ in 0..64 {
        j.poll(now, payload, stored, len);
        if j.writes() > before {
            return;
        }
    }
    panic!("Schreibvorgang wird nicht fertig");
}

#[test]
fn an_empty_store_reports_empty() {
    let mut j = journal(0);
    let mut buf = [0u8; SLOT];
    assert_eq!(j.load(&mut buf), Loaded::Empty);
}

#[test]
fn what_was_written_comes_back() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"hallo", &mut stored, &mut len);

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 5, sequence: 1 });
    assert_eq!(&buf[..5], b"hallo");
}

#[test]
fn entries_append_within_a_slot_without_erasing() {
    // Version 2: Ein Slot ist ein Log. Der zweite Eintrag steht hinter dem
    // ersten (32 + 4 Byte, auf 4 ausgerichtet: Versatz 36), der andere
    // Slot bleibt geloescht, geloescht wurde nur einmal.
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"eins", &mut stored, &mut len);
    write(&mut j, SECOND, b"zwei", &mut stored, &mut len);
    let nvm = j.into_inner();
    assert_eq!(nvm.erases(), 1);
    assert_eq!(nvm.slot(0)[..4], MAGIC.to_le_bytes());
    assert_eq!(nvm.slot(0)[36..40], MAGIC.to_le_bytes(), "der zweite Eintrag haengt am ersten");
    assert_eq!(*nvm.slot(1), [0xFF; SLOT], "Slot 1 blieb in Ruhe");
}

#[test]
fn a_full_slot_switches_to_the_other_and_erases_it() {
    // 5.9: „zwei Slots im Wechsel (ping-pong); ein Schreibvorgang aendert
    // genau einen Slot." Bei 128 Byte je Slot passen drei Eintraege zu 36
    // Byte; der vierte wechselt.
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    for (i, p) in [b"eins", b"zwei", b"drei", b"vier"].iter().enumerate() {
        write(&mut j, i as i64 * SECOND, *p, &mut stored, &mut len);
    }
    let nvm = j.into_inner();
    assert_eq!(nvm.erases(), 2, "einmal Slot 0, einmal Slot 1");
    assert_eq!(nvm.slot(1)[..4], MAGIC.to_le_bytes(), "der vierte Eintrag steht in Slot 1");
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 4, sequence: 4 });
    assert_eq!(&buf[..4], b"vier");
}

#[test]
fn a_dirty_tail_forces_a_slot_switch() {
    // Bytes hinter dem letzten Eintrag, die nicht geloescht sind — ein
    // abgebrochener Vorgang —, machen den Slot unbeschreibbar: NOR-Flash
    // loescht Bits nur, ein Eintrag darueber waere verfaelscht.
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"eins", &mut stored, &mut len);
    let mut nvm = j.into_inner();
    assert!(nvm.begin_write(0, 36, &[0x00, 0x11, 0x22, 0x33]));
    while nvm.poll() == NvmState::Busy {}
    let mut buf = [0u8; SLOT];
    let (mut j, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 4, sequence: 1 });
    let (mut stored, mut len) = buffer_with(b"eins");
    write(&mut j, SECOND, b"zwei", &mut stored, &mut len);
    let nvm = j.into_inner();
    assert_eq!(nvm.slot(1)[..4], MAGIC.to_le_bytes(), "der neue Eintrag ging in den anderen Slot");
}

#[test]
fn after_a_cut_the_next_start_appends_or_switches_safely() {
    // Ein Abbruch waehrend des Anhaengens: Der alte Eintrag bleibt, der
    // naechste Lauf schreibt weiter, und der dritte Start findet den
    // juengsten gueltigen Stand.
    for cut in 1..60 {
        let mut j = journal(0);
        let (mut stored, mut len) = ([0u8; SLOT], 0);
        write(&mut j, 0, b"alt", &mut stored, &mut len);
        j.device_mut().cut_at(cut);
        for _ in 0..64 {
            j.poll(SECOND, b"neu", &mut stored, &mut len);
        }
        let mut nvm = j.into_inner();
        nvm.power_on();
        let mut buf = [0u8; SLOT];
        let (mut j, got) = reopen(nvm, HASH, &mut buf);
        let Loaded::Found { length, .. } = got else { panic!("Abbruch bei {cut}: nichts mehr da") };
        let (mut stored, mut len) = buffer_with(&buf[..length as usize]);
        write(&mut j, 2 * SECOND, b"danach", &mut stored, &mut len);
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
        let Loaded::Found { length, .. } = got else { panic!("Abbruch bei {cut}: verloren") };
        assert_eq!(&buf[..length as usize], b"danach", "Abbruch bei {cut}");
    }
}

#[test]
fn the_higher_sequence_number_wins() {
    // 5.9: „beim Start gewinnt der gueltige Eintrag mit der hoeheren
    // Sequenznummer."
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"alt", &mut stored, &mut len);
    write(&mut j, SECOND, b"neu", &mut stored, &mut len);

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 3, sequence: 2 });
    assert_eq!(&buf[..3], b"neu");
}

#[test]
fn a_broken_checksum_invalidates_a_slot() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"daten", &mut stored, &mut len);

    let mut nvm = j.into_inner();
    // Ein Byte der Nutzlast kippen, ohne den Kopf anzufassen.
    let mut payload = [0u8; 5];
    nvm.read(0, HEADER as u32, &mut payload);
    payload[0] ^= 0xFF;
    nvm.power_on();
    nvm.begin_write(0, HEADER as u32, &payload);
    while nvm.poll() == takt_rt_core::loopcore::NvmState::Busy {}

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!(got, Loaded::Empty, "verfaelschte Nutzlast gilt nicht");
}

#[test]
fn an_entry_of_another_program_is_ignored() {
    // Zwei Programme auf derselben Hardware koennen gleich benannte und
    // gleich typisierte Variablen haben; der Typ-Hash allein traegt das
    // nicht.
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"fremd", &mut stored, &mut len);

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH ^ 1, &mut buf);
    assert_eq!(got, Loaded::Empty);
}

#[test]
fn an_unwritten_slot_is_not_a_fault() {
    let mut nvm = FakeNvm::<SLOT>::new();
    // Zufall im Flash, kein gueltiges Magic.
    nvm.begin_write(0, 0, &[0x12; 40]);
    while nvm.poll() == takt_rt_core::loopcore::NvmState::Busy {}

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!(got, Loaded::Empty);
}

#[test]
fn unchanged_values_are_not_written() {
    // 5.9 spricht von „geaenderten Werten"; alles andere verschliesse den
    // Flash ohne Not.
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"gleich", &mut stored, &mut len);
    assert_eq!(j.writes(), 1);
    for k in 1..100 {
        j.poll(k * SECOND, b"gleich", &mut stored, &mut len);
    }
    assert_eq!(j.writes(), 1, "unveraenderte Werte wurden erneut geschrieben");
}

#[test]
fn min_interval_limits_the_rate() {
    // 5.9: „hoechstens alle `min_interval`."
    let ten_s = 10 * SECOND;
    let mut j = journal(ten_s);
    let (mut stored, mut len) = ([0u8; SLOT], 0);

    // Der erste Schreibvorgang darf sofort laufen.
    write(&mut j, 0, b"a", &mut stored, &mut len);
    assert_eq!(j.writes(), 1);

    // Innerhalb des Intervalls aendert sich der Wert jede Sekunde.
    for k in 1..10 {
        let payload = [b'a' + k as u8];
        for _ in 0..8 {
            j.poll(k * SECOND, &payload, &mut stored, &mut len);
        }
    }
    assert_eq!(j.writes(), 1, "vor Ablauf des Intervalls geschrieben");

    write(&mut j, ten_s, b"z", &mut stored, &mut len);
    assert_eq!(j.writes(), 2);
}

#[test]
fn a_flush_ignores_the_interval() {
    // 5.9: Vor dem geordneten Ende eines Laufs (`next_run`, 12.7) wird
    // synchron geschrieben — dort begrenzt kein Intervall mehr den Verschleiss.
    let mut j = journal(10 * SECOND);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    assert!(j.flush(b"b", &mut stored, &mut len, || true));
    assert_eq!(j.writes(), 2);

    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 1, sequence: 2 });
    assert_eq!(&buf[..1], b"b");
}

#[test]
fn a_slow_device_does_not_stall_the_caller() {
    // Der Test, den FB-149 gebraucht haette: Ein Geraet, dessen Vorgaenge
    // 50 Aufrufe dauern, haelt `poll` nicht auf.
    let mut j = Journal::new(FakeNvm::<SLOT>::new().with_latency(50), HASH, 0);
    j.load(&mut [0u8; SLOT]);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    let mut polls = 0;
    while j.writes() == 0 {
        j.poll(0, b"lang", &mut stored, &mut len);
        polls += 1;
        assert!(polls < 1000, "wird nicht fertig");
    }
    assert!(polls > 50, "ein langsames Geraet war ploetzlich schnell");
    assert_eq!(j.writes(), 1);
}

/// Die Abnahme nach 5.9: Stromausfall an *jeder* Stelle.
///
/// 8.11 beschreibt `CUT_AT_BYTE` als „bricht einen Programmiervorgang
/// mitten im Sektor ab und laesst den Rest unbestimmt". Geprueft wird
/// nicht stichprobenhaft, sondern fuer jedes Byte: Danach steht entweder
/// der alte oder der neue Stand da, nie etwas dazwischen.
#[test]
fn a_power_cut_leaves_either_the_old_or_the_new_entry() {
    const OLD: &[u8] = b"alter-stand";
    const NEW: &[u8] = b"neuer-stand";

    // Wie viele Bytes ein vollstaendiger Vorgang schreibt; die Attrappe
    // zaehlt das Loeschen mit, weil auch dabei der Strom wegbleiben kann.
    let total = {
        let (mut j, _) = with_old(OLD);
        let (mut stored, mut len) = buffer_with(OLD);
        write(&mut j, SECOND, NEW, &mut stored, &mut len);
        j.into_inner().bytes_written()
    };
    assert!(total > 0, "die Attrappe zaehlt nicht");

    for cut in 0..=total {
        let (mut j, _) = with_old(OLD);
        j.device_mut().cut_at(cut);
        let (mut stored, mut len) = buffer_with(OLD);
        for _ in 0..64 {
            j.poll(SECOND, NEW, &mut stored, &mut len);
        }

        // Strom wieder da: Was findet der naechste Start vor?
        let mut nvm = j.into_inner();
        nvm.power_on();
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(nvm, HASH, &mut buf);
        match got {
            Loaded::Found { length, .. } => {
                let found = &buf[..length as usize];
                assert!(found == OLD || found == NEW, "Abbruch bei {cut}: weder alt noch neu, sondern {found:?}");
            }
            Loaded::Empty => panic!("Abbruch bei {cut}: der alte Eintrag ging verloren"),
        }
    }
}

#[test]
fn a_cut_during_the_first_write_leaves_no_half_entry() {
    // Ohne Vorgaenger ist „nichts" eine richtige Antwort; ein halber
    // Eintrag waere es nicht. Der erste Vorgang loescht zuerst den ganzen
    // Slot: Abgebrochen wird an jedem Byte von Loeschen, Nutzlast und Kopf.
    let first = |cut: u32| {
        let mut nvm = FakeNvm::<SLOT>::new();
        nvm.cut_at(cut);
        let mut buf = [0u8; SLOT];
        let (mut j, _) = reopen(nvm, HASH, &mut buf);
        let (mut stored, mut len) = ([0u8; SLOT], 0);
        for _ in 0..64 {
            j.poll(0, b"erster", &mut stored, &mut len);
        }
        let mut nvm = j.into_inner();
        let written = nvm.bytes_written();
        nvm.power_on();
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(nvm, HASH, &mut buf);
        (written, got, buf)
    };
    let (total, got, _) = first(u32::MAX);
    assert_eq!(total, SLOT as u32 + 6 + HEADER as u32, "Loeschen, Nutzlast, Kopf");
    assert!(matches!(got, Loaded::Found { .. }));
    let mut found = 0;
    for cut in 0..=total {
        let (_, got, buf) = first(cut);
        if let Loaded::Found { length, .. } = got {
            assert_eq!(&buf[..length as usize], b"erster", "halber Eintrag bei Abbruch {cut}");
            found += 1;
        }
    }
    assert_eq!(found, 1, "nur der vollstaendige Vorgang hinterlaesst einen Eintrag");
}

/// 11.3: Leser akzeptieren aeltere Versionen ihres Formats. Der Test
/// schreibt den Eintrag mit Version 0 und passender Pruefsumme in einen
/// geloeschten Slot, und `load` nimmt ihn.
#[test]
fn an_older_slot_version_still_loads() {
    let (j, _) = with_old(b"alt");
    let mut nvm = j.into_inner();
    let mut raw = [0u8; SLOT];
    let slot = (0..2u8)
        .find(|&s| nvm.read(s, 0, &mut raw) && raw[..4] == MAGIC.to_le_bytes())
        .expect("ein beschriebener Slot");
    raw[4..6].copy_from_slice(&0u16.to_le_bytes());
    let len = u32::from_le_bytes(raw[24..28].try_into().expect("Laenge")) as usize;
    let crc = crc32_final(crc32_update(crc32_update(crc32_start(), &raw[..28]), &raw[HEADER..HEADER + len]));
    raw[28..32].copy_from_slice(&crc.to_le_bytes());
    // Flash loescht Bits nur; ueberschrieben wird nach einer Loeschung.
    assert!(nvm.begin_erase(slot));
    while nvm.poll() == NvmState::Busy {}
    assert!(nvm.begin_write(slot, 0, &raw));
    for _ in 0..64 {
        if nvm.poll() == NvmState::Done {
            break;
        }
    }
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(nvm, HASH, &mut buf);
    assert!(matches!(got, Loaded::Found { length: 3, .. }), "{got:?}");
    assert_eq!(&buf[..3], b"alt");
}

// --- Abbruch beim Slotwechsel (5.9, 8.11) -------------------------------

/// Vier Byte Nutzlast je Eintrag: 36 Byte, drei je Slot.
fn entry(i: u8) -> [u8; 4] {
    [b'e', b'0' + i / 10, b'0' + i % 10, 0]
}

/// Ein Journal, in dem Slot 1 die Eintraege 4 bis 6 traegt (ein aelterer
/// gueltiger Stand) und Slot 0 voll ist mit 7 bis 9; der naechste Vorgang
/// loescht Slot 1 und schreibt dort.
fn before_a_switch() -> Journal<FakeNvm<SLOT>> {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    for i in 1..=9 {
        write(&mut j, i64::from(i) * SECOND, &entry(i), &mut stored, &mut len);
    }
    let nvm = j.into_inner();
    assert_eq!(nvm.slot(0)[36..40], MAGIC.to_le_bytes());
    assert_eq!(nvm.slot(1)[72..76], MAGIC.to_le_bytes());
    let mut buf = [0u8; SLOT];
    let (j, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!(got, Loaded::Found { length: 4, sequence: 9 });
    j
}

/// Bricht den zehnten Eintrag nach `cut` Bytes ab und liefert, was der
/// naechste Start findet; `seed` macht den Rest des Vorgangs unbestimmt.
fn cut_the_switch(cut: u32, seed: Option<u32>) -> Option<Vec<u8>> {
    let mut j = before_a_switch();
    match seed {
        Some(seed) => j.device_mut().cut_at_seeded(cut, seed),
        None => j.device_mut().cut_at(cut),
    }
    let (mut stored, mut len) = buffer_with(&entry(9));
    for _ in 0..64 {
        j.poll(10 * SECOND, &entry(10), &mut stored, &mut len);
    }
    let mut nvm = j.into_inner();
    nvm.power_on();
    let mut buf = [0u8; SLOT];
    match reopen(nvm, HASH, &mut buf).1 {
        Loaded::Found { length, .. } => Some(buf[..length as usize].to_vec()),
        Loaded::Empty => None,
    }
}

/// **Ein Abbruch beim Slotwechsel laesst den juengsten Stand oder den
/// neuen** (5.9: „ein Schreibvorgang aendert genau einen Slot“): fuer jedes
/// Byte von Loeschen, Nutzlast und Kopf — nie leer, nie der aeltere Stand
/// aus dem geloeschten Slot.
#[test]
fn a_cut_during_a_slot_switch_leaves_the_newest_or_the_new_entry() {
    let total = {
        let mut j = before_a_switch();
        j.device_mut().cut_at(u32::MAX);
        let (mut stored, mut len) = buffer_with(&entry(9));
        write(&mut j, 10 * SECOND, &entry(10), &mut stored, &mut len);
        j.into_inner().bytes_written()
    };
    assert_eq!(total, SLOT as u32 + 4 + HEADER as u32, "Loeschen, Nutzlast, Kopf");
    let mut new = 0;
    for cut in 0..=total {
        let found = cut_the_switch(cut, None).unwrap_or_else(|| panic!("Abbruch bei {cut}: nichts mehr da"));
        assert!(found == entry(9) || found == entry(10), "Abbruch bei {cut}: {found:?}");
        new += usize::from(found == entry(10));
    }
    assert_eq!(new, 1, "nur der vollstaendige Vorgang bringt den neuen Stand");
}

/// **Auch ein unbestimmter Rest aendert daran nichts** (8.11: „laesst den
/// Rest unbestimmt“): Nach dem Abbruch bekommt der Rest des Vorgangs
/// Zufall — Programmieren loescht zufaellige Bits, Loeschen setzt sie.
/// Magic, CRC und die Pruefung auf Geloeschtes tragen das.
#[test]
fn an_undetermined_rest_after_a_cut_never_yields_a_wrong_entry() {
    for seed in 1..=16 {
        for cut in (0..SLOT as u32 + 40).step_by(3) {
            let found =
                cut_the_switch(cut, Some(seed)).unwrap_or_else(|| panic!("Saat {seed}, Abbruch bei {cut}: leer"));
            assert!(found == entry(9) || found == entry(10), "Saat {seed}, Abbruch bei {cut}: {found:?}");
        }
    }
    for seed in 1..=16 {
        for cut in 0..48 {
            let (mut j, _) = with_old(b"alter-stand");
            j.device_mut().cut_at_seeded(cut, seed);
            let (mut stored, mut len) = buffer_with(b"alter-stand");
            for _ in 0..64 {
                j.poll(SECOND, b"neuer-stand", &mut stored, &mut len);
            }
            let mut nvm = j.into_inner();
            nvm.power_on();
            let mut buf = [0u8; SLOT];
            let Loaded::Found { length, .. } = reopen(nvm, HASH, &mut buf).1 else {
                panic!("Saat {seed}, Abbruch bei {cut}: der alte Eintrag ging verloren")
            };
            let found = &buf[..length as usize];
            assert!(found == b"alter-stand" || found == b"neuer-stand", "Saat {seed}, Abbruch bei {cut}: {found:?}");
        }
    }
}

// --- Fehler des Geraets (5.9: das Journal faultet nie) ------------------

/// Pollt, bis kein Vorgang mehr laeuft oder `polls` erschoepft sind.
fn settle(j: &mut Journal<FakeNvm<SLOT>>, now: i64, payload: &[u8], stored: &mut [u8], len: &mut usize) {
    for _ in 0..64 {
        j.poll(now, payload, stored, len);
    }
}

/// **Scheitert das Anhaengen, bleibt der alte Stand, und der naechste
/// Vorgang wechselt den Slot**: Der angebrochene Eintrag macht den aktiven
/// Slot unbeschreibbar.
#[test]
fn a_failed_append_keeps_the_old_entry_and_switches_the_slot() {
    for failing in [0, 1] {
        let (mut j, _) = with_old(b"alt");
        j.device_mut().fail_operation(failing);
        let (mut stored, mut len) = buffer_with(b"alt");
        let erases = j.device().erases();
        for _ in 0..64 {
            if j.failures() > 0 {
                break;
            }
            j.poll(SECOND, b"neu", &mut stored, &mut len);
        }
        assert_eq!((j.writes(), j.failures()), (0, 1), "Vorgang {failing} scheitert");
        assert_eq!(len, 0, "der Vergleichsstand gilt nicht mehr");
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(clone(j.device()), HASH, &mut buf);
        assert_eq!((got, &buf[..3]), (Loaded::Found { length: 3, sequence: 1 }, &b"alt"[..]));

        write(&mut j, 2 * SECOND, b"neu", &mut stored, &mut len);
        let nvm = j.into_inner();
        assert_eq!(nvm.erases(), erases + 1, "der Wiederholungsversuch loescht den anderen Slot");
        assert_eq!(nvm.slot(1)[..4], MAGIC.to_le_bytes());
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(nvm, HASH, &mut buf);
        assert_eq!((got, &buf[..3]), (Loaded::Found { length: 3, sequence: 2 }, &b"neu"[..]));
    }
}

/// **Scheitert die Loeschung beim Slotwechsel, geht nichts verloren**, und
/// der naechste Vorgang versucht es erneut.
#[test]
fn a_failed_erase_loses_nothing_and_is_retried() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    for i in 1..=3 {
        write(&mut j, i64::from(i) * SECOND, &entry(i), &mut stored, &mut len);
    }
    j.device_mut().fail_operation(0);
    j.poll(4 * SECOND, &entry(4), &mut stored, &mut len);
    j.poll(4 * SECOND, &entry(4), &mut stored, &mut len);
    assert_eq!((j.writes(), j.failures()), (3, 1));
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(clone(j.device()), HASH, &mut buf);
    assert_eq!((got, &buf[..4]), (Loaded::Found { length: 4, sequence: 3 }, &entry(3)[..]));
    write(&mut j, 5 * SECOND, &entry(4), &mut stored, &mut len);
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
    assert_eq!((got, &buf[..4]), (Loaded::Found { length: 4, sequence: 4 }, &entry(4)[..]));
}

/// **Nach einem Fehler wartet der naechste Versuch `min_interval`** (5.9).
#[test]
fn a_failed_write_is_retried_after_min_interval() {
    let mut j = journal(10 * SECOND);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    j.device_mut().fail_operation(0);
    settle(&mut j, 10 * SECOND, b"b", &mut stored, &mut len);
    assert_eq!(j.failures(), 1);
    settle(&mut j, 15 * SECOND, b"b", &mut stored, &mut len);
    assert_eq!((j.writes(), j.failures()), (1, 1), "vor Ablauf des Intervalls kein neuer Versuch");
    settle(&mut j, 20 * SECOND, b"b", &mut stored, &mut len);
    assert_eq!(j.writes(), 2);
}

/// **Ein Flush, dessen Vorgang scheitert, meldet `false`**; ein frueherer,
/// laengst gescheiterter Vorgang haelt ihn aber nicht davon ab, den Stand
/// zu schreiben (5.9: vor dem Ende wird synchron geschrieben).
#[test]
fn a_flush_reports_its_own_failure_and_retries_an_earlier_one() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    j.device_mut().fail_operation(0);
    assert!(!j.flush(b"b", &mut stored, &mut len, || true), "der Vorgang des Flush scheiterte");

    let mut j = journal(10 * SECOND);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    j.device_mut().fail_operation(0);
    settle(&mut j, 10 * SECOND, b"b", &mut stored, &mut len);
    assert_eq!(j.failures(), 1);
    assert!(j.flush(b"c", &mut stored, &mut len, || true), "der fruehere Fehler blockiert den Flush");
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
    assert_eq!((got, buf[0]), (Loaded::Found { length: 1, sequence: 2 }, b'c'));
}

/// **Ein haengendes Geraet haelt den Flush nicht ewig** (12.7, 12.11): Er
/// gibt auf, wenn der Aufrufer nicht mehr warten will, und meldet `false`.
#[test]
fn a_flush_on_a_hanging_device_gives_up() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    j.device_mut().cut_at(0);
    j.poll(SECOND, b"b", &mut stored, &mut len);
    let mut patience = 100;
    let flushed = j.flush(b"b", &mut stored, &mut len, || {
        patience -= 1;
        patience > 0
    });
    assert!(!flushed);
    assert_eq!(patience, 0, "gewartet, bis der Aufrufer aufgab");
    assert_eq!(j.failures(), 1);
}

/// Eine Kopie des Geraets, ohne dessen Zustand zu veraendern.
fn clone(nvm: &FakeNvm<SLOT>) -> FakeNvm<SLOT> {
    let mut copy = FakeNvm::<SLOT>::new();
    for slot in 0..2u8 {
        assert!(copy.begin_erase(slot));
        while copy.poll() == NvmState::Busy {}
        assert!(copy.begin_write(slot, 0, nvm.slot(slot)));
        while copy.poll() == NvmState::Busy {}
    }
    copy
}

// --- Laden: Versionen, verfaelschte Koepfe, Lesefehler (5.9, 11.3) ------

/// Schreibt den rohen Slot neu, nach einer Loeschung.
fn rewrite(nvm: &mut FakeNvm<SLOT>, slot: u8, raw: &[u8]) {
    assert!(nvm.begin_erase(slot));
    while nvm.poll() == NvmState::Busy {}
    assert!(nvm.begin_write(slot, 0, raw));
    while nvm.poll() == NvmState::Busy {}
}

/// Der CRC eines Eintrags an `at`, ueber Kopf ohne CRC-Feld und Nutzlast.
fn seal(raw: &mut [u8], at: usize) {
    let len = u32::from_le_bytes(raw[at + 24..at + 28].try_into().expect("Laenge")) as usize;
    let crc =
        crc32_final(crc32_update(crc32_update(crc32_start(), &raw[at..at + 28]), &raw[at + HEADER..at + HEADER + len]));
    raw[at + 28..at + 32].copy_from_slice(&crc.to_le_bytes());
}

/// **Eine neuere Version wird abgelehnt** (11.3: Leser akzeptieren aeltere,
/// nicht neuere), auch mit passender Pruefsumme.
#[test]
fn a_newer_slot_version_is_refused() {
    let (j, _) = with_old(b"alt");
    let mut nvm = j.into_inner();
    let mut raw = *nvm.slot(0);
    raw[4..6].copy_from_slice(&3u16.to_le_bytes());
    seal(&mut raw, 0);
    rewrite(&mut nvm, 0, &raw);
    let mut buf = [0u8; SLOT];
    assert_eq!(reopen(nvm, HASH, &mut buf).1, Loaded::Empty);
}

/// **Ein verfaelschter Kopf gilt nicht** — Laenge oder Sequenz: Der CRC
/// deckt den Kopf mit.
#[test]
fn a_tampered_header_invalidates_the_entry() {
    for (field, value) in [(24..28, 2u32.to_le_bytes()), (8..12, 7u32.to_le_bytes())] {
        let (j, _) = with_old(b"alt");
        let mut nvm = j.into_inner();
        let mut raw = *nvm.slot(0);
        raw[field.clone()].copy_from_slice(&value);
        rewrite(&mut nvm, 0, &raw);
        let mut buf = [0u8; SLOT];
        assert_eq!(reopen(nvm, HASH, &mut buf).1, Loaded::Empty, "Feld {field:?}");
    }
}

/// **Ist der juengste Eintrag eines Logs verfaelscht, gilt der davor**
/// (5.9: der gueltige Eintrag mit der hoechsten Sequenznummer).
#[test]
fn a_broken_newest_entry_leaves_the_one_before() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, &entry(1), &mut stored, &mut len);
    write(&mut j, SECOND, &entry(2), &mut stored, &mut len);
    let mut nvm = j.into_inner();
    let mut raw = *nvm.slot(0);
    raw[36 + HEADER] ^= 0xFF;
    rewrite(&mut nvm, 0, &raw);
    let mut buf = [0u8; SLOT];
    let (_, got) = reopen(nvm, HASH, &mut buf);
    assert_eq!((got, &buf[..4]), (Loaded::Found { length: 4, sequence: 1 }, &entry(1)[..]));
}

/// **Scheitert das zweite Lesen des Gewinners, geht nichts verloren**
/// (5.9): Der Stand gilt fuer diesen Start nicht, aber der naechste Vorgang
/// loescht nicht den Slot des Gewinners und schreibt mit hoeherer Nummer —
/// sonst schluege beim naechsten Start ein aelterer Eintrag den neuen.
#[test]
fn a_read_error_on_the_winner_loses_nothing() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    for i in 1..=4 {
        write(&mut j, i64::from(i) * SECOND, &entry(i), &mut stored, &mut len);
    }
    let nvm = j.into_inner();
    let reads = {
        let mut probe = Journal::new(clone(&nvm), HASH, 0);
        probe.load(&mut [0u8; SLOT]);
        probe.device().reads()
    };
    for failing in [reads - 2, reads - 1] {
        let mut nvm = clone(&nvm);
        nvm.fail_read(failing);
        let mut buf = [0u8; SLOT];
        let (mut j, got) = reopen(nvm, HASH, &mut buf);
        assert_eq!(got, Loaded::Empty, "Lesen {failing} scheitert");
        let (mut stored, mut len) = ([0u8; SLOT], 0);
        write(&mut j, 10 * SECOND, &entry(5), &mut stored, &mut len);
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(j.into_inner(), HASH, &mut buf);
        assert_eq!((got, &buf[..4]), (Loaded::Found { length: 4, sequence: 5 }, &entry(5)[..]), "Lesen {failing}");
    }
}

// --- Groessen (5.9, 11.5) -----------------------------------------------

/// **Ein Eintrag, der in keinen Slot passt, wird verweigert** — ohne
/// Loeschung und ohne gezaehlten Erfolg; sonst loeschte jeder Versuch den
/// anderen Slot, und der Eintrag galt beim Laden nie.
#[test]
fn an_entry_larger_than_a_slot_is_refused_without_erasing() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    let big = [7u8; SLOT - HEADER + 1];
    settle(&mut j, 0, &big, &mut stored, &mut len);
    assert_eq!((j.writes(), j.device().erases()), (0, 0));
    assert!(j.failures() > 0, "die Verweigerung ist sichtbar");
}

/// **Ein Stand, der den Vergleichspuffer uebersteigt, wird verweigert statt
/// gekuerzt**: Ein gekuerzter Eintrag waere gueltig und falsch.
#[test]
fn a_state_larger_than_the_stored_buffer_is_refused_not_cut() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; 8], 0);
    settle(&mut j, 0, b"zwoelf-bytes", &mut stored, &mut len);
    assert_eq!(j.writes(), 0, "nichts Gekuerztes geschrieben");
    assert!(j.failures() > 0);
    let mut buf = [0u8; SLOT];
    assert_eq!(reopen(j.into_inner(), HASH, &mut buf).1, Loaded::Empty);
}

/// **`Persist::load` mit zu kurzem Vergleichspuffer bricht nicht ab**: Ein
/// Eintrag, der nicht hineinpasst, gilt nicht.
#[test]
fn loading_into_a_short_stored_buffer_does_not_panic() {
    let mut j = journal(0);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"sechzehn-zeichen", &mut stored, &mut len);
    let (mut current, mut short) = ([0u8; SLOT], [0u8; 8]);
    let mut p = Persist::new(Journal::new(j.into_inner(), HASH, 0), &mut current, &mut short);
    assert_eq!(p.load(&mut Nothing).0, Loaded::Empty);
}

/// Ein Programm ohne `persist`.
struct Nothing;

impl takt_rt_core::Program for Nothing {
    fn tick(&mut self, _k: u64, _now: i64) {}
}
