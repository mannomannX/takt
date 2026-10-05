//! Die korrekt gerundeten transzendenten Funktionen (4.2).
//!
//! **Eine Implementierung fuer beide Breiten.** Jede Funktion rechnet in
//! [`Big`] mit `N` Woertern Mantisse — `f32` mit zwei, `f64` mit vier — und
//! rundet erst am Ende. Der erschoepfende Test ueber alle `f32` prueft damit
//! denselben Code, der `f64` rechnet, nur mit kuerzerer Mantisse.
//!
//! **Korrekt gerundet, ohne Liste harter Faelle.** Der relative Fehler des
//! Zwischenergebnisses liegt fuer `f32` unter 2^-110 und fuer `f64` unter
//! 2^-240. Die bekannten schwierigsten Argumente brauchen fuer `f32` etwa
//! 2^-70 und fuer `f64` etwa 2^-125; das gerundete Ergebnis ist darum das
//! korrekt gerundete. Fuer `f32` belegt das der erschoepfende Test
//! (`tests/exhaustive.rs`), fuer `f64` die Schranke zusammen mit den
//! Suchen nach den schwierigsten Faellen aus der Literatur. Exakte
//! Ergebnisse — die einzigen, die genau auf einem Mittelpunkt zwischen zwei
//! Gleitkommazahlen liegen koennen — hat nur `pow`, und es rechnet sie exakt.
//!
//! **Eine Phase.** Es gibt keinen schnellen Pfad mit Rundungstest und
//! langsamen Rueckfall: Der Kostenvertrag ist ohnehin der langsame Pfad
//! (9.4.3), und so ist die Laufzeit fast unabhaengig vom Argument.
//!
//! Die Reihenlaengen stehen in [`Terms`]; jede ist so gewaehlt, dass das erste
//! weggelassene Glied hoechstens 2^-(64 N + 1) des Ergebnisses ausmacht — so
//! knapp nur bei `exp` und `atan` mit zwei Woertern und bei `log` mit vier,
//! sonst weit darunter.

use core::cmp::Ordering;

use crate::big::{Big, Class, Const, Format, MAX, classify};
use crate::table::{ATAN64, COS64, INV_FACT, INV_LN2, INV_ODD, LN2, LOG128, PI, SIN64, TWO_OVER_PI};

/// Reihenlaengen je Wortzahl.
struct Terms {
    /// Glieder von exp(s) fuer |s| <= 2^-9.5: s^n / n! bis n = exp.
    exp: usize,
    /// Koeffizienten von sin(s) / s und cos(s) in s^2 fuer |s| <= 2^-7.
    sin: usize,
    cos: usize,
    /// Koeffizienten von atan(u) / u in u^2 fuer |u| <= 2^-7.
    atan: usize,
    /// Koeffizienten von atanh(t) / t in t^2 fuer |t| <= 2^-8.4.
    log: usize,
}

const fn terms(n: usize) -> Terms {
    if n == 2 {
        Terms { exp: 10, sin: 7, cos: 8, atan: 9, log: 8 }
    } else {
        Terms { exp: 20, sin: 13, cos: 14, atan: 19, log: 15 }
    }
}

fn konst<const N: usize>(c: &Const) -> Big<N> {
    Big::from_const(c)
}

/// Eine Konstante mit wechselndem Vorzeichen fuer alternierende Reihen.
fn signed<const N: usize>(c: &Const, neg: bool) -> Big<N> {
    let b = Big::from_const(c);
    if neg { b.neg() } else { b }
}

/// Σ c(i) · w^i fuer i < len, nach Horner.
fn horner<const N: usize>(w: &Big<N>, len: usize, c: impl Fn(usize) -> Big<N>) -> Big<N> {
    let mut p = c(len - 1);
    for i in (0..len - 1).rev() {
        p = p.mul(w).add(&c(i));
    }
    p
}

fn pi<const N: usize>() -> Big<N> {
    konst(&PI)
}

// ------------------------------------------------------------------- exp

