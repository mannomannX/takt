//! Matrizen fester Groesse (Referenz 3.11): die Numerik einmal, fuer den
//! Interpreter und den erzeugten Code (plan/m6.md 2.10).
//!
//! Zeilenweise Ablage: `a[i * n + j]` ist Element (i, j) einer m×n-Matrix.
//! Skalarprodukte sind `fma`-Ketten in fester Reihenfolge (4.2); die LU
//! nimmt Spaltenpivotisierung (den betragsgroessten Eintrag, bei Gleichheit
//! den ersten), Cholesky das Standardverfahren. Keine Funktion prueft auf
//! Endlichkeit — das tut der Aufrufer (4.1).
//!
//! Die Indizes bleiben ausgeschrieben, als (Zeile, Spalte) ueber `get` und
//! `set`: Iteratoren machten die Reihenfolge der `fma`-Kette unleserlich,
//! und sie ist der Punkt. Ein Element ausserhalb des Slices liest sich als
//! null, und was dorthin geschrieben wuerde, faellt weg; die Aufrufer reichen
//! Slices in der Groesse ihrer Matrizen.

/// Ein Element: `f32` oder `f64`, mit korrekt gerundetem `fma` und `sqrt`.
///
/// Die Grundrechenarten stehen als Funktionen der Breite da: Jede ist eine
/// Operation nach IEEE 754, einmal gerundet, ohne Ueberlauf als Fehler.
pub trait Scalar: Copy + PartialOrd {
    /// Null.
    const ZERO: Self;
    /// Eins.
    const ONE: Self;
    /// `a + b`.
    fn add(a: Self, b: Self) -> Self;
    /// `a - b`.
    fn sub(a: Self, b: Self) -> Self;
    /// `a * b`.
    fn mul(a: Self, b: Self) -> Self;
    /// `a / b`.
    fn div(a: Self, b: Self) -> Self;
    /// `-a`.
    fn neg(a: Self) -> Self;
    /// `a * b + c`, einmal gerundet.
    fn fma(a: Self, b: Self, c: Self) -> Self;
    /// Quadratwurzel.
    fn sqrt(self) -> Self;
    /// Betrag.
    fn abs(self) -> Self;
    /// Endlich?
    fn is_finite(self) -> bool;
}

macro_rules! scalar {
    ($t:ty, $fma:ident, $sqrt:ident, $fabs:ident) => {
        impl Scalar for $t {
            const ZERO: Self = 0.0;
            const ONE: Self = 1.0;
            fn add(a: Self, b: Self) -> Self {
                a + b
            }
            fn sub(a: Self, b: Self) -> Self {
                a - b
            }
            fn mul(a: Self, b: Self) -> Self {
                a * b
            }
            fn div(a: Self, b: Self) -> Self {
                a / b
            }
            fn neg(a: Self) -> Self {
                -a
            }
            fn fma(a: Self, b: Self, c: Self) -> Self {
                super::$fma(a, b, c)
            }
            fn sqrt(self) -> Self {
                super::$sqrt(self)
            }
            fn abs(self) -> Self {
                super::$fabs(self)
            }
            fn is_finite(self) -> bool {
                <$t>::is_finite(self)
            }
        }
    };
}

scalar!(f64, fma_f64, sqrt_f64, fabs_f64);
scalar!(f32, fma_f32, sqrt_f32, fabs_f32);

/// Die Matrix ist singulaer: ein Pivot ist exakt null.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Singular;

/// Scratch, den `det`, `inv` und `solve` fuer eine n×n-Matrix brauchen.
pub const fn scratch_len(n: usize) -> usize {
    n.saturating_mul(n)
}

/// Wo Element (i, j) bei `cols` Spalten liegt.
fn index(cols: usize, i: usize, j: usize) -> Option<usize> {
    i.checked_mul(cols)?.checked_add(j)
}

/// Element (i, j) einer Matrix mit `cols` Spalten; ausserhalb null.
fn get<S: Scalar>(a: &[S], cols: usize, i: usize, j: usize) -> S {
    index(cols, i, j).and_then(|k| a.get(k)).copied().unwrap_or(S::ZERO)
}

/// Setzt Element (i, j) einer Matrix mit `cols` Spalten; ausserhalb faellt
/// der Wert weg.
fn set<S: Scalar>(a: &mut [S], cols: usize, i: usize, j: usize, v: S) {
    if let Some(slot) = index(cols, i, j).and_then(|k| a.get_mut(k)) {
        *slot = v;
    }
}

/// Tauscht zwei Eintraege; ausserhalb bleibt alles.
fn swap<T>(v: &mut [T], x: Option<usize>, y: Option<usize>) {
    if let (Some(x), Some(y)) = (x, y) {
        if let Ok([a, b]) = v.get_disjoint_mut([x, y]) {
            core::mem::swap(a, b);
        }
    }
}

/// Kopiert die n×n-Matrix `a` an den Anfang von `out`.
fn copy_square<S: Scalar>(a: &[S], n: usize, out: &mut [S]) {
    for (o, x) in out.iter_mut().zip(a).take(scratch_len(n)) {
        *o = *x;
    }
}

/// `out = a · b` fuer `a` m×k und `b` k×n.
pub fn mul<S: Scalar>(a: &[S], b: &[S], m: usize, k: usize, n: usize, out: &mut [S]) {
    for i in 0..m {
        for j in 0..n {
            let mut acc = S::ZERO;
            for l in 0..k {
                acc = S::fma(get(a, k, i, l), get(b, n, l, j), acc);
            }
            set(out, n, i, j, acc);
        }
    }
}

