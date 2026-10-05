//! `takt_m_scale_f64` und `takt_m_scale_f32` (3.2, INT-008): dieselbe
//! Rechnung wie `libtaktm::scale_*`, die Faktoren als zwei Haelften eines
//! `u128`.

use takt_native_abi::math::{takt_m_scale_f32, takt_m_scale_f64};

/// Die Haelften eines `u128`, wie der erzeugte Code sie uebergibt.
fn halves(v: u128) -> (u64, u64) {
    (v as u64, (v >> 64) as u64)
}

/// **Der Einstieg rechnet bitgleich wie `libtaktm`**, auch mit Faktoren
/// ueber 2^64, Null im Zaehler, Null im Nenner und nicht endlichen
/// Eingaben.
#[test]
fn the_entries_scale_exactly_like_libtaktm() {
    let factors: [(u128, u128); 6] =
        [(1, 1), (100_000, 6_894_757), (6_894_757, 100_000), ((3u128 << 64) | 5, (1u128 << 70) + 1), (0, 7), (5, 0)];
    let xs = [0.0, -0.0, 1.0, -2.5, 1e300, f64::MIN_POSITIVE, 5e-324, f64::MAX, f64::INFINITY, f64::NAN];
    for (num, den) in factors {
        let ((nl, nh), (dl, dh)) = (halves(num), halves(den));
        for x in xs {
            let got = takt_m_scale_f64(x, nl, nh, dl, dh);
            let want = libtaktm::scale_f64(x, num, den);
            assert_eq!(got.to_bits(), want.to_bits(), "f64 {x:e} * {num} / {den}");
            let x32 = x as f32;
            let got = takt_m_scale_f32(x32, nl, nh, dl, dh);
            let want = libtaktm::scale_f32(x32, num, den);
            assert_eq!(got.to_bits(), want.to_bits(), "f32 {x32:e} * {num} / {den}");
        }
    }
}

/// Die Haelften setzen sich in der richtigen Reihenfolge zusammen: Ein
/// Faktor 2^64 im Zaehler verdoppelt nicht nur, er schiebt.
#[test]
fn the_high_half_counts_two_to_the_sixty_four() {
    let x = 1.0f64;
    let got = takt_m_scale_f64(x, 0, 1, 1, 0);
    assert_eq!(got, 18_446_744_073_709_551_616.0);
    let back = takt_m_scale_f64(got, 1, 0, 0, 1);
    assert_eq!(back, 1.0);
}
