//! Puffer, Fenster und Cursor eines Ereignisstroms (Referenz 8.6, 9.6) samt
//! Lemma 9.6.1: `|W| <= CAP` und `bytes(W) <= CAPB`.

use takt_interp::Value;
use takt_interp::stream::{Buffer, Delivery, byte_len};

/// Puffer mit reichlich Bytebudget, damit nur die Elementschranke greift.
fn buffer(cap: u32) -> Buffer {
    Buffer::new(cap, u32::MAX)
}

fn push(b: &mut Buffer, n: i64) -> Delivery {
    b.push(n * 1_000_000, Value::Int(n), 1, false)
}

#[test]
fn elements_get_strictly_increasing_sequence_numbers() {
    let mut b = buffer(8);
    for n in 0..4 {
        assert_eq!(push(&mut b, n), Delivery::Ok);
    }
    let seqs: Vec<i64> = b.items.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3]);
}

#[test]
fn the_window_starts_at_the_cursor() {
    // 9.6: `W = [e in buf | e.seq >= cur[s, m]]`.
    let mut b = buffer(8);
    for n in 0..5 {
        push(&mut b, n);
    }
    assert_eq!(b.window(0).len(), 5);
    assert_eq!(b.window(3).len(), 2);
    assert_eq!(b.window(5).len(), 0);
    // `count` liest, ohne zu untersuchen (8.6).
    assert_eq!(b.count(3), 2);
}

#[test]
fn overflow_with_fault_rejects_the_surplus_element() {
    // 9.6: „die ueberzaehligen Elemente werden verworfen"; der Fault trifft
    // die Konsumenten bei ihrer naechsten Aktivierung.
    let mut b = buffer(2);
    assert_eq!(push(&mut b, 0), Delivery::Ok);
    assert_eq!(push(&mut b, 1), Delivery::Ok);
    assert_eq!(push(&mut b, 2), Delivery::Overflow);
    assert_eq!(b.items.len(), 2, "der Puffer bleibt bei CAP");
    assert_eq!(b.overflowed, 1);
    // Die vorhandenen Elemente bleiben unangetastet.
    assert_eq!(b.items.front().map(|e| e.seq), Some(0));
}

#[test]
fn overflow_with_drop_oldest_makes_room() {
    let mut b = buffer(2);
    b.push(0, Value::Int(0), 1, true);
    b.push(1, Value::Int(1), 1, true);
    assert_eq!(b.push(2, Value::Int(2), 1, true), Delivery::Dropped(1));
    assert_eq!(b.items.len(), 2);
    assert_eq!(b.dropped, 1);
    // Das aelteste ist weg, das neue drin.
    let seqs: Vec<i64> = b.items.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![1, 2]);
}

#[test]
fn the_byte_bound_triggers_the_same_overflow() {
    // 8.6: die FIFO ist zweidimensional beschraenkt; beide Schranken
    // erzeugen denselben Ueberlauf.
    let mut b = Buffer::new(100, 10);
    assert_eq!(b.push(0, Value::Bytes(vec![0; 6]), 6, false), Delivery::Ok);
    assert_eq!(b.push(1, Value::Bytes(vec![0; 6]), 6, false), Delivery::Overflow);
    assert_eq!(b.items.len(), 1, "die Elementschranke war nicht erreicht");
    assert_eq!(b.bytes, 6);
}

#[test]
fn eviction_removes_what_no_consumer_can_see() {
    // 9.6: „entferne e aus buf[s] mit e.seq < min_m cur[s, m]".
    let mut b = buffer(8);
    for n in 0..5 {
        push(&mut b, n);
    }
    b.evict(3);
    let seqs: Vec<i64> = b.items.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![3, 4]);
    // Die Bytelast wandert mit.
    assert_eq!(b.bytes, 2);
}

#[test]
fn eviction_keeps_the_byte_count_consistent() {
    let mut b = Buffer::new(8, 100);
    b.push(0, Value::Bytes(vec![0; 4]), 4, false);
    b.push(1, Value::Bytes(vec![0; 7]), 7, false);
    assert_eq!(b.bytes, 11);
    b.evict(1);
    assert_eq!(b.bytes, 7);
}

#[test]
fn lemma_9_6_1_bounds_the_window() {
    // Lemma 9.6.1: `|W| <= CAP` und `bytes(W) <= CAPB`, unabhaengig davon,
    // wie viel geliefert wurde und wo der Cursor steht.
    for cap in [1u32, 2, 5, 16] {
        for cap_bytes in [4u32, 32, 1024] {
            let mut b = Buffer::new(cap, cap_bytes);
            for n in 0..64i64 {
                let bytes = (n % 7 + 1) as u32;
                b.push(n, Value::Bytes(vec![0; bytes as usize]), bytes, n % 2 == 0);
            }
            for cursor in [0i64, 1, 8, 63, 64] {
                let w = b.window(cursor);
                assert!(w.len() as u32 <= cap, "|W| = {} > CAP = {cap}", w.len());
                let sum: u32 = w.iter().map(|e| e.bytes).sum();
                assert!(sum <= cap_bytes, "bytes(W) = {sum} > CAPB = {cap_bytes}");
            }
        }
    }
}

#[test]
fn a_consumer_that_keeps_up_never_overflows() {
    // Lemma 9.6.1: „ein mithaltender Konsument erzeugt nie einen Ueberlauf",
    // solange MAXPT * n_m <= CAP gilt. Hier: vier Elemente je Aktivierung,
    // CAP = 8, der Konsument untersucht jedes Mal sein ganzes Fenster.
    let mut b = buffer(8);
    let mut cursor = 0i64;
    for round in 0..20i64 {
        for k in 0..4i64 {
            assert_eq!(push(&mut b, round * 4 + k), Delivery::Ok, "Runde {round}");
        }
        // Der Konsument untersucht alles und schiebt den Cursor.
        let w = b.window(cursor);
        assert!(w.len() <= 8);
        if let Some(last) = w.last() {
            cursor = last.seq + 1;
        }
        b.evict(cursor);
    }
    assert_eq!(b.overflowed, 0);
    assert_eq!(b.dropped, 0);
}

#[test]
fn a_consumer_that_falls_behind_overflows() {
    // Die Gegenprobe: wer nicht untersucht, laeuft ueber.
    let mut b = buffer(4);
    let mut over = 0;
    for n in 0..20i64 {
        if push(&mut b, n) == Delivery::Overflow {
            over += 1;
        }
    }
    assert!(over > 0, "ohne Konsum muss der Puffer ueberlaufen");
    assert_eq!(b.items.len(), 4);
}

#[test]
fn skip_moves_the_cursor_past_everything() {
    // 8.6: „`s.skip()` untersucht alles (verwirft das Fenster)".
    let mut b = buffer(8);
    for n in 0..5 {
        push(&mut b, n);
    }
    let cursor = b.end();
    assert_eq!(b.window(cursor).len(), 0);
    assert_eq!(cursor, 5);
}

#[test]
fn byte_len_counts_the_content_of_variable_elements() {
    // 8.6: Elemente variabler Laenge liegen im Byte-Ring; ihre Last ist der
    // Inhalt, nicht die deklarierte Hoechstlaenge.
    assert_eq!(byte_len(&Value::Bytes(vec![0; 30])), 30);
    assert_eq!(byte_len(&Value::Line { text: "hello".into(), truncated: false }), 5);
    assert_eq!(byte_len(&Value::Str("abc".into())), 3);
    // Elemente fester Groesse zaehlen mindestens ein Byte.
    assert_eq!(byte_len(&Value::Int(7)), 1);
}