/// `out = a + b`, elementweise.
pub fn add<S: Scalar>(a: &[S], b: &[S], out: &mut [S]) {
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = S::add(*x, *y);
    }
}

/// `out = a - b`, elementweise.
pub fn sub<S: Scalar>(a: &[S], b: &[S], out: &mut [S]) {
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        *o = S::sub(*x, *y);
    }
}

/// `out = s · a`.
pub fn scale<S: Scalar>(a: &[S], s: S, out: &mut [S]) {
    for (o, x) in out.iter_mut().zip(a) {
        *o = S::mul(*x, s);
    }
}

/// `out = a / s`.
pub fn divide<S: Scalar>(a: &[S], s: S, out: &mut [S]) {
    for (o, x) in out.iter_mut().zip(a) {
        *o = S::div(*x, s);
    }
}

/// `out = aᵀ` fuer `a` m×n; `out` ist n×m.
pub fn transpose<S: Scalar>(a: &[S], m: usize, n: usize, out: &mut [S]) {
    for i in 0..m {
        for j in 0..n {
            set(out, m, j, i, get(a, n, i, j));
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
    let mut swaps = 0usize;
    for k in 0..n {
        let mut p = k;
        let mut best = get(a, n, k, k).abs();
        for i in (k..n).skip(1) {
            let v = get(a, n, i, k).abs();
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
                swap(a, index(n, p, j), index(n, k, j));
            }
            swap(perm, Some(p), Some(k));
            swaps = swaps.saturating_add(1);
        }
        let pivot = get(a, n, k, k);
        for i in (k..n).skip(1) {
            let l = S::div(get(a, n, i, k), pivot);
            set(a, n, i, k, l);
            for j in (k..n).skip(1) {
                set(a, n, i, j, S::fma(S::neg(l), get(a, n, k, j), get(a, n, i, j)));
            }
        }
    }
    Ok(swaps)
}

/// Loest `L·U·x = P·b` fuer die Spalte `col` einer n×k-Matrix `b`; das
/// Ergebnis steht in derselben Spalte von `out`. `rhs(i)` liefert die rechte
/// Seite der Zeile `i` nach der Permutation.
fn lu_solve<S: Scalar>(lu: &[S], n: usize, k: usize, col: usize, rhs: impl Fn(usize) -> S, out: &mut [S]) {
    for i in 0..n {
        let mut acc = rhs(i);
        for j in 0..i {
            acc = S::fma(S::neg(get(lu, n, i, j)), get(out, k, j, col), acc);
        }
        set(out, k, i, col, acc);
    }
    for i in (0..n).rev() {
        let mut acc = get(out, k, i, col);
        for j in (i..n).skip(1) {
            acc = S::fma(S::neg(get(lu, n, i, j)), get(out, k, j, col), acc);
        }
        set(out, k, i, col, S::div(acc, get(lu, n, i, i)));
    }
}

/// Determinante ueber die LU; eine singulaere Matrix hat null.
pub fn det<S: Scalar>(a: &[S], n: usize, scratch: &mut [S], perm: &mut [usize]) -> S {
    copy_square(a, n, scratch);
    match lu(scratch, n, perm) {
        Err(Singular) => S::ZERO,
        Ok(swaps) => {
            let mut d = if swaps % 2 == 0 { S::ONE } else { S::neg(S::ONE) };
            for i in 0..n {
                d = S::mul(d, get(scratch, n, i, i));
            }
            d
        }
    }
}

/// `out = a⁻¹` ueber die LU, spaltenweise gegen die Einheitsmatrix.
pub fn inv<S: Scalar>(a: &[S], n: usize, out: &mut [S], scratch: &mut [S], perm: &mut [usize]) -> Result<(), Singular> {
    copy_square(a, n, scratch);
    lu(scratch, n, perm)?;
    for col in 0..n {
        let perm_col = |i: usize| if perm.get(i) == Some(&col) { S::ONE } else { S::ZERO };
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
    copy_square(a, n, scratch);
    lu(scratch, n, perm)?;
    for col in 0..k {
        lu_solve(scratch, n, k, col, |i| perm.get(i).map_or(S::ZERO, |p| get(b, k, *p, col)), out);
    }
    Ok(())
}

/// Cholesky: `out = L` mit `a = L·Lᵀ`, untere Dreiecksmatrix, oberhalb
/// null; `None`, wenn `a` nicht positiv definit ist.
pub fn cholesky<S: Scalar>(a: &[S], n: usize, out: &mut [S]) -> Option<()> {
    for x in out.iter_mut().take(n.saturating_mul(n)) {
        *x = S::ZERO;
    }
    for j in 0..n {
        let mut d = get(a, n, j, j);
        for k in 0..j {
            d = S::fma(S::neg(get(out, n, j, k)), get(out, n, j, k), d);
        }
        if d.partial_cmp(&S::ZERO) != Some(core::cmp::Ordering::Greater) {
            return None;
        }
        let l = d.sqrt();
        set(out, n, j, j, l);
        for i in (j..n).skip(1) {
            let mut s = get(a, n, i, j);
            for k in 0..j {
                s = S::fma(S::neg(get(out, n, i, k)), get(out, n, j, k), s);
            }
            set(out, n, i, j, S::div(s, l));
        }
    }
    Some(())
}
