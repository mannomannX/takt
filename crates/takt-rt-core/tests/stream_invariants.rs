//! Die Invarianten des Strompuffers unter zufälliger Last (9.6.1).
//!
//! Die Tests daneben (`stream.rs`) prüfen einzelne Regeln an gewählten
//! Fällen; hier laufen viele Folgen durch, und nach *jedem* Schritt
//! gelten dieselben Aussagen. Der Unterschied ist die Absicht: Dort
//! steht, was die Referenz verlangt, hier, was nie passieren darf.
//!
//! **Warum ohne Zufallsbibliothek.** Der Kern hat keine Abhängigkeit
//! (12.1), und die Tests sollen dieselbe Grenze achten. Ein linearer
//! Kongruenzgenerator reicht: Er ist deterministisch, und ein
//! Fehlschlag nennt den Startwert, unter dem er wiederkehrt.

use takt_rt_core::stream::{Delivery, Desc, Ring};

/// Ein fester, reproduzierbarer Ablauf.
struct Sequence(u32);

impl Sequence {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.0 >> 16
    }
}

/// Der Inhalt, den das Element mit der Nummer `seq` traegt: Ein
/// vertauschtes oder verschobenes Element faellt so auf.
fn content_of(seq: i64, i: usize) -> u8 {
    (seq.wrapping_mul(7) as u8).wrapping_add(i as u8)
}

/// Was nach jedem Schritt gelten muss (9.6, 9.6.1).
fn invariants(r: &Ring<'_>, cursor: i64, at_label: &str) {
    // Lemma 9.6.1: Der Puffer ist zweidimensional beschränkt.
    assert!(r.len() <= r.capacity(), "{at_label}: mehr Elemente als CAP");
    assert!(r.count(cursor) <= r.len(), "{at_label}: das Fenster ist größer als der Puffer");
    // 9.6: Die Nummern sind streng steigend und lückenlos im Puffer, und
    // jedes Element traegt den Inhalt seiner Nummer.
    let mut vorige: Option<i64> = None;
    let mut bytes = 0;
    for i in 0..r.len() {
        let Some(d) = r.at(i64::MIN, i) else { panic!("{at_label}: Element {i} fehlt") };
        if let Some(v) = vorige {
            assert_eq!(d.seq, v + 1, "{at_label}: die Nummern haben eine Luecke");
        }
        vorige = Some(d.seq);
        let mut buf = [0u8; 8];
        let n = r.read(d, &mut buf);
        let want: Vec<u8> = (0..d.len as usize).map(|i| content_of(d.seq, i)).collect();
        assert_eq!(&buf[..n], &want[..], "{at_label}: Inhalt von Element {}", d.seq);
        bytes += d.len as usize;
    }
    assert!(bytes <= r.capacity_bytes(), "{at_label}: {bytes} Byte, mehr als CAPB");
    // Die nächste Nummer liegt hinter der letzten vergebenen.
    if let Some(last_seen) = vorige {
        assert!(r.end() > last_seen, "{at_label}: `end` liegt nicht hinter der letzten Nummer");
    }
    // Ein Cursor hinter dem Ende sieht ein leeres Fenster.
    assert_eq!(r.count(r.end()), 0, "{at_label}: hinter dem Ende ist das Fenster nicht leer");
}

/// Zufällige Folgen von Einlagern, Lesen und Freigeben; nach jedem
/// Schritt gelten die Invarianten.
#[test]
fn the_invariants_hold_under_random_operations() {
    for seed in [1u32, 7, 42, 1234, 99_991] {
        let (mut d, mut b) = ([Desc::default(); 5], [0u8; 20]);
        let mut r = Ring::new(&mut d, &mut b);
        let mut sequence = Sequence(seed);
        let mut cursor = 0i64;
        for step_of in 0..500u32 {
            let at_label = &format!("Saat {seed}, Schritt {step_of}");
            match sequence.next() % 5 {
                0..=2 => {
                    let n = (sequence.next() % 4 + 1) as usize;
                    let content: Vec<u8> = (0..n).map(|i| content_of(r.end(), i)).collect();
                    r.push(i64::from(step_of), &content, sequence.next() % 2 == 0);
                }
                3 => {
                    if let Some(e) = r.at(cursor, 0) {
                        cursor = e.seq + 1;
                    }
                }
                _ => r.evict(cursor),
            }
            invariants(&r, cursor, at_label);
        }
    }
}

