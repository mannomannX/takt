//! Gleitkomma mit `N` Woertern Mantisse, nur aus Ganzzahlarithmetik.
//!
//! Die korrekt gerundeten Funktionen rechnen ihr Zwischenergebnis hier und
//! runden es erst am Ende in die Zielbreite. Kein Schritt benutzt die FPU:
//! Ganzzahlarithmetik ist auf jedem Ziel dieselbe, also ist es auch jedes
//! Ergebnis (Satz 9.4.4) — unabhaengig davon, ob ein Kern `f64` in Hardware
//! rechnet, in Software oder gar nicht.
//!
//! Jede Operation schneidet ab: Ihr Ergebnis liegt dem Betrag nach hoechstens
//! eine Einheit der letzten Stelle unter dem exakten Wert. Mit `N = 2` sind
//! das 2^-127 relativ, mit `N = 4` 2^-255. Die Fehlerschranken der
//! Funktionen (`elem.rs`) zaehlen diese Einheiten.
//!
//! Exponenten und Bitzahlen rechnen saettigend: Sie bleiben weit unter den
//! Grenzen von `i32`, und ein gesaettigter Exponent laege jenseits jeder
//! Breite, rundete also wie der exakte zu ∞ oder null. Die Woerter der
//! Mantisse rechnen mit Uebertrag ueber `mul_add`.

use core::cmp::Ordering;
use core::num::NonZeroU128;

/// Die groesste Wortzahl; `f64` rechnet mit vier.
pub(crate) const MAX: usize = 4;

/// Eine Konstante aus `table.rs`: 256 Bit Mantisse, auf `N` Woerter
/// abgeschnitten, wenn sie gelesen wird.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Const {
    pub(crate) neg: bool,
    pub(crate) exp: i32,
    pub(crate) m: [u64; MAX],
}

/// `(-1)^neg · 0.m · 2^exp`, die Mantisse normiert (oberstes Bit gesetzt)
/// oder null; das niedrigste Wort zuerst.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Big<const N: usize> {
    pub(crate) neg: bool,
    pub(crate) exp: i32,
    pub(crate) m: [u64; N],
}

/// Eine Zielbreite: Mantissenbits mit der fuehrenden Eins, kleinster und
/// groesster Exponent der Form `1.f · 2^e` (`emax` ist zugleich der Bias),
/// dazu die Lage der Felder im Bitmuster.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Format {
    pub(crate) p: u32,
    pub(crate) emin: i32,
    pub(crate) emax: i32,
    /// Die Bits des Bruchteils, `p - 1`; darueber beginnt der Exponent.
    pub(crate) frac_bits: u32,
    /// Das Vorzeichenbit.
    sign: u64,
    /// Die Bits des Exponenten; allein gesetzt sind sie +∞.
    inf: u64,
}

/// binary64.
pub(crate) const F64: Format =
    Format { p: 53, emin: -1022, emax: 1023, frac_bits: 52, sign: 1 << 63, inf: 0x7FF0_0000_0000_0000 };
/// binary32.
pub(crate) const F32: Format = Format { p: 24, emin: -126, emax: 127, frac_bits: 23, sign: 1 << 31, inf: 0x7F80_0000 };

impl Format {
    /// Das Bitmuster von ±0.
    pub(crate) fn zero(self, neg: bool) -> u64 {
        if neg { self.sign } else { 0 }
    }

    /// Das Bitmuster von ±∞.
    pub(crate) fn inf(self, neg: bool) -> u64 {
        self.zero(neg) | self.inf
    }

    /// Ein ruhiges NaN, das oberste Bit des Bruchteils gesetzt; ueberall
    /// dasselbe, damit auch es bitgleich ist.
    pub(crate) fn nan(self) -> u64 {
        self.inf | (1u64 << self.frac_bits >> 1)
    }

    /// Die Bits des Bruchteils.
    pub(crate) fn frac(self) -> u64 {
        !(u64::MAX << self.frac_bits)
    }
}

/// Das `f32` zu einem Bitmuster der Breite `F32`: die unteren 32 Bit.
pub(crate) fn to_f32(bits: u64) -> f32 {
    let [b0, b1, b2, b3, ..] = bits.to_le_bytes();
    f32::from_bits(u32::from_le_bytes([b0, b1, b2, b3]))
}

/// Ein zerlegtes Argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Nan,
    Inf {
        neg: bool,
    },
    Zero {
        neg: bool,
    },
    /// `(-1)^neg · mant · 2^exp`, exakt; `mant` ungerade.
    Finite {
        neg: bool,
        mant: u64,
        exp: i32,
    },
}

