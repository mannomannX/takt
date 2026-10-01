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

use core::cmp::Ordering;

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
/// groesster Exponent der Form `1.f · 2^e`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Format {
    pub(crate) p: u32,
    pub(crate) emin: i32,
    pub(crate) emax: i32,
}

/// binary64.
pub(crate) const F64: Format = Format { p: 53, emin: -1022, emax: 1023 };
/// binary32.
pub(crate) const F32: Format = Format { p: 24, emin: -126, emax: 127 };

impl Format {
    /// Breite des Bitmusters.
    fn bits(self) -> u32 {
        if self.p == 53 { 64 } else { 32 }
    }

    fn sign(self, neg: bool) -> u64 {
        u64::from(neg) << (self.bits() - 1)
    }

    /// Das Bitmuster von ±0.
    pub(crate) fn zero(self, neg: bool) -> u64 {
        self.sign(neg)
    }

    /// Das Bitmuster von ±∞.
    pub(crate) fn inf(self, neg: bool) -> u64 {
        self.sign(neg) | (((1u64 << (self.bits() - self.p)) - 1) << (self.p - 1))
    }

    /// Ein ruhiges NaN; ueberall dasselbe, damit auch es bitgleich ist.
    pub(crate) fn nan(self) -> u64 {
        self.inf(false) | (1u64 << (self.p - 2))
    }
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
    let frac_bits = f.p - 1;
    let exp_bits = f.bits() - f.p;
    let neg = (bits >> (f.bits() - 1)) & 1 == 1;
    let biased = ((bits >> frac_bits) & ((1 << exp_bits) - 1)) as i32;
    let frac = bits & ((1u64 << frac_bits) - 1);
    let bias = (1 << (exp_bits - 1)) - 1;
    if biased == (1 << exp_bits) - 1 {
        return if frac == 0 { Class::Inf { neg } } else { Class::Nan };
    }
    let (mant, exp) = if biased == 0 {
        if frac == 0 {
            return Class::Zero { neg };
        }
        (frac, 1 - bias - frac_bits as i32)
    } else {
        (frac | (1 << frac_bits), biased - bias - frac_bits as i32)
    };
    let tz = mant.trailing_zeros();
    Class::Finite { neg, mant: mant >> tz, exp: exp + tz as i32 }
}

impl<const N: usize> Big<N> {
    const LIMBS: () = assert!(N >= 2 && N <= MAX);

    pub(crate) const ZERO: Self = Big { neg: false, exp: 0, m: [0; N] };

    pub(crate) const ONE: Self = {
        let mut m = [0; N];
        m[N - 1] = 1 << 63;
        Big { neg: false, exp: 1, m }
    };

    pub(crate) fn is_zero(&self) -> bool {
        self.m[N - 1] == 0
    }

    /// `(-1)^neg · v · 2^e`, exakt.
    pub(crate) fn from_parts(neg: bool, v: u128, e: i32) -> Self {
        let () = Self::LIMBS;
        if v == 0 {
            return Self::ZERO;
        }
        let lz = v.leading_zeros();
        let top = v << lz;
        let mut m = [0; N];
        m[N - 1] = (top >> 64) as u64;
        m[N - 2] = top as u64;
        Big { neg, exp: e + 128 - lz as i32, m }
    }

    /// Eine ganze Zahl, exakt.
    pub(crate) fn from_i64(k: i64) -> Self {
        Self::from_parts(k < 0, u128::from(k.unsigned_abs()), 0)
    }

    /// Eine Konstante, auf `N` Woerter abgeschnitten.
    pub(crate) fn from_const(c: &Const) -> Self {
        let mut m = [0; N];
        m.copy_from_slice(&c.m[MAX - N..]);
        Big { neg: c.neg, exp: c.exp, m }
    }

