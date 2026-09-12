//! Der Strompuffer gegen die Regeln aus 8.6 und 9.6.
//!
//! Die Semantik steht dort als ASCII-Block; die Tests gehen ihn Zeile
//! fuer Zeile durch. Was hier gruen ist, muss der Interpreter genauso
//! halten — der differentielle Test in `takt-interp` misst das.

use takt_rt_core::stream::{Delivery, Desc, Ring};

/// Ein Ring mit fester Elementgroesse: `n` Slots zu je einem Byte.
fn ring<'a>(descs: &'a mut [Desc], bytes: &'a mut [u8]) -> Ring<'a> {
    Ring::new(descs, bytes)
}

/// Der Inhalt eines Elements als Byte.
fn byte_at(r: &Ring<'_>, cursor: i64, i: usize) -> Option<u8> {
    let d = r.at(cursor, i)?;
    let mut buf = [0u8; 8];
    let n = r.read(d, &mut buf);
    (n > 0).then_some(buf[0])
}

// --- Einlagern und Fenster ----------------------------------------------

#[test]
fn elements_keep_their_order_and_numbering() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
    let mut r = ring(&mut d, &mut b);
    for k in 0..3u8 {
        assert_eq!(r.push(i64::from(k) * 1000, &[k], false), Delivery::Ok);
    }
    assert_eq!(r.len(), 3);
    // 9.6: `seq` ist streng steigend und beginnt bei null.
    for i in 0..3 {
        let e = r.at(0, i).expect("Element");
        assert_eq!(e.seq, i as i64);
        assert_eq!(e.t, i as i64 * 1000);
        assert_eq!(byte_at(&r, 0, i), Some(i as u8));
    }
}

/// 9.6: Das Fenster sind die Elemente ab dem Cursor des Konsumenten.
#[test]
fn the_window_starts_at_the_cursor() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
    let mut r = ring(&mut d, &mut b);
    for k in 0..4u8 {
        r.push(0, &[k], false);
    }
    assert_eq!(r.count(0), 4);
    assert_eq!(r.count(2), 2, "ab `seq >= 2` sind es zwei");
    assert_eq!(byte_at(&r, 2, 0), Some(2), "das erste im Fenster ist Nummer 2");
    assert_eq!(r.count(4), 0, "hinter dem letzten ist das Fenster leer");
    assert_eq!(r.at(4, 0), None);
}

/// 8.6: `s.count` liest, ohne zu untersuchen — es konsumiert nichts.
#[test]
fn counting_does_not_consume() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, &[7], false);
    assert_eq!(r.count(0), 1);
    assert_eq!(r.count(0), 1, "zweimal zaehlen aendert nichts");
    assert_eq!(r.len(), 1);
}

// --- Ueberlauf ----------------------------------------------------------

/// 8.6: `overflow = fault` verwirft das ueberzaehlige Element; der Fault
/// trifft die Konsumenten bei ihrer naechsten Aktivierung.
#[test]
fn a_full_ring_with_fault_policy_drops_the_newcomer() {
    let (mut d, mut b) = ([Desc::default(); 2], [0u8; 8]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, &[1], false);
    r.push(0, &[2], false);
    assert_eq!(r.push(0, &[3], false), Delivery::Overflow);
    assert_eq!(r.overflowed, 1);
    assert_eq!(r.len(), 2, "die alten bleiben");
    assert_eq!(byte_at(&r, 0, 0), Some(1));
}

/// 8.6: `overflow = drop_oldest` verwirft die aeltesten und zaehlt sie.
#[test]
fn drop_oldest_makes_room() {
    let (mut d, mut b) = ([Desc::default(); 2], [0u8; 8]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, &[1], true);
    r.push(0, &[2], true);
    assert_eq!(r.push(0, &[3], true), Delivery::Dropped(1));
    assert_eq!(r.dropped, 1);
    assert_eq!(r.len(), 2);
    // Die Nummern laufen weiter: `seq` ist streng steigend (9.6).
    assert_eq!(r.at(0, 0).map(|e| e.seq), Some(1));
    assert_eq!(byte_at(&r, 0, 0), Some(2));
    assert_eq!(byte_at(&r, 0, 1), Some(3));
}

/// 8.6: Die FIFO ist zweidimensional beschraenkt — Elemente *und* Bytes.
#[test]
fn the_byte_ring_bounds_independently() {
    let (mut d, mut b) = ([Desc::default(); 8], [0u8; 6]);
    let mut r = ring(&mut d, &mut b);
    assert_eq!(r.push(0, b"abcd", false), Delivery::Ok);
    // Vier von sechs Byte belegt; zwei passen noch, drei nicht.
    assert_eq!(r.push(0, b"efg", false), Delivery::Overflow, "die Byteschranke greift vor der Elementzahl");
    assert_eq!(r.push(0, b"ef", false), Delivery::Ok);
}