/// Zerlegt ein Bitmuster der Breite `f`.
pub(crate) fn classify(bits: u64, f: Format) -> Class {
    let neg = bits & f.sign != 0;
    let frac = bits & f.frac();
    if bits & f.inf == f.inf {
        return if frac == 0 { Class::Inf { neg } } else { Class::Nan };
    }
    // Das Feld des Exponenten; null heisst subnormal: ohne fuehrende Eins,
    // mit dem Exponenten der kleinsten Normalzahl.
    let field = (bits & f.inf) >> f.frac_bits;
    let mant = if field == 0 { frac } else { frac | (1 << f.frac_bits) };
    if mant == 0 {
        return Class::Zero { neg };
    }
    let Ok(field) = i32::try_from(field.max(1)) else { return Class::Nan };
    let exp = field.saturating_sub(f.emax).saturating_sub_unsigned(f.frac_bits);
    let tz = mant.trailing_zeros();
    Class::Finite { neg, mant: mant >> tz, exp: exp.saturating_add_unsigned(tz) }
}

impl<const N: usize> Big<N> {
    const LIMBS: () = assert!(N >= 2 && N <= MAX);

    /// Die Mantisse mit einem Schutzwort darunter (`add`).
    const GUARDED: usize = N + 1;

    /// Die Woerter des vollen Produkts zweier Mantissen (`mul`).
    const WIDE: usize = 2 * N;

    /// Newton-Schritte fuer Kehrwert und Wurzel: von 62 Bit auf die volle
    /// Mantisse mit Reserve.
    const NEWTON: usize = N / 2 + 1;

    pub(crate) const ZERO: Self = Big { neg: false, exp: 0, m: [0; N] };

    pub(crate) const ONE: Self = {
        let mut m = [0; N];
        if let Some(top) = m.last_mut() {
            *top = 1 << 63;
        }
        Big { neg: false, exp: 1, m }
    };

    /// Das oberste Wort der Mantisse; null fuer null.
    pub(crate) fn top(&self) -> u64 {
        self.m.last().copied().unwrap_or(0)
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.top() == 0
    }

    /// `(-1)^neg · v · 2^e`, exakt.
    pub(crate) fn from_parts(neg: bool, v: u128, e: i32) -> Self {
        let () = Self::LIMBS;
        if v == 0 {
            return Self::ZERO;
        }
        let lz = v.leading_zeros();
        let mut m = [0; N];
        if let Some(top) = m.last_chunk_mut::<2>() {
            *top = split(v << lz);
        }
        Big { neg, exp: e.saturating_add(128).saturating_sub_unsigned(lz), m }
    }

    /// Eine ganze Zahl, exakt.
    pub(crate) fn from_i64(k: i64) -> Self {
        Self::from_parts(k < 0, u128::from(k.unsigned_abs()), 0)
    }

    /// Eine Konstante, auf `N` Woerter abgeschnitten.
    pub(crate) fn from_const(c: &Const) -> Self {
        Big { neg: c.neg, exp: c.exp, m: c.m.last_chunk().copied().unwrap_or([0; N]) }
    }

    /// `Σ w[i] · 2^(64 i + scale)` fuer eine Ganzzahl beliebiger Wortzahl,
    /// auf `N` Woerter abgeschnitten.
    pub(crate) fn from_words(neg: bool, w: &[u64], scale: i32) -> Self {
        let len = bit_len(w);
        if len == 0 {
            return Self::ZERO;
        }
        // Die obersten 64·N Bits, linksbuendig: das oberste Wort ab Bit
        // `len - 64`, jedes weitere 64 Bit tiefer.
        let mut m = [0; N];
        let mut bit = 0i32.saturating_add_unsigned(len);
        for slot in m.iter_mut().rev() {
            bit = bit.saturating_sub(64);
            *slot = word_at(w, bit);
        }
        Big { neg, exp: scale.saturating_add_unsigned(len), m }
    }

    pub(crate) fn neg(&self) -> Self {
        Big { neg: !self.neg, ..*self }
    }

    pub(crate) fn abs(&self) -> Self {
        Big { neg: false, ..*self }
    }

    /// Mal `2^k`, exakt.
    pub(crate) fn mul_pow2(&self, k: i32) -> Self {
        if self.is_zero() { *self } else { Big { exp: self.exp.saturating_add(k), ..*self } }
    }

