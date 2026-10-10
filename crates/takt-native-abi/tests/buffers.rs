//! Die Puffervertraege der C-Einstiege (4.5, 5.9, 12.6; FB-397, INT-034):
//! Jeder Einstieg schreibt genau so viele Bytes, wie sein Vertrag sagt —
//! Waechterbytes um jeden Ausgabepuffer zeigen ein Byte zu viel —, und
//! nimmt jede Eingabe: eine Laenge unter eins und ein Nullzeiger sind leer.
//!
//! Laeuft ohne `host`: Dessen Panic-Handler gehoert der statischen
//! Bibliothek, ein Testbinary bringt den von `std` mit.

#![allow(unsafe_code, reason = "die Einstiege sind `unsafe extern \"C\"`; jeder Aufruf haelt ihren Vertrag")]

use takt_hal::contract::{Delivery, Device, Event, NONE, Track, Window};
use takt_native::sha256::{CTX_MAX_BYTES, sha256};
use takt_native_abi::edge::takt_edge_settle;
use takt_native_abi::{
    DIGEST_BYTES, takt_native_crc32, takt_native_fft256, takt_native_map_get, takt_native_map_insert,
    takt_native_map_len, takt_native_map_remove, takt_native_sha256, takt_native_sha256_final, takt_native_sha256_init,
    takt_native_sha256_update,
};

/// So viele Waechterbytes stehen vor und hinter jedem Ausgabepuffer.
const GUARD: usize = 16;
const MARK: u8 = 0xA5;

/// Ein Ausgabepuffer von `n` Byte zwischen zwei Waechterzonen.
struct Guarded(Vec<u8>);

impl Guarded {
    fn new(n: usize) -> Guarded {
        Guarded(vec![MARK; n + 2 * GUARD])
    }

    /// Wie [`Guarded::new`], der Puffer selbst mit Nullen, wie die Arena nach `init`.
    fn zeroed(n: usize) -> Guarded {
        let mut g = Guarded::new(n);
        g.0[GUARD..GUARD + n].fill(0);
        g
    }

    fn out(&mut self) -> *mut u8 {
        self.0[GUARD..].as_mut_ptr()
    }

    /// Der Puffer selbst; die Waechter muessen unberuehrt sein.
    fn inner(&self) -> &[u8] {
        let n = self.0.len() - 2 * GUARD;
        assert!(self.0[..GUARD].iter().all(|b| *b == MARK), "vor dem Puffer geschrieben");
        assert!(self.0[GUARD + n..].iter().all(|b| *b == MARK), "hinter den Puffer geschrieben");
        &self.0[GUARD..GUARD + n]
    }
}

/// Die Laenge der kanonischen Form eines `Sha256Ctx` (5.9): acht Worte,
/// der gefuellte Block mit Laenge, die Gesamtzahl. Der erzeugte Code
/// uebergibt genau sie (`encode_canonical`), nicht die Puffergroesse.
fn ctx_len(ctx: &[u8]) -> i32 {
    36 + i32::from_le_bytes(ctx[32..36].try_into().expect("Laenge")) + 8
}

fn digest_of(data: &[u8]) -> Vec<u8> {
    let mut want = 32u32.to_le_bytes().to_vec();
    want.extend_from_slice(&sha256(data));
    want
}

/// **`sha256` schreibt genau 36 Byte**: Laenge und Digest (5.9).
#[test]
fn a_digest_fills_exactly_its_36_bytes() {
    let mut out = Guarded::new(DIGEST_BYTES);
    // SAFETY: drei lesbare Bytes, 36 schreibbare.
    unsafe { takt_native_sha256(b"abc".as_ptr(), 3, out.out()) };
    assert_eq!(out.inner(), &digest_of(b"abc")[..]);
}

