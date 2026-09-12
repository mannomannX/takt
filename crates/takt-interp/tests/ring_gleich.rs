//! Der Ring der Runtime und der Puffer des Interpreters sagen dasselbe
//! (8.6, 9.6, Satz 9.4.4).
//!
//! Zwei Implementierungen derselben FIFO: Der Interpreter nutzt
//! `VecDeque` auf dem Host, `takt-rt-core` einen Ring ueber geliehenen
//! Feldern (`no_std`, ohne Allokation). Wo sie sich unterscheiden,
//! weicht der erzeugte Code vom Interpreter ab — und Satz 9.4.4 waere
//! verletzt.
//!
//! Der Test fuehrt beide durch dieselbe Folge von Operationen und
//! vergleicht nach jedem Schritt, was sie sagen: Laenge, Fenster,
//! Nummern, Zaehler.

use takt_interp::stream::{Buffer, Delivery as IDelivery};
use takt_interp::value::Value;
use takt_rt_core::stream::{Delivery as RDelivery, Desc, Ring};

/// Ein Element als Wert und als Bytes — beide Seiten sehen dasselbe.
fn elem(k: u8) -> (Value, [u8; 1]) {
    (Value::UInt(u64::from(k)), [k])
}

/// Vergleicht, was beide ueber ihr Fenster sagen.
fn gleich(b: &Buffer, r: &Ring<'_>, cursor: i64, schritt: &str) {
    assert_eq!(b.count(cursor), r.count(cursor), "{schritt}: `s.count` weicht ab");
    assert_eq!(b.items.len(), r.len(), "{schritt}: die Zahl der Elemente weicht ab");
    assert_eq!(b.end(), r.end(), "{schritt}: die naechste Nummer weicht ab");
    assert_eq!(b.dropped, r.dropped, "{schritt}: `s.dropped` weicht ab");
    assert_eq!(b.overflowed, r.overflowed, "{schritt}: `s.overflowed` weicht ab");
    // Das Fenster Element fuer Element.
    let fenster = b.window(cursor);
    for (i, e) in fenster.iter().enumerate() {
        let d = r.at(cursor, i).unwrap_or_else(|| panic!("{schritt}: Element {i} fehlt im Ring"));
        assert_eq!(e.seq, d.seq, "{schritt}: `seq` von Element {i}");
        assert_eq!(e.t, d.t, "{schritt}: `.t` von Element {i}");
    }
    assert!(r.at(cursor, fenster.len()).is_none(), "{schritt}: der Ring hat ein Element zu viel");
}

/// Beide Puffer mit denselben Schranken.
fn paar(cap: u32, cap_bytes: u32) -> (Buffer, ([Desc; 8], [u8; 32])) {
    assert!(cap <= 8 && cap_bytes <= 32, "die Testfelder fassen mehr als der Puffer");
    (Buffer::new(cap, cap_bytes), ([Desc::default(); 8], [0u8; 32]))
}

#[test]
fn pushing_agrees_on_order_and_numbering() {
    let (mut b, (mut d, mut by)) = paar(4, 16);
    let mut r = Ring::new(&mut d[..4], &mut by[..16]);
    for k in 0..3u8 {
        let (v, bytes) = elem(k);
        let a = b.push(i64::from(k) * 10, v, 1, false);
        let c = r.push(i64::from(k) * 10, &bytes, false);
        assert_eq!(a == IDelivery::Ok, c == RDelivery::Ok, "Schritt {k}: die Lieferung weicht ab");
    }
    gleich(&b, &r, 0, "nach drei Elementen");
    gleich(&b, &r, 2, "mit Cursor 2");
}

/// 8.6: `overflow = fault` — beide verwerfen das ueberzaehlige Element
/// und zaehlen den Ueberlauf.
#[test]
fn overflow_with_fault_agrees() {
    let (mut b, (mut d, mut by)) = paar(2, 8);
    let mut r = Ring::new(&mut d[..2], &mut by[..8]);
    for k in 0..4u8 {
        let (v, bytes) = elem(k);
        let a = b.push(0, v, 1, false);
        let c = r.push(0, &bytes, false);
        assert_eq!(a == IDelivery::Overflow, c == RDelivery::Overflow, "Schritt {k}: das Ueberlaufurteil weicht ab");
    }
    gleich(&b, &r, 0, "nach dem Ueberlauf");
}

/// 8.6: `overflow = drop_oldest` — beide verwerfen dieselbe Zahl.
#[test]
fn overflow_with_drop_oldest_agrees() {
    let (mut b, (mut d, mut by)) = paar(3, 12);
    let mut r = Ring::new(&mut d[..3], &mut by[..12]);
    for k in 0..8u8 {
        let (v, bytes) = elem(k);
        let a = b.push(0, v, 1, true);
        let c = r.push(0, &bytes, true);
        let a_n = match a {
            IDelivery::Dropped(n) => n,
            _ => 0,
        };
        let c_n = match c {
            RDelivery::Dropped(n) => n,
            _ => 0,
        };
        assert_eq!(a_n, c_n, "Schritt {k}: die Zahl der verworfenen Elemente weicht ab");
        gleich(&b, &r, 0, &format!("nach Schritt {k}"));
    }
}

/// 9.6: Eviction ueber das Minimum der Cursor — beide geben dasselbe
/// frei.
#[test]
fn eviction_agrees() {
    let (mut b, (mut d, mut by)) = paar(4, 16);
    let mut r = Ring::new(&mut d[..4], &mut by[..16]);
    for k in 0..4u8 {
        let (v, bytes) = elem(k);
        b.push(0, v, 1, false);
        r.push(0, &bytes, false);
    }
    for min in 0..=4i64 {
        b.evict(min);
        r.evict(min);
        gleich(&b, &r, min, &format!("nach `evict({min})`"));
    }
}

/// Eine lange Folge gemischter Operationen: Was beide nach jedem Schritt
/// sagen, muss gleich sein. Der Ablauf ist deterministisch, damit ein
/// Fehlschlag reproduzierbar ist.
#[test]
fn a_long_mixed_run_agrees_at_every_step() {
    let (mut b, (mut d, mut by)) = paar(4, 16);
    let mut r = Ring::new(&mut d[..4], &mut by[..16]);
    let mut cursor = 0i64;
    // Ein einfacher Generator statt einer Zufallsquelle: Das Crate hat
    // keine Abhaengigkeit, und ein fester Ablauf ist besser zu lesen.
    let mut x: u32 = 12345;
    for schritt in 0..400u32 {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
        match (x >> 16) % 4 {
            0 | 1 => {
                let (v, bytes) = elem((schritt % 251) as u8);
                let a = b.push(i64::from(schritt), v, 1, true);
                let c = r.push(i64::from(schritt), &bytes, true);
                assert_eq!(
                    matches!(a, IDelivery::Ok),
                    matches!(c, RDelivery::Ok),
                    "Schritt {schritt}: die Lieferung weicht ab"
                );
            }
            2 => {
                // Der Konsument sieht ein Element an: „untersucht heisst
                // konsumiert" (8.6).
                if let Some(e) = r.at(cursor, 0) {
                    cursor = e.seq + 1;
                }
            }
            _ => {
                b.evict(cursor);
                r.evict(cursor);
            }
        }
        gleich(&b, &r, cursor, &format!("Schritt {schritt}"));
    }
}