    /// Vergleich der Betraege.
    pub(crate) fn cmp_abs(&self, o: &Self) -> Ordering {
        match (self.is_zero(), o.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (false, false) => {}
        }
        self.exp.cmp(&o.exp).then_with(|| self.m.iter().rev().cmp(o.m.iter().rev()))
    }

    /// Produkt, abgeschnitten.
    pub(crate) fn mul(&self, o: &Self) -> Self {
        if self.is_zero() || o.is_zero() {
            return Self::ZERO;
        }
        // Das volle Produkt nach der Schulmethode: Zeile i beginnt bei Wort i,
        // ihr letzter Uebertrag ist das Wort darueber.
        let mut p = [0u64; 2 * MAX];
        for (i, a) in self.m.iter().enumerate() {
            let mut row = p.iter_mut().skip(i);
            let mut carry = 0;
            for (b, slot) in o.m.iter().zip(row.by_ref()) {
                [*slot, carry] = mul_add(*a, *b, *slot, carry);
            }
            if let Some(slot) = row.next() {
                *slot = carry;
            }
        }
        // Die oberen N Woerter. Liegt die fuehrende Eins ein Bit tiefer,
        // ruecken sie um eins nach oben, und das oberste Bit des Worts
        // darunter kommt nach.
        let Some((low, high)) = p.get(..Self::WIDE).and_then(<[u64]>::split_last_chunk::<N>) else {
            return Self::ZERO;
        };
        let mut m = *high;
        let mut exp = self.exp.saturating_add(o.exp);
        if m.last().is_some_and(|w| w >> 63 == 0) {
            let mut below = low.last().copied().unwrap_or(0);
            for w in m.iter_mut() {
                let word = *w;
                *w = (word << 1) | (below >> 63);
                below = word;
            }
            exp = exp.saturating_sub(1);
        }
        Big { neg: self.neg != o.neg, exp, m }
    }

    pub(crate) fn square(&self) -> Self {
        self.mul(self)
    }

    /// Summe, abgeschnitten. Ein Schutzwort unter der Mantisse haelt den
    /// Fehler einer Ausloeschung bei einer Einheit des groesseren Summanden
    /// mal 2^-64.
    pub(crate) fn add(&self, o: &Self) -> Self {
        if o.is_zero() {
            return *self;
        }
        if self.is_zero() {
            return *o;
        }
        let (a, b) = if self.cmp_abs(o) == Ordering::Less { (o, self) } else { (self, o) };
        // x ist a mit dem Schutzwort, y ist b um die Exponentdifferenz nach
        // rechts; Bits unter dem Schutzwort fallen weg.
        let mut x = [0u64; MAX + 1];
        for (slot, w) in x.iter_mut().skip(1).zip(&a.m) {
            *slot = *w;
        }
        let mut y = [0u64; MAX + 1];
        let d = a.exp.abs_diff(b.exp);
        let (words, bits) = (usize::try_from(d / 64).unwrap_or(usize::MAX), d % 64);
        // Wort k von b.m · 2^64: unter und ueber der Mantisse null.
        let src = |k: usize| k.checked_sub(1).and_then(|j| b.m.get(j)).copied().unwrap_or(0);
        for (i, slot) in y.iter_mut().take(Self::GUARDED).enumerate() {
            let lo = i.saturating_add(words);
            *slot = funnel(src(lo), src(lo.saturating_add(1)), bits);
        }
        let mut m = [0; N];
        if a.neg == b.neg {
            let mut carry = false;
            for (xi, yi) in x.iter_mut().zip(&y).take(Self::GUARDED) {
                let (s, c1) = xi.overflowing_add(*yi);
                let (s, c2) = s.overflowing_add(u64::from(carry));
                *xi = s;
                carry = c1 || c2;
            }
            let mut exp = a.exp;
            if carry {
                // Eine Stelle nach rechts, der Uebertrag wird das oberste Bit.
                let mut above = 1;
                for w in x.iter_mut().take(Self::GUARDED).rev() {
                    let low = *w & 1;
                    *w = (*w >> 1) | (above << 63);
                    above = low;
                }
                exp = exp.saturating_add(1);
            }
            for (slot, w) in m.iter_mut().zip(x.iter().skip(1)) {
                *slot = *w;
            }
            return Big { neg: a.neg, exp, m };
        }
        let mut borrow = false;
        for (xi, yi) in x.iter_mut().zip(&y).take(Self::GUARDED) {
            let (s, b1) = xi.overflowing_sub(*yi);
            let (s, b2) = s.overflowing_sub(u64::from(borrow));
            *xi = s;
            borrow = b1 || b2;
        }
        if x.iter().all(|w| *w == 0) {
            return Self::ZERO;
        }
        // Normieren: um die fuehrenden Nullen der N + 1 Woerter nach links,
        // die oberen N Woerter bleiben.
        let lz = leading_zeros(x.iter().take(Self::GUARDED));
        let mut bit = 0i32.saturating_sub_unsigned(lz);
        for slot in m.iter_mut() {
            bit = bit.saturating_add(64);
            *slot = word_at(&x, bit);
        }
        Big { neg: a.neg, exp: a.exp.saturating_sub_unsigned(lz), m }
    }