/// exp(x) fuer |x| < 2^11.
///
/// x = k ln 2 + r mit |r| <= ln 2 / 2; exp(r) als Reihe in s = r / 256 und
/// achtmal quadriert. Der Fehler von k ln 2 ist absolut 2^(11 - 64 N), das
/// Quadrieren verstaerkt den der Reihe um 2^8.
fn exp_big<const N: usize>(x: &Big<N>) -> Big<N> {
    let k = x.mul(&konst(&INV_LN2)).round_i64();
    let r = x.sub(&konst::<N>(&LN2).mul(&Big::from_i64(k)));
    let s = r.mul_pow2(-8);
    let mut p = horner(&s, terms(N).exp + 1, |i| konst(&INV_FACT[i]));
    for _ in 0..8 {
        p = p.square();
    }
    p.mul_pow2(k as i32)
}

pub(crate) fn exp<const N: usize>(bits: u64, f: Format) -> u64 {
    match classify(bits, f) {
        Class::Nan => f.nan(),
        Class::Inf { neg: true } => f.zero(false),
        Class::Inf { neg: false } => f.inf(false),
        Class::Zero { .. } => Big::<N>::ONE.round(f),
        Class::Finite { neg, mant, exp } => {
            let x = Big::<N>::from_parts(neg, u128::from(mant), exp);
            // Ab |x| = 2^11 liegt das Ergebnis in beiden Breiten jenseits des
            // groessten Werts oder unter dem halben kleinsten.
            if x.exp > 11 {
                return if neg { f.zero(false) } else { f.inf(false) };
            }
            exp_big(&x).round(f)
        }
    }
}

// ------------------------------------------------------------------- log

/// ln x fuer x = mant · 2^exp > 0.
///
/// x = m · 2^e mit m in [0.75, 1.5), m · (1 + j/128) = z nahe 1 mit
/// |z - 1| <= 2^-7.4 und exakt (m traegt hoechstens 53 Bit, 1 + j/128
/// acht), ln z = 2 atanh(t) mit t = (z - 1) / (z + 1).
/// ln x = (e ln 2 - ln(1 + j/128)) + ln z; die Summe loescht hoechstens
/// einen Faktor 2,4 aus, und fuer x nahe 1 ist e = j = 0.
fn log_big<const N: usize>(mant: u64, exp: i32) -> Big<N> {
    let x = Big::<N>::from_parts(false, u128::from(mant), exp);
    let top = x.m[N - 1];
    let (e, m) = if top >= 3 << 62 { (x.exp, Big { exp: 0, ..x }) } else { (x.exp - 1, Big { exp: 1, ..x }) };
    // 128/m · 2^8 aus dem obersten Wort, j = round(128/m) - 128 in [-43, 43].
    let q = (1u128 << (79 - m.exp)) / u128::from(top);
    let j = (((q + 128) >> 8) as i64 - 128).clamp(-43, 43);
    let c = Big::<N>::from_parts(false, (128 + j) as u128, -7);
    let u = m.mul(&c).sub(&Big::ONE);
    let t = u.div(&u.add(&Big::from_i64(2)));
    let series = horner(&t.square(), terms(N).log, |i| konst(&INV_ODD[i]));
    let ln_z = t.mul(&series).mul_pow2(1);
    let ln_c = konst::<N>(&LOG128[(j + 43) as usize]);
    let e_ln2 = konst::<N>(&LN2).mul(&Big::from_i64(i64::from(e)));
    e_ln2.sub(&ln_c).add(&ln_z)
}

pub(crate) fn log<const N: usize>(bits: u64, f: Format) -> u64 {
    match classify(bits, f) {
        Class::Nan | Class::Inf { neg: true } | Class::Finite { neg: true, .. } => f.nan(),
        Class::Inf { neg: false } => f.inf(false),
        Class::Zero { .. } => f.inf(true),
        Class::Finite { neg: false, mant, exp } => log_big::<N>(mant, exp).round(f),
    }
}

// ------------------------------------------------------- sin, cos, tan