/// **Eine Laenge unter eins und ein Nullzeiger sind leer** (Totalitaet, 4.1).
#[test]
fn a_length_below_one_or_a_null_pointer_is_empty() {
    let empty = digest_of(b"");
    for (p, n) in [(b"abc".as_ptr(), 0), (b"abc".as_ptr(), -1), (b"abc".as_ptr(), i32::MIN), (std::ptr::null(), 3)] {
        let mut out = Guarded::new(DIGEST_BYTES);
        // SAFETY: Bei positiver Laenge zeigt `p` auf drei Bytes oder ist null.
        unsafe { takt_native_sha256(p, n, out.out()) };
        assert_eq!(out.inner(), &empty[..], "Laenge {n}");
        // SAFETY: wie oben.
        assert_eq!(unsafe { takt_native_crc32(p, n) }, takt_native::crc::crc32(b""), "Laenge {n}");
    }
    // SAFETY: ein Nullzeiger als Ziel schreibt nichts.
    unsafe { takt_native_sha256(b"abc".as_ptr(), 3, std::ptr::null_mut()) };
}

/// **Ein `Sha256Ctx` macht die Runde**: init, zwei updates, final ergibt
/// den Digest des Ganzen; jeder Zustand passt in `CTX_MAX_BYTES`, und was
/// kein Zustand ist, gilt als der leere.
#[test]
fn a_sha256_context_makes_the_round_trip() {
    let mut a = Guarded::new(CTX_MAX_BYTES);
    let mut b = Guarded::new(CTX_MAX_BYTES);
    let mut c = Guarded::new(CTX_MAX_BYTES);
    let mut d = Guarded::new(DIGEST_BYTES);
    // SAFETY: jeder Puffer fasst seinen Vertrag; Ein- und Ausgabe ueberlappen nicht.
    unsafe {
        takt_native_sha256_init(a.out());
        takt_native_sha256_update(a.inner().as_ptr(), ctx_len(a.inner()), b"hello ".as_ptr(), 6, b.out());
        takt_native_sha256_update(b.inner().as_ptr(), ctx_len(b.inner()), b"world".as_ptr(), 5, c.out());
        takt_native_sha256_final(c.inner().as_ptr(), ctx_len(c.inner()), d.out());
    }
    assert_eq!(ctx_len(c.inner()), 36 + 11 + 8, "elf Byte im Block");
    assert_eq!(d.inner(), &digest_of(b"hello world")[..]);

    let mut e = Guarded::new(DIGEST_BYTES);
    // SAFETY: vier Bytes Unsinn als Zustand, 36 schreibbare.
    unsafe { takt_native_sha256_final([1u8, 2, 3, 4].as_ptr(), 4, e.out()) };
    assert_eq!(e.inner(), &digest_of(b"")[..], "kein Zustand ist der leere");
}

/// **`fft256` schreibt genau `n` Byte**, und eine falsche Laenge schreibt
/// nichts: Der Ausgang behaelt seinen alten Inhalt.
#[test]
fn fft256_writes_exactly_its_length_or_nothing() {
    for n in [1024usize, 2048] {
        let input = vec![0u8; n];
        let mut out = Guarded::new(n);
        // SAFETY: `n` lesbare und `n` schreibbare Bytes.
        unsafe { takt_native_fft256(input.as_ptr(), n as i32, out.out()) };
        assert!(out.inner().iter().all(|b| *b == 0), "die Transformierte von null ist null ({n} Byte)");
    }
    for n in [0usize, 1, 1023, 1025, 4096] {
        let input = vec![1u8; n];
        let mut out = Guarded::new(n);
        // SAFETY: `n` lesbare und `n` schreibbare Bytes.
        unsafe { takt_native_fft256(input.as_ptr(), n as i32, out.out()) };
        assert!(out.inner().iter().all(|b| *b == MARK), "{n} Byte: nichts geschrieben");
    }
}