    pub(crate) fn sub(&self, o: &Self) -> Self {
        self.add(&o.neg())
    }

    /// Die naechste ganze Zahl, bei Gleichstand vom Nullpunkt weg; ab 2^31
    /// gesaettigt.
    pub(crate) fn round_i32(&self) -> i32 {
        let Ok(e) = u32::try_from(self.exp) else { return 0 };
        // Die e ganzen Stellen und das Bit danach.
        let head = self.top() >> 63u32.saturating_sub(e.min(62));
        let k = i32::try_from((head >> 1).saturating_add(head & 1)).unwrap_or(i32::MAX);
        if self.neg { k.saturating_neg() } else { k }
    }

    /// Kehrwert nach Newton. Der Startwert aus dem obersten Wort traegt 62
    /// Bit; jeder Schritt verdoppelt sie, bis die Mantisse voll ist.
    pub(crate) fn recip(&self) -> Self {
        let Some(t) = NonZeroU128::new(u128::from(self.top())) else { return Self::ZERO };
        let mut y = Self::from_parts(self.neg, (1u128 << 127) / t, (-63i32).saturating_sub(self.exp));
        for _ in 0..Self::NEWTON {
            let e = Self::ONE.sub(&self.mul(&y));
            y = y.add(&y.mul(&e));
        }
        y
    }

    pub(crate) fn div(&self, o: &Self) -> Self {
        self.mul(&o.recip())
    }

    /// Quadratwurzel eines nicht negativen Werts ueber den Kehrwert der
    /// Wurzel nach Newton.
    pub(crate) fn sqrt(&self) -> Self {
        if self.is_zero() || self.neg {
            return Self::ZERO;
        }
        // self = f · 2^(2h), f in [1/4, 1): Der Exponent von f ist 0 oder -1.
        // Der Startwert der Wurzel aus den obersten 128 Bit von f.
        let odd = self.exp.rem_euclid(2) == 1;
        let h = self.exp.saturating_add(i32::from(odd)).div_euclid(2);
        let f = Big { exp: if odd { -1 } else { 0 }, ..*self };
        let [lo, hi] = self.m.last_chunk::<2>().copied().unwrap_or([0; 2]);
        let top = (u128::from(hi) << 64) | u128::from(lo);
        let root = if odd { (top >> 1).isqrt() } else { top.isqrt() };
        // root ≈ sqrt(f) · 2^64, also 1/sqrt(f) ≈ 2^127 / root · 2^-63.
        let mut y = Self::from_parts(false, (1u128 << 127) / NonZeroU128::new(root).unwrap_or(NonZeroU128::MIN), -63);
        for _ in 0..Self::NEWTON {
            let e = Self::ONE.sub(&f.mul(&y.square()));
            y = y.add(&y.mul(&e).mul_pow2(-1));
        }
        f.mul(&y).mul_pow2(h)
    }

    /// Rundet zur naechsten Zahl der Breite, Gleichstand zur geraden, mit
    /// Subnormalen und Ueberlauf. Das Bitmuster.
    pub(crate) fn round(&self, f: Format) -> u64 {
        if self.is_zero() {
            return f.zero(self.neg);
        }
        let e = self.exp.saturating_sub(1);
        if e > f.emax {
            return f.inf(self.neg);
        }
        // Die Stellen, die bleiben: `p`, unter `emin` um die Differenz weniger.
        let lost = u32::try_from(f.emin.saturating_sub(e)).unwrap_or(0);
        let Some(kept) = f.p.checked_sub(lost) else { return f.zero(self.neg) };
        let top = self.top();
        let rest_low = self.m.iter().rev().skip(1).any(|w| *w != 0);
        let (mut n, half, sticky) = if kept == 0 {
            (0, true, top << 1 != 0 || rest_low)
        } else {
            // Die oberen `kept` Bits, das Bit danach und ob darunter eines
            // gesetzt ist.
            let below = 63u32.saturating_sub(kept);
            let head = top >> below;
            (head >> 1, head & 1 == 1, top & !(u64::MAX << below) != 0 || rest_low)
        };
        if half && (sticky || n & 1 == 1) {
            n = n.saturating_add(1);
        }
        let sign = f.zero(self.neg);
        if kept < f.p {
            // Subnormal: Das Muster ist die Mantisse; ein Uebertrag in die
            // kleinste Normalzahl setzt von selbst das Exponentenbit.
            return sign | n;
        }
        let mut e = e;
        if n == 1 << f.p {
            n >>= 1;
            e = e.saturating_add(1);
            if e > f.emax {
                return f.inf(self.neg);
            }
        }
        // Das Feld des Exponenten ab dem Bias, ohne die fuehrende Eins.
        let field = u64::try_from(e.saturating_add(f.emax)).unwrap_or(0);
        sign | (field << f.frac_bits) | (n & f.frac())
    }
}

