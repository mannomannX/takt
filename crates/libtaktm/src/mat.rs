//! Matrizen fester Groesse (Referenz 3.11): die Numerik einmal, fuer den
//! Interpreter und den erzeugten Code (plan/m6.md 2.10).
//!
//! Zeilenweise Ablage: `a[i * n + j]` ist Element (i, j) einer m×n-Matrix.
//! Skalarprodukte sind `fma`-Ketten in fester Reihenfolge (4.2); die LU
//! nimmt Spaltenpivotisierung (den betragsgroessten Eintrag, bei Gleichheit
//! den ersten), Cholesky das Standardverfahren. Keine Funktion prueft auf
//! Endlichkeit — das tut der Aufrufer (4.1).
//!
//! Die Indexrechnung auf flachen Zeilen bleibt ausgeschrieben: Iteratoren
//! machten die Reihenfolge der `fma`-Kette unleserlich, und sie ist der Punkt.
#![allow(clippy::needless_range_loop)]

use core::ops::{Add, Div, Mul, Neg, Sub};

/// Ein Element: `f32` oder `f64`, mit korrekt gerundetem `fma` und `sqrt`.
pub trait Scalar:
    Copy
    + PartialOrd
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    /// Null.
    const ZERO: Self;
    /// Eins.
    const ONE: Self;
    /// `a * b + c`, einmal gerundet.
    fn fma(a: Self, b: Self, c: Self) -> Self;
    /// Quadratwurzel.
    fn sqrt(self) -> Self;
    /// Betrag.
    fn abs(self) -> Self;
    /// Endlich?
    fn is_finite(self) -> bool;
}

impl Scalar for f64 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn fma(a: Self, b: Self, c: Self) -> Self {
        super::fma_f64(a, b, c)
    }
    fn sqrt(self) -> Self {
        super::sqrt_f64(self)
    }
    fn abs(self) -> Self {
        super::fabs_f64(self)
    }
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

impl Scalar for f32 {
    const ZERO: Self = 0.0;
    const ONE: Self = 1.0;
    fn fma(a: Self, b: Self, c: Self) -> Self {
        super::fma_f32(a, b, c)
    }
    fn sqrt(self) -> Self {
        super::sqrt_f32(self)
    }
    fn abs(self) -> Self {
        super::fabs_f32(self)
    }
    fn is_finite(self) -> bool {
        f32::is_finite(self)
    }
}

/// Die Matrix ist singulaer: ein Pivot ist exakt null.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Singular;

/// Scratch, den `det`, `inv` und `solve` fuer eine n×n-Matrix brauchen.
pub const fn scratch_len(n: usize) -> usize {
    n * n
}

/// `out = a · b` fuer `a` m×k und `b` k×n.
pub fn mul<S: Scalar>(a: &[S], b: &[S], m: usize, k: usize, n: usize, out: &mut [S]) {
    for i in 0..m {
        for j in 0..n {
            let mut acc = S::ZERO;
            for l in 0..k {
                acc = S::fma(a[i * k + l], b[l * n + j], acc);
            }
            out[i * n + j] = acc;
        }
    }
}

/// `out = a + b`, elementweise.
pub fn add<S: Scalar>(a: &[S], b: &[S], out: &mut [S]) {
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = *x + *y;
    }
}

/// `out = a - b`, elementweise.
pub fn sub<S: Scalar>(a: &[S], b: &[S], out: &mut [S]) {
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = *x - *y;
    }
}

/// `out = s · a`.
pub fn scale<S: Scalar>(a: &[S], s: S, out: &mut [S]) {
    for (o, x) in out.iter_mut().zip(a) {
        *o = *x * s;
    }
}

/// `out = a / s`.
pub fn divide<S: Scalar>(a: &[S], s: S, out: &mut [S]) {
    for (o, x) in out.iter_mut().zip(a) {
        *o = *x / s;
    }
}

/// `out = aᵀ` fuer `a` m×n; `out` ist n×m.
pub fn transpose<S: Scalar>(a: &[S], m: usize, n: usize, out: &mut [S]) {
    for i in 0..m {
        for j in 0..n {
            out[j * m + i] = a[i * n + j];
        }
    }
}

