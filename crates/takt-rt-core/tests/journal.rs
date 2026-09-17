//! Das `persist`-Journal (Referenz 5.9).
//!
//! Die Anforderungen der Reihe nach: Ping-Pong, Sequenznummer, CRC32,
//! „beim Start gewinnt der gueltige Eintrag mit der hoeheren
//! Sequenznummer", `min_interval` — und die Stromausfallsicherheit, die
//! 8.11 mit `CUT_AT_BYTE` prueft.

use takt_native::crc::{crc32_final, crc32_start, crc32_update};
use takt_rt_core::journal::{FakeNvm, HEADER, Journal, Loaded, MAGIC};
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
    // 5.9: Vor `reboot`, `boot_jump` und Deep Sleep wird synchron
    // geschrieben — dort begrenzt kein Intervall mehr den Verschleiss.
    let mut j = journal(10 * SECOND);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    write(&mut j, 0, b"a", &mut stored, &mut len);
    assert!(j.flush(b"b", &mut stored, &mut len));
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
    // Eintrag waere es nicht.
    for cut in 0..80 {
        let mut nvm = FakeNvm::<SLOT>::new();
        nvm.cut_at(cut);
        let mut buf = [0u8; SLOT];
        let (mut j, _) = reopen(nvm, HASH, &mut buf);
        let (mut stored, mut len) = ([0u8; SLOT], 0);
        for _ in 0..64 {
            j.poll(0, b"erster", &mut stored, &mut len);
        }
        let mut nvm = j.into_inner();
        nvm.power_on();
        let mut buf = [0u8; SLOT];
        let (_, got) = reopen(nvm, HASH, &mut buf);
        if let Loaded::Found { length, .. } = got {
            assert_eq!(&buf[..length as usize], b"erster", "halber Eintrag bei Abbruch {cut}");
        }
    }
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