/// **Eine Map haelt ihre Slots ein** (3.9): `cap` Slots zu `1 + klen +
/// vlen` Byte; `get` schreibt genau `vlen` Byte; ohne Slots, mit
/// negativer Groesse oder voll bleibt alles beim Alten.
#[test]
fn a_map_stays_within_its_slots() {
    const CAP: usize = 4;
    const K: usize = 2;
    const V: usize = 3;
    let mut slots = Guarded::zeroed(CAP * (1 + K + V));
    let (cap, k, v) = (CAP as i32, K as i32, V as i32);
    // SAFETY: die Slots fassen `cap * (1 + klen + vlen)` Byte; Schluessel
    // und Werte haben ihre Laenge.
    unsafe {
        let s = slots.out();
        assert_eq!(takt_native_map_len(s, cap, k, v), 0);
        for i in 0..CAP as u8 {
            assert!(takt_native_map_insert(s, cap, k, v, [i, 0].as_ptr(), [i, i, i].as_ptr()));
        }
        assert!(!takt_native_map_insert(s, cap, k, v, [9, 9].as_ptr(), [9, 9, 9].as_ptr()), "voll");
        assert_eq!(takt_native_map_len(s, cap, k, v), CAP as i32);
        let mut out = Guarded::new(V);
        assert!(takt_native_map_get(s, cap, k, v, [2, 0].as_ptr(), out.out()));
        assert_eq!(out.inner(), &[2, 2, 2]);
        assert!(!takt_native_map_get(s, cap, k, v, [7, 0].as_ptr(), out.out()), "unbekannter Schluessel");
        assert!(takt_native_map_remove(s, cap, k, v, [2, 0].as_ptr()));
        assert_eq!(takt_native_map_len(s, cap, k, v), CAP as i32 - 1);
        assert_eq!(takt_native_map_len(std::ptr::null_mut(), cap, k, v), 0, "ohne Slots");
        assert!(!takt_native_map_insert(std::ptr::null_mut(), cap, k, v, [1, 0].as_ptr(), [1, 1, 1].as_ptr()));
        assert_eq!(takt_native_map_len(s, -1, k, v), 0, "negative Kapazitaet ist keine");
    }
    slots.inner();
}

/// **Der Rand schreibt hoechstens `max_events` Meldungen** (12.6): Mehr
/// Ereignisse, als das Feld fasst, laufen nicht darueber hinaus.
#[test]
fn the_edge_writes_at_most_max_events() {
    let mut tracks = [Track::new(0); 3];
    let mut devices = [Device::default(); 3];
    let device_of = [0u32, 1, 2];
    // Drei Treiber, jeder mit einem Zeitstempel knapp neben dem Fenster:
    // drei Klemmungen, also drei Meldungen.
    let delivery =
        |channel| Delivery { channel, element: false, inconsistent: false, t: 2_100, age: 0, seq: 0, at: NONE };
    let mut deliveries = [delivery(0), delivery(1), delivery(2)];
    let window = Window { lo: 1_000, hi: 2_000, tolerance: 500 };
    let mut events = [Event { kind: 0xEE, what: 0xEE, index: 0xEEEE }; 3];
    // SAFETY: jedes Feld hat so viele Eintraege, wie daneben steht.
    let n = unsafe {
        takt_edge_settle(
            tracks.as_mut_ptr(),
            3,
            devices.as_mut_ptr(),
            3,
            device_of.as_ptr(),
            deliveries.as_mut_ptr(),
            3,
            window,
            events.as_mut_ptr(),
            2,
        )
    };
    assert_eq!(n, 2, "zwei Plaetze, zwei Meldungen");
    assert_eq!(events[2], Event { kind: 0xEE, what: 0xEE, index: 0xEEEE }, "der dritte Platz bleibt");
    assert!(deliveries.iter().all(|d| d.at == 2_000), "geklemmt wird trotzdem jede");
    // SAFETY: ohne Felder ist nichts zu lesen und nichts zu schreiben.
    let none = unsafe {
        takt_edge_settle(
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
            window,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(none, 0);
}