/// |x| = k · π/2 + r mit |r| <= π/4: (k mod 4, r).
///
/// Unter π/4 ist r = x. Darueber nach Payne und Hanek: x · 2/π modulo 4
/// aus N + 3 Woertern von 2/π ab dem Wort, vor dem nur Vielfache von 4
/// beitragen. Der Nachkommateil traegt dann mindestens 64 (N + 1) Bit hinter
/// den fuehrenden Nullen, die ein `f64` naeher als 2^-62 an ein Vielfaches
/// von π/2 bringt.
fn reduce<const N: usize>(mant: u64, exp: i32) -> (u32, Big<N>) {
    let x = Big::<N>::from_parts(false, u128::from(mant), exp);
    if x.cmp_abs(&pi::<N>().mul_pow2(-2)) == Ordering::Less {
        return (0, x);
    }
    let len = N + 3;
    let i0 = (exp - 2).div_euclid(64).max(0) as usize;
    let mut s = [0u64; MAX + 4];
    let mut carry = 0u128;
    for (t, slot) in s.iter_mut().take(len).enumerate() {
        let word = TWO_OVER_PI.get(i0 + len - 1 - t).copied().unwrap_or(0);
        let v = u128::from(mant) * u128::from(word) + carry;
        *slot = v as u64;
        carry = v >> 64;
    }
    s[len] = carry as u64;
    // s · 2^-fbits ist x · 2/π ohne die Vielfachen von 4.
    let fbits = (64 * (i0 + len) as i32 - exp) as u32;
    let bit = |s: &[u64; MAX + 4], b: u32| (s[(b / 64) as usize] >> (b % 64)) & 1;
    let mut k = (bit(&s, fbits) | (bit(&s, fbits + 1) << 1)) as u32;
    let up = bit(&s, fbits - 1) == 1;
    let (word, rest) = ((fbits / 64) as usize, fbits % 64);
    // Nur der Nachkommateil bleibt.
    for (i, w) in s.iter_mut().enumerate() {
        if i > word || (i == word && rest == 0) {
            *w = 0;
        } else if i == word {
            *w &= (1u64 << rest) - 1;
        }
    }
    if up {
        // Zur naechsten ganzen Zahl: r = f - 1, der Betrag 2^fbits - s.
        k = (k + 1) % 4;
        let mut borrow = true;
        for w in s.iter_mut() {
            let (v, b) = (!*w).overflowing_add(u64::from(borrow));
            *w = v;
            borrow = b;
        }
        for (i, w) in s.iter_mut().enumerate() {
            if i > word || (i == word && rest == 0) {
                *w = 0;
            } else if i == word {
                *w &= (1u64 << rest) - 1;
            }
        }
    }
    let f = Big::<N>::from_words(up, &s[..=len], -(fbits as i32));
    (k, f.mul(&pi()).mul_pow2(-1))
}