    /// `Σ w[i] · 2^(64 i + scale)` fuer eine Ganzzahl beliebiger Wortzahl,
    /// auf `N` Woerter abgeschnitten.
    pub(crate) fn from_words(neg: bool, w: &[u64], scale: i32) -> Self {
        let Some(top) = w.iter().rposition(|x| *x != 0) else { return Self::ZERO };
        let len = 64 * (top as i32 + 1) - w[top].leading_zeros() as i32;
        // Die obersten 64·N Bits, linksbuendig.
        let mut m = [0; N];
        for (i, slot) in m.iter_mut().enumerate() {
            let bit = len - 64 * (N - i) as i32;
            *slot = word_at(w, bit);
        }
        Big { neg, exp: len + scale, m }
    }

    pub(crate) fn neg(&self) -> Self {
        Big { neg: !self.neg, ..*self }
    }

    pub(crate) fn abs(&self) -> Self {
        Big { neg: false, ..*self }
    }

    /// Mal `2^k`, exakt.
    pub(crate) fn mul_pow2(&self, k: i32) -> Self {
        if self.is_zero() { *self } else { Big { exp: self.exp + k, ..*self } }
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
        let mut p = [0u64; 2 * MAX];
        for i in 0..N {
            let mut carry = 0u64;
            for j in 0..N {
                let t = u128::from(self.m[i]) * u128::from(o.m[j]) + u128::from(p[i + j]) + u128::from(carry);
                p[i + j] = t as u64;
                carry = (t >> 64) as u64;
            }
            p[i + N] = carry;
        }
        let mut m = [0; N];
        let mut exp = self.exp + o.exp;
        if p[2 * N - 1] >> 63 == 1 {
            m.copy_from_slice(&p[N..2 * N]);
        } else {
            for i in 0..N {
                m[i] = (p[N + i] << 1) | (p[N + i - 1] >> 63);
            }
            exp -= 1;
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
        let mut x = [0u64; MAX + 1];
        let mut y = [0u64; MAX + 1];
        x[1..=N].copy_from_slice(&a.m);
        // b um die Exponentdifferenz nach rechts, Bits unter dem Schutzwort
        // fallen weg.
        let d = (a.exp - b.exp) as u32;
        let (words, bits) = ((d / 64) as usize, d % 64);
        for (i, slot) in y[..=N].iter_mut().enumerate() {
            // Quelle: das Wort von (b.m << 64) bei Index i + words.
            let src = |k: usize| if (1..=N).contains(&k) { b.m[k - 1] } else { 0 };
            let lo = i + words;
            *slot = if bits == 0 { src(lo) } else { (src(lo) >> bits) | (src(lo + 1) << (64 - bits)) };
        }
        let mut exp = a.exp;
        if a.neg == b.neg {
            let mut carry = false;
            for i in 0..=N {
                let (s, c1) = x[i].overflowing_add(y[i]);
                let (s, c2) = s.overflowing_add(u64::from(carry));
                x[i] = s;
                carry = c1 || c2;
            }
            if carry {
                for i in 0..N {
                    x[i] = (x[i] >> 1) | (x[i + 1] << 63);
                }
                x[N] = (x[N] >> 1) | (1 << 63);
                exp += 1;
            }
        } else {
            let mut borrow = false;
            for i in 0..=N {
                let (s, b1) = x[i].overflowing_sub(y[i]);
                let (s, b2) = s.overflowing_sub(u64::from(borrow));
                x[i] = s;
                borrow = b1 || b2;
            }
            let Some(top) = x[..=N].iter().rposition(|w| *w != 0) else { return Self::ZERO };
            let shift = 64 * (N - top) as u32 + x[top].leading_zeros();
            shift_left(&mut x[..=N], shift);
            exp -= shift as i32;
        }
        let mut m = [0; N];
        m.copy_from_slice(&x[1..=N]);
        Big { neg: a.neg, exp, m }
    }

    pub(crate) fn sub(&self, o: &Self) -> Self {
        self.add(&o.neg())
    }

    /// Die naechste ganze Zahl (bei Gleichstand vom Nullpunkt weg); nur
    /// fuer Betraege unter 2^62.
    pub(crate) fn round_i64(&self) -> i64 {
        if self.is_zero() || self.exp < 0 {
            return 0;
        }
        let top = self.m[N - 1];
        let k = if self.exp == 0 {
            (top >> 63) as i64
        } else {
            let e = self.exp.min(62) as u32;
            ((top >> (64 - e)) + ((top >> (63 - e)) & 1)) as i64
        };
        if self.neg { -k } else { k }
    }

    /// Kehrwert nach Newton. Der Startwert aus dem obersten Wort traegt 62
    /// Bit; jeder Schritt verdoppelt sie, bis die Mantisse voll ist.
    pub(crate) fn recip(&self) -> Self {
        if self.is_zero() {
            return Self::ZERO;
        }
        let t = u128::from(self.m[N - 1]);
        let mut y = Self::from_parts(self.neg, (1u128 << 127) / t, -63 - self.exp);
        for _ in 0..newton_steps(N) {
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
        // self = f · 2^(2h), f in [1/4, 1); der Startwert der Wurzel aus
        // den obersten 128 Bit von f.
        let odd = self.exp.rem_euclid(2) == 1;
        let h = (self.exp + i32::from(odd)) / 2;
        let f = Big { exp: self.exp - 2 * h, ..*self };
        let top = (u128::from(self.m[N - 1]) << 64) | u128::from(self.m[N - 2]);
        let root = if odd { (top >> 1).isqrt() } else { top.isqrt() };
        // root ≈ sqrt(f) · 2^64, also 1/sqrt(f) ≈ 2^127 / root · 2^-63.
        let mut y = Self::from_parts(false, (1u128 << 127) / root.max(1), -63);
        for _ in 0..newton_steps(N) {
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
        let e = self.exp - 1;
        if e > f.emax {
            return f.inf(self.neg);
        }
        let p = f.p as i32;
        let kept = p - (f.emin - e).max(0);
        if kept < 0 {
            return f.zero(self.neg);
        }
        let top = self.m[N - 1];
        let rest_low = self.m[..N - 1].iter().any(|w| *w != 0);
        let (mut n, half, sticky) = if kept == 0 {
            (0, true, top << 1 != 0 || rest_low)
        } else {
            let k = kept as u32;
            let below = 63 - k;
            (top >> (64 - k), (top >> below) & 1 == 1, top & ((1u64 << below) - 1) != 0 || rest_low)
        };
        if half && (sticky || n & 1 == 1) {
            n += 1;
        }
        let sign = f.sign(self.neg);
        if kept < p {
            // Subnormal: Das Muster ist die Mantisse; ein Uebertrag in die
            // kleinste Normalzahl setzt von selbst das Exponentenbit.
            return sign | n;
        }
        let mut e = e;
        if n == 1 << p {
            n >>= 1;
            e += 1;
            if e > f.emax {
                return f.inf(self.neg);
            }
        }
        let bias = f.emax;
        sign | (((e + bias) as u64) << (p - 1)) | (n & ((1 << (p - 1)) - 1))
    }
}

/// Newton-Schritte fuer Kehrwert und Wurzel: von 62 Bit auf die volle
/// Mantisse mit Reserve.
fn newton_steps(n: usize) -> usize {
    n / 2 + 1
}

/// Das 64-Bit-Wort einer Ganzzahl `w`, das bei Bit `bit` beginnt; Bits
/// ausserhalb sind null.
fn word_at(w: &[u64], bit: i32) -> u64 {
    let get = |i: i32| if i >= 0 { w.get(i as usize).copied().unwrap_or(0) } else { 0 };
    let (q, r) = (bit.div_euclid(64), bit.rem_euclid(64) as u32);
    if r == 0 { get(q) } else { (get(q) >> r) | (get(q + 1) << (64 - r)) }
}

/// Schiebt eine Ganzzahl um `s` Bit nach links; was oben hinausfaellt, ist null.
fn shift_left(x: &mut [u64], s: u32) {
    let (words, bits) = ((s / 64) as usize, s % 64);
    for i in (0..x.len()).rev() {
        let hi = if i >= words { x[i - words] } else { 0 };
        let lo = if i > words { x[i - words - 1] } else { 0 };
        x[i] = if bits == 0 { hi } else { (hi << bits) | (lo >> (64 - bits)) };
    }
}