/// Lemma 9.6.1: „Ein mithaltender Konsument erzeugt nie einen
/// Überlauf." Der Test prüft die Umkehrung mit: Wer nicht mithält,
/// verliert — aber nur nach der deklarierten Regel.
#[test]
fn a_consumer_that_keeps_up_never_overflows() {
    for seed in [3u32, 17, 2024] {
        let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
        let mut r = Ring::new(&mut d, &mut b);
        let mut sequence = Sequence(seed);
        let mut cursor = 0i64;
        for step_of in 0..300u32 {
            let n = (sequence.next() % 4 + 1) as usize;
            let content = [7u8; 4];
            assert_eq!(
                r.push(i64::from(step_of), &content[..n], false),
                Delivery::Ok,
                "Saat {seed}, Schritt {step_of}: ein mithaltender Konsument sah einen Ueberlauf"
            );
            // Mithalten heißt: jedes Element ansehen und freigeben.
            while let Some(e) = r.at(cursor, 0) {
                cursor = e.seq + 1;
            }
            r.evict(cursor);
        }
        assert_eq!(r.overflowed, 0);
        assert_eq!(r.dropped, 0);
    }
}

/// Der Inhalt kommt unverändert zurück — auch wenn er im Byte-Ring
/// umbricht. Die Folge ist so gewählt, dass die Naht oft getroffen wird.
#[test]
fn content_survives_the_ring_boundary() {
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 11]);
    let mut r = Ring::new(&mut d, &mut b);
    let mut cursor = 0i64;
    let mut wrapped = 0;
    for step_of in 0..200u32 {
        let n = (step_of % 3 + 1) as usize;
        let content: Vec<u8> = (0..n).map(|i| content_of(r.end(), i)).collect();
        if r.push(0, &content, true) == Delivery::Overflow {
            continue;
        }
        // Alles im Fenster lesen und mit dem Erwarteten vergleichen: Der
        // Inhalt folgt aus der Nummer des Elements.
        let mut i = 0;
        while let Some(e) = r.at(cursor, i) {
            let mut buf = [0u8; 4];
            let gelesen = r.read(e, &mut buf);
            assert_eq!(gelesen, e.len as usize, "Schritt {step_of}: verschieden viele Bytes");
            let want: Vec<u8> = (0..gelesen).map(|j| content_of(e.seq, j)).collect();
            assert_eq!(&buf[..gelesen], &want[..], "Schritt {step_of}: Inhalt von Element {}", e.seq);
            wrapped += usize::from(e.offset as usize + gelesen > 11);
            i += 1;
        }
        if let Some(e) = r.at(cursor, 0) {
            cursor = e.seq + 1;
            r.evict(cursor);
        }
    }
    assert!(wrapped > 0, "kein Element lag ueber der Naht");
}

/// Grenzfälle der Kapazität: ein Platz, ein Byte, gar keiner.
#[test]
fn degenerate_capacities_do_not_panic() {
    // Ein einziger Platz.
    let (mut d, mut b) = ([Desc::default(); 1], [0u8; 2]);
    let mut r = Ring::new(&mut d, &mut b);
    assert_eq!(r.push(0, &[1], false), Delivery::Ok);
    assert_eq!(r.push(0, &[2], false), Delivery::Overflow);
    assert_eq!(r.push(0, &[2], true), Delivery::Dropped(1));
    assert_eq!(r.len(), 1);

    // Kein Byte: Jedes Element ist zu gross.
    let (mut d, mut b) = ([Desc::default(); 4], [0u8; 0]);
    let mut r = Ring::new(&mut d, &mut b);
    assert_eq!(r.push(0, &[1], true), Delivery::Overflow);
    assert!(r.is_empty());

    // Ein leeres Element passt auch in einen Ring ohne Bytes.
    assert_eq!(r.push(0, &[], false), Delivery::Ok);
    assert_eq!(r.len(), 1);
}