/// Ein Element, das allein die Byteschranke sprengt, ist ein Ueberlauf
/// und kein Verwerfen: Der geleerte Ring naehme es auch nicht.
#[test]
fn an_oversized_element_never_empties_the_ring() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 4]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, b"ab", true);
    assert_eq!(r.push(0, b"xxxxx", true), Delivery::Overflow);
    assert_eq!(r.len(), 1, "der Ring wurde nicht geraeumt");
    assert_eq!(r.dropped, 0);
}

// --- Eviction -----------------------------------------------------------

/// 9.6: Ein Element verlaesst den Puffer erst, wenn *alle* Konsumenten
/// daran vorbei sind — das Minimum ueber die Cursor entscheidet.
#[test]
fn eviction_waits_for_the_slowest_consumer() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
    let mut r = ring(&mut d, &mut b);
    for k in 0..4u8 {
        r.push(0, &[k], false);
    }
    // Ein Konsument steht bei 3, der andere bei 1: Das Minimum gilt.
    r.evict(1);
    assert_eq!(r.len(), 3, "nur Nummer 0 faellt weg");
    assert_eq!(r.at(1, 0).map(|e| e.seq), Some(1));
    r.evict(4);
    assert!(r.is_empty(), "hinter dem letzten Cursor bleibt nichts");
}

/// Nach der Eviction laufen die Nummern weiter; sie sind kein Index.
#[test]
fn sequence_numbers_survive_eviction() {
    let (mut d, mut b) = ([Desc::default(); 2], [0u8; 8]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, &[1], false);
    r.push(0, &[2], false);
    r.evict(2);
    r.push(0, &[3], false);
    assert_eq!(r.at(0, 0).map(|e| e.seq), Some(2), "die dritte Nummer ist 2");
    assert_eq!(r.end(), 3);
}

// --- Der Ring als Ring --------------------------------------------------

/// Der Byte-Ring bricht um: Ein Inhalt kann ueber die Naht laufen, und
/// `read` muss ihn trotzdem vollstaendig liefern.
#[test]
fn content_that_wraps_is_read_whole() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 8]);
    let mut r = ring(&mut d, &mut b);
    r.push(0, b"abcdef", true);
    r.evict(1);
    // Sechs Byte sind frei ab Versatz 6: Der naechste Inhalt bricht um.
    assert_eq!(r.push(0, b"wxyz", true), Delivery::Ok);
    let e = r.at(1, 0).expect("Element");
    let mut buf = [0u8; 4];
    assert_eq!(r.read(e, &mut buf), 4);
    assert_eq!(&buf, b"wxyz", "der Inhalt laeuft ueber die Naht");
}

/// Viele Durchlaeufe: Der Ring bleibt beschraenkt und verliert nichts,
/// was er behalten soll (Lemma 9.6.1).
#[test]
fn a_keeping_consumer_never_loses_an_element() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
    let mut r = ring(&mut d, &mut b);
    let mut cursor = 0i64;
    for k in 0..200u32 {
        assert_eq!(r.push(0, &[(k % 251) as u8], false), Delivery::Ok, "bei {k}");
        // Der Konsument haelt mit: Er sieht jedes Element genau einmal.
        assert_eq!(r.count(cursor), 1);
        let e = r.at(cursor, 0).expect("Element");
        assert_eq!(e.seq, i64::from(k));
        cursor = e.seq + 1;
        r.evict(cursor);
    }
    assert!(r.is_empty());
    assert_eq!(r.overflowed, 0, "ein mithaltender Konsument erzeugt nie einen Ueberlauf (9.6.1)");
    assert_eq!(r.dropped, 0);
}

/// Die Schranken gelten auch unter Last: `|W| <= CAP` und
/// `bytes(W) <= CAPB` (Lemma 9.6.1).
#[test]
fn the_bounds_hold_under_pressure() {
    let (mut d, mut b) = ([Desc::default(); 3], [0u8; 12]);
    let mut r = ring(&mut d, &mut b);
    for k in 0..100u32 {
        r.push(0, &[(k % 7) as u8, 0, 0], true);
        assert!(r.len() <= r.capacity(), "die Elementschranke haelt");
        assert!(r.count(0) <= r.capacity(), "das Fenster ist durch CAP beschraenkt");
    }
}

/// Ein Ring ohne Plaetze nimmt nichts an — und stuerzt nicht ab.
#[test]
fn a_ring_without_slots_overflows() {
    let (mut d, mut b): ([Desc; 0], [u8; 4]) = ([], [0u8; 4]);
    let mut r = ring(&mut d, &mut b);
    assert_eq!(r.push(0, &[1], true), Delivery::Overflow);
    assert_eq!(r.capacity(), 0);
    assert!(r.is_empty());
}
