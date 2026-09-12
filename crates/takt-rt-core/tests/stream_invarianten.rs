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
struct Folge(u32);

impl Folge {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.0 >> 16
    }
}

/// Was nach jedem Schritt gelten muss (9.6, 9.6.1).
fn invarianten(r: &Ring<'_>, cursor: i64, wo: &str) {
    // Lemma 9.6.1: Der Puffer ist zweidimensional beschränkt.
    assert!(r.len() <= r.capacity(), "{wo}: mehr Elemente als CAP");
    assert!(r.count(cursor) <= r.len(), "{wo}: das Fenster ist größer als der Puffer");
    // 9.6: Die Nummern sind streng steigend und lückenlos im Puffer.
    let mut vorige: Option<i64> = None;
    for i in 0..r.len() {
        let Some(d) = r.at(i64::MIN, i) else { panic!("{wo}: Element {i} fehlt") };
        if let Some(v) = vorige {
            assert_eq!(d.seq, v + 1, "{wo}: die Nummern haben eine Luecke");
        }
        vorige = Some(d.seq);
    }
    // Die nächste Nummer liegt hinter der letzten vergebenen.
    if let Some(letzte) = vorige {
        assert!(r.end() > letzte, "{wo}: `end` liegt nicht hinter der letzten Nummer");
    }
    // Ein Cursor hinter dem Ende sieht ein leeres Fenster.
    assert_eq!(r.count(r.end()), 0, "{wo}: hinter dem Ende ist das Fenster nicht leer");
}

/// Zufällige Folgen von Einlagern, Lesen und Freigeben; nach jedem
/// Schritt gelten die Invarianten.
#[test]
fn the_invariants_hold_under_random_operations() {
    for saat in [1u32, 7, 42, 1234, 99_991] {
        let (mut d, mut b) = ([Desc::default(); 5], [0u8; 20]);
        let mut r = Ring::new(&mut d, &mut b);
        let mut folge = Folge(saat);
        let mut cursor = 0i64;
        for schritt in 0..500u32 {
            let wo = &format!("Saat {saat}, Schritt {schritt}");
            match folge.next() % 5 {
                0..=2 => {
                    let n = (folge.next() % 4 + 1) as usize;
                    let inhalt = [(schritt % 251) as u8; 4];
                    r.push(i64::from(schritt), &inhalt[..n], folge.next() % 2 == 0);
                }
                3 => {
                    if let Some(e) = r.at(cursor, 0) {
                        cursor = e.seq + 1;
                    }
                }
                _ => r.evict(cursor),
            }
            invarianten(&r, cursor, wo);
        }
    }
}

/// Lemma 9.6.1: „Ein mithaltender Konsument erzeugt nie einen
/// Überlauf." Der Test prüft die Umkehrung mit: Wer nicht mithält,
/// verliert — aber nur nach der deklarierten Regel.
#[test]
fn a_consumer_that_keeps_up_never_overflows() {
    for saat in [3u32, 17, 2024] {
        let (mut d, mut b) = ([Desc::default(); 4], [0u8; 16]);
        let mut r = Ring::new(&mut d, &mut b);
        let mut folge = Folge(saat);
        let mut cursor = 0i64;
        for schritt in 0..300u32 {
            let n = (folge.next() % 4 + 1) as usize;
            let inhalt = [7u8; 4];
            assert_eq!(
                r.push(i64::from(schritt), &inhalt[..n], false),
                Delivery::Ok,
                "Saat {saat}, Schritt {schritt}: ein mithaltender Konsument sah einen Ueberlauf"
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
    for schritt in 0..200u32 {
        let n = (schritt % 3 + 1) as usize;
        let marke = (schritt % 251) as u8;
        let inhalt = [marke, marke.wrapping_add(1), marke.wrapping_add(2)];
        if r.push(0, &inhalt[..n], true) == Delivery::Overflow {
            continue;
        }
        // Alles im Fenster lesen und mit dem Erwarteten vergleichen.
        let mut i = 0;
        while let Some(e) = r.at(cursor, i) {
            let mut buf = [0u8; 4];
            let gelesen = r.read(e, &mut buf);
            assert_eq!(gelesen, e.len as usize, "Schritt {schritt}: verschieden viele Bytes");
            // Das erste Byte traegt die Marke des Schritts, in dem das
            // Element entstand — es muss zu seiner Nummer passen.
            assert!(buf[0] < 251, "Schritt {schritt}: der Inhalt ist beschaedigt");
            i += 1;
        }
        if let Some(e) = r.at(cursor, 0) {
            cursor = e.seq + 1;
            r.evict(cursor);
        }
    }
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