/// sin r und cos r fuer |r| <= π/4.
///
/// r = j/64 + s mit |s| <= 1/128, die Additionstheoreme mit sin(j/64) und
/// cos(j/64) aus der Tabelle. Fuer j >= 1 ist sin(j/64) cos s mindestens
/// doppelt so gross wie |cos(j/64) sin s|; die Summe loescht nicht aus.
fn sin_cos<const N: usize>(r: &Big<N>) -> (Big<N>, Big<N>) {
    let a = r.abs();
    let j = a.mul_pow2(6).round_i64().clamp(0, 50) as usize;
    let s = a.sub(&Big::from_parts(false, j as u128, -6));
    let w = s.square();
    let t = terms(N);
    let sin_s = s.mul(&horner(&w, t.sin, |i| signed(&INV_FACT[2 * i + 1], i % 2 == 1)));
    let cos_s = horner(&w, t.cos, |i| signed(&INV_FACT[2 * i], i % 2 == 1));
    let (sin_a, cos_a) = if j == 0 {
        (sin_s, cos_s)
    } else {
        let (sj, cj) = (konst::<N>(&SIN64[j]), konst::<N>(&COS64[j]));
        (sj.mul(&cos_s).add(&cj.mul(&sin_s)), cj.mul(&cos_s).sub(&sj.mul(&sin_s)))
    };
    (if r.neg { sin_a.neg() } else { sin_a }, cos_a)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trig {
    Sin,
    Cos,
    Tan,
}

pub(crate) fn sin<const N: usize>(bits: u64, f: Format) -> u64 {
    trig::<N>(Trig::Sin, bits, f)
}

pub(crate) fn cos<const N: usize>(bits: u64, f: Format) -> u64 {
    trig::<N>(Trig::Cos, bits, f)
}

pub(crate) fn tan<const N: usize>(bits: u64, f: Format) -> u64 {
    trig::<N>(Trig::Tan, bits, f)
}

fn trig<const N: usize>(fun: Trig, bits: u64, f: Format) -> u64 {
    match classify(bits, f) {
        Class::Nan | Class::Inf { .. } => f.nan(),
        Class::Zero { neg } => {
            if fun == Trig::Cos {
                Big::<N>::ONE.round(f)
            } else {
                f.zero(neg)
            }
        }
        Class::Finite { neg, mant, exp } => trig_big::<N>(fun, neg, mant, exp).round(f),
    }
}

/// sin, cos oder tan von `(-1)^neg · mant · 2^exp`, ungerundet.
fn trig_big<const N: usize>(fun: Trig, neg: bool, mant: u64, exp: i32) -> Big<N> {
    let (k, r) = reduce::<N>(mant, exp);
    let (s, c) = sin_cos(&r);
    let v = match (fun, k) {
        (Trig::Sin, 0) | (Trig::Cos, 3) => s,
        (Trig::Sin, 1) | (Trig::Cos, 0) => c,
        (Trig::Sin, 2) | (Trig::Cos, 1) => s.neg(),
        (Trig::Sin, _) | (Trig::Cos, _) => c.neg(),
        (Trig::Tan, k) if k % 2 == 0 => s.div(&c),
        (Trig::Tan, _) => c.div(&s).neg(),
    };
    // sin und tan sind ungerade, cos ist gerade.
    if neg && fun != Trig::Cos { v.neg() } else { v }
}

// ------------------------------------------- atan, atan2, asin, acos

/// atan t fuer t in [0, 1].
///
/// atan t = atan(j/64) + atan(u) mit u = (t - j/64) / (1 + t j/64),
/// |u| <= 1/128; fuer j >= 1 ist atan(j/64) mindestens doppelt so gross wie
/// |atan u|.
fn atan_unit<const N: usize>(t: &Big<N>) -> Big<N> {
    let j = t.mul_pow2(6).round_i64().clamp(0, 64) as usize;
    let u = if j == 0 {
        *t
    } else {
        let c = Big::<N>::from_parts(false, j as u128, -6);
        t.sub(&c).div(&Big::ONE.add(&t.mul(&c)))
    };
    let series = u.mul(&horner(&u.square(), terms(N).atan, |i| signed(&INV_ODD[i], i % 2 == 1)));
    if j == 0 { series } else { konst::<N>(&ATAN64[j]).add(&series) }
}

/// Der Winkel von (ax, ay) fuer positive Betraege, in [0, π/2].
///
/// Unter der Diagonalen atan(ay/ax), darueber π/2 - atan(ax/ay); dort ist
/// das Ergebnis mindestens π/4, die Differenz loescht nicht aus.
fn angle<const N: usize>(ay: &Big<N>, ax: &Big<N>) -> Big<N> {
    if ay.cmp_abs(ax) == Ordering::Greater {
        pi::<N>().mul_pow2(-1).sub(&atan_unit(&ax.div(ay)))
    } else {
        atan_unit(&ay.div(ax))
    }
}

/// Ein Vielfaches von π, gerundet: `num/4 · π`.
fn pi_quarters<const N: usize>(num: u64, neg: bool, f: Format) -> u64 {
    let v = pi::<N>().mul(&Big::from_parts(neg, u128::from(num), -2));
    v.round(f)
}

pub(crate) fn atan<const N: usize>(bits: u64, f: Format) -> u64 {
    match classify(bits, f) {
        Class::Nan => f.nan(),
        Class::Inf { neg } => pi_quarters::<N>(2, neg, f),
        Class::Zero { neg } => f.zero(neg),
        Class::Finite { neg, mant, exp } => atan_big::<N>(neg, mant, exp).round(f),
    }
}

/// atan von `(-1)^neg · mant · 2^exp`, ungerundet.
fn atan_big<const N: usize>(neg: bool, mant: u64, exp: i32) -> Big<N> {
    let x = Big::<N>::from_parts(false, u128::from(mant), exp);
    let a = if x.cmp_abs(&Big::ONE) == Ordering::Greater {
        pi::<N>().mul_pow2(-1).sub(&atan_unit(&x.recip()))
    } else {
        atan_unit(&x)
    };
    if neg { a.neg() } else { a }
}

/// atan2(y, x) nach IEEE 754-2019 9.2: Nullen und Unendlichkeiten ergeben
/// Vielfache von π/4 mit dem Vorzeichen von y.
pub(crate) fn atan2<const N: usize>(y_bits: u64, x_bits: u64, f: Format) -> u64 {
    let (cy, cx) = (classify(y_bits, f), classify(x_bits, f));
    let x_neg = match cx {
        Class::Nan => return f.nan(),
        Class::Inf { neg } | Class::Zero { neg } | Class::Finite { neg, .. } => neg,
    };
    match (cy, cx) {
        (Class::Nan, _) => f.nan(),
        (Class::Zero { neg }, _) => {
            if x_neg {
                pi_quarters::<N>(4, neg, f)
            } else {
                f.zero(neg)
            }
        }
        (Class::Inf { neg }, Class::Inf { .. }) => pi_quarters::<N>(if x_neg { 3 } else { 1 }, neg, f),
        (Class::Inf { neg }, _) | (Class::Finite { neg, .. }, Class::Zero { .. }) => pi_quarters::<N>(2, neg, f),
        (Class::Finite { neg, .. }, Class::Inf { .. }) => {
            if x_neg {
                pi_quarters::<N>(4, neg, f)
            } else {
                f.zero(neg)
            }
        }
        (Class::Finite { neg, mant: my, exp: ey }, Class::Finite { mant: mx, exp: ex, .. }) => {
            let ay = Big::<N>::from_parts(false, u128::from(my), ey);
            let ax = Big::<N>::from_parts(false, u128::from(mx), ex);
            let a = angle(&ay, &ax);
            let a = if x_neg { pi::<N>().sub(&a) } else { a };
            (if neg { a.neg() } else { a }).round(f)
        }
        (_, Class::Nan) => f.nan(),
    }
}

/// asin und acos ueber den Winkel von (sqrt(1 - x^2), |x|); 1 - x^2 als
/// (1 - |x|)(1 + |x|), beide Faktoren exakt.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arc {
    Asin,
    Acos,
}