/// LU-Zerlegung in place mit Spaltenpivotisierung: `a` wird zu `L` (unter
/// der Diagonale, Einsen implizit) und `U`; `perm[i]` ist die Zeile der
/// Eingabe, die an Stelle `i` steht. Liefert die Zahl der Vertauschungen.
pub fn lu<S: Scalar>(a: &mut [S], n: usize, perm: &mut [usize]) -> Result<usize, Singular> {
    for (i, p) in perm.iter_mut().enumerate().take(n) {
        *p = i;
    }
    let mut swaps = 0;
    for k in 0..n {
        let mut p = k;
        let mut best = a[k * n + k].abs();
        for i in k + 1..n {
            let v = a[i * n + k].abs();
            if v > best {
                best = v;
                p = i;
            }
        }
        if best == S::ZERO {
            return Err(Singular);
        }
        if p != k {
            for j in 0..n {
                a.swap(p * n + j, k * n + j);
            }
            perm.swap(p, k);
            swaps += 1;
        }
        let pivot = a[k * n + k];
        for i in k + 1..n {
            let l = a[i * n + k] / pivot;
            a[i * n + k] = l;
            for j in k + 1..n {
                a[i * n + j] = S::fma(-l, a[k * n + j], a[i * n + j]);
            }
        }
    }
    Ok(swaps)
}

/// Loest `L·U·x = P·b` fuer die Spalte `col` einer n×k-Matrix `b`; das
/// Ergebnis steht in derselben Spalte von `out`. `at(i)` liefert die rechte
/// Seite der Zeile `i` nach der Permutation.
fn lu_solve<S: Scalar>(lu: &[S], n: usize, k: usize, col: usize, at: impl Fn(usize) -> S, out: &mut [S]) {
    for i in 0..n {
        let mut acc = at(i);
        for j in 0..i {
            acc = S::fma(-lu[i * n + j], out[j * k + col], acc);
        }
        out[i * k + col] = acc;
    }
    for i in (0..n).rev() {
        let mut acc = out[i * k + col];
        for j in i + 1..n {
            acc = S::fma(-lu[i * n + j], out[j * k + col], acc);
        }
        out[i * k + col] = acc / lu[i * n + i];
    }
}

/// Determinante ueber die LU; eine singulaere Matrix hat null.
pub fn det<S: Scalar>(a: &[S], n: usize, scratch: &mut [S], perm: &mut [usize]) -> S {
    scratch[..n * n].copy_from_slice(&a[..n * n]);
    match lu(scratch, n, perm) {
        Err(Singular) => S::ZERO,
        Ok(swaps) => {
            let mut d = if swaps % 2 == 0 { S::ONE } else { -S::ONE };
            for i in 0..n {
                d = d * scratch[i * n + i];
            }
            d
        }
    }
}

/// `out = a⁻¹` ueber die LU, spaltenweise gegen die Einheitsmatrix.
pub fn inv<S: Scalar>(a: &[S], n: usize, out: &mut [S], scratch: &mut [S], perm: &mut [usize]) -> Result<(), Singular> {
    scratch[..n * n].copy_from_slice(&a[..n * n]);
    lu(scratch, n, perm)?;
    for col in 0..n {
        let perm_col = |i: usize| if perm[i] == col { S::ONE } else { S::ZERO };
        lu_solve(scratch, n, n, col, perm_col, out);
    }
    Ok(())
}

/// Loest `a · x = b` fuer `a` n×n und `b` n×k; `out` ist n×k.
pub fn solve<S: Scalar>(
    a: &[S],
    n: usize,
    b: &[S],
    k: usize,
    out: &mut [S],
    scratch: &mut [S],
    perm: &mut [usize],
) -> Result<(), Singular> {
    scratch[..n * n].copy_from_slice(&a[..n * n]);
    lu(scratch, n, perm)?;
    for col in 0..k {
        lu_solve(scratch, n, k, col, |i| b[perm[i] * k + col], out);
    }
    Ok(())
}

/// Cholesky: `out = L` mit `a = L·Lᵀ`, untere Dreiecksmatrix, oberhalb
/// null; `None`, wenn `a` nicht positiv definit ist.
pub fn cholesky<S: Scalar>(a: &[S], n: usize, out: &mut [S]) -> Option<()> {
    for x in out.iter_mut().take(n * n) {
        *x = S::ZERO;
    }
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d = S::fma(-out[j * n + k], out[j * n + k], d);
        }
        if d.partial_cmp(&S::ZERO) != Some(core::cmp::Ordering::Greater) {
            return None;
        }
        let l = d.sqrt();
        out[j * n + j] = l;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s = S::fma(-out[i * n + k], out[j * n + k], s);
            }
            out[i * n + j] = s / l;
        }
    }
    Some(())
}
