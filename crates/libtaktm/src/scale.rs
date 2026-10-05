//! `x · num / den`, korrekt gerundet: die Einheitenumrechnung `x.to(U)`
//! (3.2, INT-008).
//!
//! Der Faktor ist der exakte Bruch zweier Einheiten. `x · num` und danach
//! `/ den` in Gleitkomma runden zweimal und laufen bei `1e300 psi .to(bar)`
//! an einem Zwischenwert ueber. Hier rechnet Ganzzahlarithmetik das
//! Produkt exakt und teilt mit Rest; gerundet wird einmal, mit dem Rest als
//! Sticky-Bit — wie bei den Funktionen aus 4.2 ohne einen Schritt in der FPU.
//!
//! Die Groessen: Die Mantisse hat hoechstens 53 Bit, Zaehler und Nenner
//! hoechstens 128; das Produkt passt mit der Verschiebung fuer den Quotienten
//! (Genauigkeit plus drei Bit) in 256 Bit.

use crate::big::{Class, F32, F64, Format, classify};

/// `x · num / den`, korrekt gerundet. Ein Nenner null oder ein Faktor null
/// mal ∞ ist NaN; sonst gilt das Vorzeichen von `x`.
pub fn scale_f64(x: f64, num: u128, den: u128) -> f64 {
    f64::from_bits(scale(x.to_bits(), num, den, F64))
}

/// `x · num / den`, korrekt gerundet in `f32`.
pub fn scale_f32(x: f32, num: u128, den: u128) -> f32 {
    f32::from_bits(scale(u64::from(x.to_bits()), num, den, F32) as u32)
}

/// Das Bitmuster von `x · num / den` in der Breite `f`.
fn scale(bits: u64, num: u128, den: u128, f: Format) -> u64 {
    if den == 0 {
        return f.nan();
    }
    match classify(bits, f) {
        Class::Nan => f.nan(),
        Class::Inf { .. } if num == 0 => f.nan(),
        Class::Inf { neg } => f.inf(neg),
        Class::Zero { neg } => f.zero(neg),
        Class::Finite { neg, .. } if num == 0 => f.zero(neg),
        Class::Finite { neg, mant, exp } => {
            let product = U256::mul(mant, num);
            let divisor = U256::from_u128(den);
            // Der Quotient bekommt mindestens Genauigkeit plus zwei Bit,
            // damit Rundungs- und Sticky-Bit in ihm liegen.
            let shift = (f.p as i32 + 3 + divisor.bits() as i32 - product.bits() as i32).max(0);
            let (q, r) = product.shl(shift as u32).div_rem(&divisor);
            round(neg, &q, exp - shift, !r.is_zero(), f)
        }
    }
}

/// Rundet `(-1)^neg · (q + δ) · 2^e` mit `0 <= δ < 1` und `δ > 0` genau dann,
/// wenn `sticky`, zur naechsten Zahl der Breite `f`, Gleichstand gerade.
fn round(neg: bool, q: &U256, e: i32, sticky: bool, f: Format) -> u64 {
    let n = q.bits() as i32;
    let p = f.p as i32;
    let lead = n - 1 + e;
    if lead > f.emax {
        return f.inf(neg);
    }
    // Normal behaelt `p` Bit; darunter endet die Mantisse bei 2^(emin - p + 1).
    let keep = if lead >= f.emin { p } else { lead - f.emin + p };
    let drop = n - keep;
    let kept = if keep > 0 { q.shr(drop as u32).low() } else { 0 };
    let half = drop >= 1 && drop <= n && q.bit((drop - 1) as u32);
    let below = sticky || (drop >= 2 && q.any_below((drop - 1).min(n) as u32));
    let up = half && (below || kept & 1 == 1);
    let kept = kept + u64::from(up);
    let sign = f.zero(neg);
    if lead >= f.emin {
        // `(lead + bias - 1) · 2^(p-1) + kept`: Ein Uebertrag der Mantisse
        // laeuft in den Exponenten, bis zum Muster von ∞.
        let bias = f.emax;
        sign | ((((lead + bias - 1) as u64) << (p - 1)) + kept)
    } else {
        sign | kept
    }
}

/// Eine vorzeichenlose 256-Bit-Zahl, das niedrigste Wort zuerst.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct U256([u64; 4]);

impl U256 {
    fn from_u128(v: u128) -> Self {
        U256([v as u64, (v >> 64) as u64, 0, 0])
    }