pub(crate) fn asin<const N: usize>(bits: u64, f: Format) -> u64 {
    arc::<N>(Arc::Asin, bits, f)
}

pub(crate) fn acos<const N: usize>(bits: u64, f: Format) -> u64 {
    arc::<N>(Arc::Acos, bits, f)
}

fn arc<const N: usize>(fun: Arc, bits: u64, f: Format) -> u64 {
    match classify(bits, f) {
        Class::Nan | Class::Inf { .. } => f.nan(),
        Class::Zero { neg } => {
            if fun == Arc::Asin {
                f.zero(neg)
            } else {
                pi_quarters::<N>(2, false, f)
            }
        }
        Class::Finite { neg, mant, exp } => {
            let ax = Big::<N>::from_parts(false, u128::from(mant), exp);
            match ax.cmp_abs(&Big::ONE) {
                Ordering::Greater => return f.nan(),
                Ordering::Equal => {
                    return match (fun, neg) {
                        (Arc::Asin, _) => pi_quarters::<N>(2, neg, f),
                        (Arc::Acos, false) => f.zero(false),
                        (Arc::Acos, true) => pi_quarters::<N>(4, false, f),
                    };
                }
                Ordering::Less => {}
            }
            arc_big(fun, neg, &ax).round(f)
        }
    }
}

/// asin oder acos von `±ax` fuer `ax < 1`, ungerundet.
fn arc_big<const N: usize>(fun: Arc, neg: bool, ax: &Big<N>) -> Big<N> {
    let root = Big::ONE.sub(ax).mul(&Big::ONE.add(ax)).sqrt();
    match fun {
        Arc::Asin => {
            let a = angle(ax, &root);
            if neg { a.neg() } else { a }
        }
        Arc::Acos => {
            let a = angle(&root, ax);
            if neg { pi::<N>().sub(&a) } else { a }
        }
    }
}

// ------------------------------------------------------------------- pow

/// Ob ein endliches, nicht verschwindendes y ganz ist, und wenn, ob ungerade.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parity {
    Odd,
    Even,
    Fraction,
}

fn parity(c: Class) -> Parity {
    match c {
        Class::Finite { exp, .. } if exp < 0 => Parity::Fraction,
        Class::Finite { exp: 0, .. } => Parity::Odd,
        _ => Parity::Even,
    }
}

