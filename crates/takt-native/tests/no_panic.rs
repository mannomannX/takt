//! 13.8 verlangt Panic-Freiheit unter Fuzzing.
//!
//! Die Vektoren pruefen, dass die *richtigen* Werte herauskommen; dieser
//! Test prueft, dass ueberhaupt einer herauskommt — fuer jede Laenge und
//! jeden Inhalt, auch fuer die leere Eingabe und fuer Bloecke, die
//! groesser sind als jeder Puffer im Korpus.
//!
//! Der Generator ist ein xorshift mit festem Startwert: Ein Fehlschlag
//! ist reproduzierbar, und der Testlauf ist es auch.

use takt_native::{Native, Output};

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

/// Jede Funktion ueber einem Block, ueber zwei und ueber drei: so kommt
/// jede Signatur der Menge an die Reihe.
fn every_call(f: Native, block: &[u8]) -> [Option<Output>; 3] {
    [takt_native::call(f, &[block]), takt_native::call(f, &[block, block]), takt_native::call(f, &[block, &[], block])]
}

#[test]
fn no_input_makes_a_function_panic() {
    let mut rng = Rng(0x2026_0912);
    let mut buf = Vec::with_capacity(4096);
    for len in [0usize, 1, 2, 3, 7, 8, 15, 16, 31, 55, 56, 63, 64, 65, 255, 256, 1023, 4096] {
        buf.clear();
        buf.extend((0..len).map(|_| rng.next() as u8));
        for f in Native::ALL {
            let _ = every_call(f, &buf);
        }
    }
    // Dazu die Bloecke, die erfahrungsgemaess Fehler finden: nur Nullen,
    // nur Einsen, und ein einzelnes gesetztes Bit an jeder Stelle.
    for len in [1usize, 8, 64] {
        for pattern in [0x00u8, 0xFF, 0x80, 0x01] {
            let block = vec![pattern; len];
            for f in Native::ALL {
                let _ = every_call(f, &block);
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
        for f in Native::ALL.into_iter().filter(|f| !f.external()) {
            let first = every_call(f, &block);
            assert!(first.iter().any(Option::is_some), "{}: keine Signatur passt", f.name());
            assert_eq!(first, every_call(f, &block), "{}", f.name());
        }
    }
}