    /// `a · b` exakt; 64 mal 128 Bit passen in 192.
    fn mul(a: u64, b: u128) -> Self {
        let lo = u128::from(a) * u128::from(b as u64);
        let hi = u128::from(a) * u128::from((b >> 64) as u64);
        let mid = (lo >> 64) + (hi & u128::from(u64::MAX));
        U256([lo as u64, mid as u64, ((hi >> 64) + (mid >> 64)) as u64, 0])
    }

    fn is_zero(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    /// Die Stellenzahl; null hat keine.
    fn bits(&self) -> u32 {
        (0..4).rev().find(|i| self.0[*i] != 0).map_or(0, |i| 64 * i as u32 + 64 - self.0[i].leading_zeros())
    }

    fn bit(&self, i: u32) -> bool {
        i < 256 && (self.0[(i / 64) as usize] >> (i % 64)) & 1 == 1
    }

    /// Ist eines der Bits unter `i` gesetzt?
    fn any_below(&self, i: u32) -> bool {
        i > 0 && !self.shl(256u32.saturating_sub(i)).is_zero()
    }

    /// Das unterste Wort.
    fn low(&self) -> u64 {
        self.0[0]
    }

    /// `self · 2^s`, abgeschnitten auf 256 Bit.
    fn shl(&self, s: u32) -> Self {
        let (words, bits) = ((s / 64) as usize, s % 64);
        let mut out = [0u64; 4];
        for (i, w) in out.iter_mut().enumerate().skip(words) {
            let from = i - words;
            *w = self.0[from] << bits;
            if bits > 0 && from > 0 {
                *w |= self.0[from - 1] >> (64 - bits);
            }
        }
        U256(out)
    }

    /// `self / 2^s`, abgerundet.
    fn shr(&self, s: u32) -> Self {
        let (words, bits) = ((s / 64) as usize, s % 64);
        let mut out = [0u64; 4];
        for (i, w) in out.iter_mut().enumerate().take(4usize.saturating_sub(words)) {
            let from = i + words;
            *w = self.0[from] >> bits;
            if bits > 0 && from + 1 < 4 {
                *w |= self.0[from + 1] << (64 - bits);
            }
        }
        U256(out)
    }

    fn ge(&self, o: &Self) -> bool {
        (0..4).rev().find(|i| self.0[*i] != o.0[*i]).is_none_or(|i| self.0[i] > o.0[i])
    }

    fn sub(&self, o: &Self) -> Self {
        let mut out = [0u64; 4];
        let mut borrow = false;
        for (i, w) in out.iter_mut().enumerate() {
            let (d, b1) = self.0[i].overflowing_sub(o.0[i]);
            let (d, b2) = d.overflowing_sub(u64::from(borrow));
            *w = d;
            borrow = b1 || b2;
        }
        U256(out)
    }

    /// Quotient und Rest, Bit fuer Bit; der Rest bleibt unter dem doppelten
    /// Teiler, und der hat hoechstens 128 Bit.
    fn div_rem(&self, d: &Self) -> (Self, Self) {
        let mut q = [0u64; 4];
        let mut r = U256([0; 4]);
        for i in (0..self.bits()).rev() {
            r = r.shl(1);
            if self.bit(i) {
                r.0[0] |= 1;
            }
            if r.ge(d) {
                r = r.sub(d);
                q[(i / 64) as usize] |= 1 << (i % 64);
            }
        }
        (U256(q), r)
    }
}

#[cfg(test)]
mod tests {
    use super::{U256, scale_f32, scale_f64};

    #[test]
    fn the_wide_product_and_division_are_exact() {
        let p = U256::mul(u64::MAX, u128::MAX);
        // (2^64 - 1)(2^128 - 1) = 2^192 - 2^128 - 2^64 + 1
        assert_eq!(p.0, [1, u64::MAX, u64::MAX - 1, 0]);
        let (q, r) = p.div_rem(&U256::from_u128(u128::MAX));
        assert_eq!((q.0, r.is_zero()), ([u64::MAX, 0, 0, 0], true));
    }

    #[test]
    fn an_exact_factor_is_exact() {
        assert_eq!(scale_f64(2.5, 1000, 1), 2500.0);
        assert_eq!(scale_f64(-3.0, 1, 4), -0.75);
        assert_eq!(scale_f32(1.0, 3, 2), 1.5);
        assert_eq!(scale_f64(0.0, 7, 3).to_bits(), 0);
        assert_eq!(scale_f64(-0.0, 7, 3).to_bits(), (-0.0f64).to_bits());
        assert!(scale_f64(1.0, 1, 0).is_nan() && scale_f64(f64::INFINITY, 0, 1).is_nan());
    }
}