/// `a · b + c + d` als zwei Woerter, das niedrige zuerst. Es passt immer:
/// (2^64 - 1)^2 + 2 (2^64 - 1) = 2^128 - 1.
pub(crate) fn mul_add(a: u64, b: u64, c: u64, d: u64) -> [u64; 2] {
    split(u128::from(a).wrapping_mul(u128::from(b)).wrapping_add(u128::from(c)).wrapping_add(u128::from(d)))
}

/// Die beiden Woerter einer 128-Bit-Zahl, das niedrige zuerst.
pub(crate) fn split(v: u128) -> [u64; 2] {
    let [l0, l1, l2, l3, l4, l5, l6, l7, h0, h1, h2, h3, h4, h5, h6, h7] = v.to_le_bytes();
    [u64::from_le_bytes([l0, l1, l2, l3, l4, l5, l6, l7]), u64::from_le_bytes([h0, h1, h2, h3, h4, h5, h6, h7])]
}

/// Die 64 Bit von `hi:lo` ab Bit `bits` (0 bis 63).
pub(crate) fn funnel(lo: u64, hi: u64, bits: u32) -> u64 {
    if bits == 0 { lo } else { (lo >> bits) | (hi << 64u32.saturating_sub(bits)) }
}

/// Die Stellenzahl einer Ganzzahl aus Woertern, das niedrigste zuerst; null
/// hat keine.
pub(crate) fn bit_len(w: &[u64]) -> u32 {
    let (mut len, mut end) = (0u32, 0u32);
    for word in w {
        end = end.saturating_add(64);
        if *word != 0 {
            len = end.saturating_sub(word.leading_zeros());
        }
    }
    len
}

/// Die fuehrenden Nullen einer Ganzzahl aus Woertern, das niedrigste zuerst.
fn leading_zeros<'a>(words: impl DoubleEndedIterator<Item = &'a u64>) -> u32 {
    let mut n = 0u32;
    for w in words.rev() {
        n = n.saturating_add(w.leading_zeros());
        if *w != 0 {
            break;
        }
    }
    n
}

/// Das 64-Bit-Wort einer Ganzzahl `w`, das bei Bit `bit` beginnt; Bits
/// ausserhalb sind null.
pub(crate) fn word_at(w: &[u64], bit: i32) -> u64 {
    let get = |i: i32| usize::try_from(i).ok().and_then(|k| w.get(k)).copied().unwrap_or(0);
    let q = bit.div_euclid(64);
    funnel(get(q), get(q.saturating_add(1)), bit.rem_euclid(64).unsigned_abs())
}

#[cfg(test)]
mod tests {
    use super::{F32, F64, Format};

    /// Die Felder der Breiten sind die von IEEE 754.
    #[test]
    fn the_formats_carry_the_ieee_patterns() {
        assert_eq!(
            (F64.zero(true), F64.inf(false), F64.inf(true), F64.nan(), F64.frac()),
            (0x8000_0000_0000_0000, 0x7FF0_0000_0000_0000, 0xFFF0_0000_0000_0000, 0x7FF8_0000_0000_0000, (1 << 52) - 1)
        );
        assert_eq!(
            (F32.zero(true), F32.inf(false), F32.inf(true), F32.nan(), F32.frac()),
            (0x8000_0000, 0x7F80_0000, 0xFF80_0000, 0x7FC0_0000, (1 << 23) - 1)
        );
        for (f, bits) in [(F64, 64), (F32, 32)] {
            let Format { p, emin, emax, frac_bits, .. } = f;
            assert_eq!((frac_bits, emin, bits - p), (p - 1, 1 - emax, f.inf.count_ones()), "{f:?}");
        }
    }
}