/// |x|^y exakt, wenn das Ergebnis dyadisch ist und hoechstens 128 Bit
/// traegt; sonst `None`.
///
/// Nur solche Ergebnisse koennen genau auf einem Mittelpunkt zwischen zwei
/// Gleitkommazahlen liegen; die Naeherung naehme dort die falsche Seite.
/// Mit |x| = xm · 2^xe und y = ±ym · 2^ye (xm, ym ungerade) ist |x|^y nur
/// dyadisch, wenn y = n / 2^k, xm eine 2^k-te Potenz z^(2^k) und xe · n
/// durch 2^k teilbar ist; fuer xm > 1 braucht es zudem n > 0, und k <= 5,
/// weil 3^64 ueber 2^53 liegt.
fn pow_exact<const N: usize>(xm: u64, xe: i32, yneg: bool, ym: u64, ye: i32) -> Option<Big<N>> {
    if xm == 1 {
        // |x| = 2^xe: das Ergebnis 2^(xe · y), wenn der Exponent ganz ist.
        let n = i128::from(xe) * i128::from(ym) * if yneg { -1 } else { 1 };
        let e = if ye >= 0 {
            if ye > 12 {
                return None;
            }
            n << ye
        } else {
            let k = (-ye) as u32;
            if k >= 64 || n % (1i128 << k) != 0 {
                return None;
            }
            n >> k
        };
        // Jenseits davon Ueberlauf oder null, das rechnet die Naeherung.
        if e.abs() > 4096 {
            return None;
        }
        return Some(Big::from_parts(false, 1, e as i32));
    }
    if yneg {
        return None;
    }
    let (n, k) = if ye >= 0 {
        if ye > 7 {
            return None;
        }
        (ym << ye, 0u32)
    } else {
        (ym, (-ye) as u32)
    };
    if k > 5 || n > 128 {
        return None;
    }
    let mut z = xm;
    for _ in 0..k {
        let r = z.isqrt();
        if r * r != z {
            return None;
        }
        z = r;
    }
    let scaled = i64::from(xe) * n as i64;
    if scaled % (1 << k) != 0 {
        return None;
    }
    // z^n durch Quadrieren, mit Pruefung auf 128 Bit.
    let (mut p, mut base, mut e) = (1u128, u128::from(z), n);
    while e > 0 {
        if e & 1 == 1 {
            p = p.checked_mul(base)?;
        }
        e >>= 1;
        if e > 0 {
            base = base.checked_mul(base)?;
        }
    }
    Some(Big::from_parts(false, p, (scaled >> k) as i32))
}

/// pow nach IEEE 754-2019 9.2.1: pow(x, ±0) = 1 und pow(+1, y) = 1 auch fuer
/// NaN; eine negative Basis mit nicht ganzzahligem Exponenten ist NaN.
pub(crate) fn pow<const N: usize>(x_bits: u64, y_bits: u64, f: Format) -> u64 {
    let (cx, cy) = (classify(x_bits, f), classify(y_bits, f));
    let one = Big::<N>::ONE.round(f);
    if matches!(cy, Class::Zero { .. }) || cx == (Class::Finite { neg: false, mant: 1, exp: 0 }) {
        return one;
    }
    if matches!(cx, Class::Nan) || matches!(cy, Class::Nan) {
        return f.nan();
    }
    let odd = parity(cy) == Parity::Odd;
    match (cx, cy) {
        (_, Class::Inf { neg: y_neg }) => {
            // Der Betrag von x gegen eins.
            let order = match cx {
                Class::Inf { .. } => Ordering::Greater,
                Class::Zero { .. } => Ordering::Less,
                Class::Finite { mant, exp, .. } => {
                    let b = 64 - mant.leading_zeros() as i32;
                    if mant == 1 && exp == 0 {
                        Ordering::Equal
                    } else if b + exp <= 0 {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    }
                }
                Class::Nan => return f.nan(),
            };
            match order {
                Ordering::Equal => one,
                Ordering::Less if y_neg => f.inf(false),
                Ordering::Less => f.zero(false),
                Ordering::Greater if y_neg => f.zero(false),
                Ordering::Greater => f.inf(false),
            }
        }
        (Class::Inf { neg: x_neg }, Class::Finite { neg: y_neg, .. }) => {
            let neg = x_neg && odd;
            if y_neg { f.zero(neg) } else { f.inf(neg) }
        }
        (Class::Zero { neg: x_neg }, Class::Finite { neg: y_neg, .. }) => {
            let neg = x_neg && odd;
            if y_neg { f.inf(neg) } else { f.zero(neg) }
        }
        (Class::Finite { neg: x_neg, mant: xm, exp: xe }, Class::Finite { neg: y_neg, mant: ym, exp: ye }) => {
            if x_neg && parity(cy) == Parity::Fraction {
                return f.nan();
            }
            let neg = x_neg && odd;
            let v = match pow_exact::<N>(xm, xe, y_neg, ym, ye) {
                Some(v) => v,
                None => {
                    let t = log_big::<N>(xm, xe).mul(&Big::from_parts(y_neg, u128::from(ym), ye));
                    if t.exp > 11 {
                        return if t.neg { f.zero(neg) } else { f.inf(neg) };
                    }
                    exp_big(&t)
                }
            };
            (if neg { v.neg() } else { v }).round(f)
        }
        _ => f.nan(),
    }
}

