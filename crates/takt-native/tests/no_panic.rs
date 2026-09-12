//! 13.8 verlangt Panic-Freiheit unter Fuzzing.
//!
//! Die Vektoren pruefen, dass die *richtigen* Werte herauskommen; dieser
//! Test prueft, dass ueberhaupt einer herauskommt — fuer jede Laenge und
//! jeden Inhalt, auch fuer die leere Eingabe und fuer Bloecke, die
//! groesser sind als jeder Puffer im Korpus.
//!
//! Der Generator ist ein xorshift mit festem Startwert: Ein Fehlschlag
//! ist reproduzierbar, und der Testlauf ist es auch.

use takt_native::Native;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[test]
fn no_input_makes_a_function_panic() {
    let mut rng = Rng(0x2026_0912);
    let mut buf = Vec::with_capacity(4096);
    for len in [0usize, 1, 2, 3, 7, 8, 15, 16, 31, 64, 255, 256, 1023, 4096] {
        buf.clear();
        buf.extend((0..len).map(|_| rng.next() as u8));
        for f in Native::ALL {
            let _ = takt_native::apply(f, &buf);
        }
    }
    // Dazu die Bloecke, die erfahrungsgemaess Fehler finden: nur Nullen,
    // nur Einsen, und ein einzelnes gesetztes Bit an jeder Stelle.
    for len in [1usize, 8, 64] {
        for pattern in [0x00u8, 0xFF, 0x80, 0x01] {
            let block = vec![pattern; len];
            for f in Native::ALL {
                let _ = takt_native::apply(f, &block);
            }
        }
    }
}

/// Eine Pruefsumme ist eine Funktion ihrer Eingabe: derselbe Block ergibt
/// denselben Wert. Das klingt selbstverstaendlich und ist die Zusage
/// `total` aus 4.5 — ohne sie waere Satz 9.4.4 fuer native Funktionen
/// nicht zu haben.
#[test]
fn the_same_input_gives_the_same_result() {
    let mut rng = Rng(0x1234_5678);
    for _ in 0..64 {
        let len = (rng.next() % 300) as usize;
        let block: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        for f in Native::ALL {
            assert_eq!(takt_native::apply(f, &block), takt_native::apply(f, &block), "{}", f.name());
        }
    }
}

/// Eine Aenderung an einem einzelnen Bit aendert die Pruefsumme.
///
/// Das ist der Zweck einer Pruefsumme, und ein vertauschtes Polynom oder
/// ein falscher Startwert faellt hier auf — nicht immer, aber ueber 200
/// Faelle zuverlaessig.
#[test]
fn a_single_bit_changes_the_checksum() {
    let base = b"Takt ist eine deterministische Sprache fuer Steuerung und Test.";
    for i in 0..base.len().min(50) {
        for bit in 0..8 {
            let mut changed = base.to_vec();
            changed[i] ^= 1 << bit;
            for f in [Native::Crc32, Native::Crc32c, Native::Crc16] {
                assert_ne!(
                    takt_native::apply(f, base),
                    takt_native::apply(f, &changed),
                    "{}: Bit {bit} in Byte {i} aendert nichts",
                    f.name()
                );
            }
        }
    }
}