/// INT-027: Die Schranke vor der Rundung, gemessen. `f64` rechnet mit vier
/// Woertern, und das Modul sagt, das Zwischenergebnis liege relativ unter
/// 2^-240 neben dem exakten Wert. `tests/unrounded.txt` traegt den exakten
/// Wert auf 256 Bit aus der Dezimalreferenz (`tools/libtaktm.py unrounded`).
#[cfg(test)]
mod unrounded {
    use super::{Arc, Trig, arc_big, atan_big, exp_big, log_big, trig_big};
    use crate::big::{Big, Class, F64, classify};

    /// Die Zahl der Werte in der Datei; eine gekuerzte bestuende sonst.
    const VALUES: usize = 240;

    fn value(fun: &str, bits: u64) -> Big<4> {
        let Class::Finite { neg, mant, exp } = classify(bits, F64) else { panic!("{bits:016x} ist nicht endlich") };
        let x = Big::<4>::from_parts(neg, u128::from(mant), exp);
        match fun {
            "exp" => exp_big(&x),
            "log" => log_big(mant, exp),
            "sin" => trig_big(Trig::Sin, neg, mant, exp),
            "cos" => trig_big(Trig::Cos, neg, mant, exp),
            "tan" => trig_big(Trig::Tan, neg, mant, exp),
            "atan" => atan_big(neg, mant, exp),
            "asin" => arc_big(Arc::Asin, neg, &x.abs()),
            "acos" => arc_big(Arc::Acos, neg, &x.abs()),
            other => panic!("unbekannte Funktion `{other}`"),
        }
    }

    #[test]
    fn every_unrounded_f64_value_is_within_two_to_the_minus_240() {
        let mut seen = 0;
        let mut wide = Vec::new();
        for line in include_str!("../tests/unrounded.txt").lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
            let bad = || -> ! { panic!("`{line}` ist keine Zeile") };
            let (head, tail) = line.split_once(" -> ").unwrap_or_else(|| bad());
            let (fun, x) = head.split_once(' ').unwrap_or_else(|| bad());
            let f: Vec<&str> = tail.split_whitespace().collect();
            let hex = |s: &str| u64::from_str_radix(s, 16).unwrap_or_else(|_| bad());
            let [neg, exp, w0, w1, w2, w3] = f[..] else { bad() };
            let want = Big::<4> {
                neg: neg == "1",
                exp: exp.parse().unwrap_or_else(|_| bad()),
                m: [hex(w0), hex(w1), hex(w2), hex(w3)],
            };
            let got = value(fun, hex(x));
            let diff = got.sub(&want);
            // |got - want| < 2^(exp - 241) <= 2^-240 |want|, denn |want| >= 2^(exp - 1).
            if !diff.is_zero() && diff.exp > want.exp - 241 {
                wide.push(format!("{line}: Abstand 2^{} relativ", diff.exp - want.exp));
            }
            seen += 1;
        }
        assert_eq!(seen, VALUES, "Werte in unrounded.txt");
        assert!(wide.is_empty(), "{} Werte ausserhalb der Schranke:\n{}", wide.len(), wide.join("\n"));
    }
}
