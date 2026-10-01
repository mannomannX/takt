//! `fft256(x: [256] float) -> [256] float` (4.5, 11.4): die diskrete
//! Fouriertransformation einer reellen Folge, X_k = sum x_n e^(-2 pi i k n / 256),
//! ohne Normierung.
//!
//! **Das Ergebnis** liegt gepackt wie bei CMSIS-DSP `arm_rfft_fast`:
//! `[Re X0, Re X128, Re X1, Im X1, …, Re X127, Im X127]`. Das sind alle
//! Informationen des Spektrums einer reellen Folge — X0 und X128 sind
//! reell, X(256-k) ist das Konjugierte von Xk —, in 256 Werten.
//!
//! **Die Rechnung ist festgelegt**, damit jedes Ziel dieselben Bits
//! liefert (4.2): Die Paare `z_n = x_2n + i x_2n+1` gehen durch eine
//! komplexe FFT mit 128 Punkten (Bitumkehr, dann Radix-2 nach Zeit
//! dezimiert, Stufe fuer Stufe), danach trennt eine letzte Schleife die
//! Spektren der geraden und der ungeraden Folge. Jede Operation ist eine einzeln
//! gerundete IEEE-Operation in der Reihenfolge, die hier steht; Rust zieht
//! nichts zu `fma` zusammen. Die Twiddle-Faktoren stehen als korrekt
//! gerundete Bitmuster in [`crate::fft_table`]. Die Breite (`f32` oder
//! `f64`) folgt aus `float` des Programms (4.2) und hier aus der Laenge der
//! kanonischen Form: 1024 oder 2048 Byte.

use crate::fft_table::{COS32, COS64, SIN32, SIN64};

/// Punkte der Transformation.
pub const N: usize = 256;

/// Laenge der kanonischen Form fuer `f32` und `f64` (5.9: little endian,
/// ohne Laenge).
pub const BYTES_F32: usize = N * 4;
/// Siehe [`BYTES_F32`].
pub const BYTES_F64: usize = N * 8;

macro_rules! width {
    ($name:ident, $t:ty, $bytes:expr, $cos:ident, $sin:ident) => {
        /// Die Transformation in einer Breite, von der kanonischen Form in
        /// die kanonische Form. Auf dem Stack liegen nur Real- und
        /// Imaginaerteil der 128 Paare (2 KiB in `f64`).
        fn $name(input: &[u8], out: &mut [u8]) {
            const W: usize = $bytes / N;
            let read = |n: usize| {
                let mut b = [0u8; W];
                b.copy_from_slice(&input[n * W..(n + 1) * W]);
                <$t>::from_le_bytes(b)
            };
            let c = |k: usize| <$t>::from_bits($cos[k]);
            let s = |k: usize| <$t>::from_bits($sin[k]);
            // z_n = x_2n + i x_2n+1, in Bitumkehr-Reihenfolge abgelegt.
            let mut re = [0.0 as $t; N / 2];
            let mut im = [0.0 as $t; N / 2];
            for n in 0..N / 2 {
                let r = (n as u8).reverse_bits() as usize >> 1;
                re[r] = read(2 * n);
                im[r] = read(2 * n + 1);
            }
            // Radix 2, Stufe fuer Stufe: w = e^(-2 pi i k / size) = W256^(k * 256 / size).
            let mut size = 2;
            while size <= N / 2 {
                let half = size / 2;
                let step = N / size;
                for start in (0..N / 2).step_by(size) {
                    for k in 0..half {
                        let (wc, ws) = (c(k * step), s(k * step));
                        let (a, b) = (start + k, start + k + half);
                        // t = z_b * (wc - i ws)
                        let tr = re[b] * wc + im[b] * ws;
                        let ti = im[b] * wc - re[b] * ws;
                        re[b] = re[a] - tr;
                        im[b] = im[a] - ti;
                        re[a] += tr;
                        im[a] += ti;
                    }
                }
                size *= 2;
            }
            let mut write = |i: usize, v: $t| out[i * W..(i + 1) * W].copy_from_slice(&v.to_le_bytes());
            // X0 und X128 aus Z0: die Summe und die Differenz von Real- und
            // Imaginaerteil.
            write(0, re[0] + im[0]);
            write(1, re[0] - im[0]);
            for k in 1..N / 2 {
                let (zr, zi, yr, yi) = (re[k], im[k], re[N / 2 - k], im[N / 2 - k]);
                // E = (Zk + conj Z(128-k)) / 2, O = (Zk - conj Z(128-k)) / 2i
                let er = (zr + yr) * 0.5;
                let ei = (zi - yi) * 0.5;
                let or = (zi + yi) * 0.5;
                let oi = (yr - zr) * 0.5;
                // Xk = E + O * (c - i s) mit c, s von 2 pi k / 256
                let (wc, ws) = (c(k), s(k));
                write(2 * k, er + (or * wc + oi * ws));
                write(2 * k + 1, ei + (oi * wc - or * ws));
            }
        }
    };
}

width!(transform_f32, f32, BYTES_F32, COS32, SIN32);
width!(transform_f64, f64, BYTES_F64, COS64, SIN64);

/// Die Transformation ueber die kanonische Form: 1024 Byte sind `f32`,
/// 2048 Byte `f64`; das Ergebnis in derselben Form nach `out`. `None` fuer
/// jede andere Laenge.
pub fn fft256(input: &[u8], out: &mut [u8; BYTES_F64]) -> Option<usize> {
    match input.len() {
        BYTES_F32 => transform_f32(input, &mut out[..BYTES_F32]),
        BYTES_F64 => transform_f64(input, out),
        _ => return None,
    }
    Some(input.len())
}

/// Ein erzeugter Eingabeblock der Vektoren (grammar/takt-native.md):
/// `@f32/SEED` oder `@f64/SEED`, siehe [`signal`]. `None` fuer alles
/// andere.
pub fn generated(token: &str, out: &mut [u8; BYTES_F64]) -> Option<usize> {
    let (width, seed) = token.strip_prefix('@')?.split_once('/')?;
    let wide = match width {
        "f32" => false,
        "f64" => true,
        _ => return None,
    };
    Some(signal(seed.parse().ok()?, wide, out))
}

/// Eine Eingabe fuer die Vektoren (grammar/takt-native.md, `@f32/SEED`
/// und `@f64/SEED`): 256 Werte aus xorshift64 im Intervall [-1, 1), in der
/// kanonischen Form der Breite. Auf jedem Ziel dieselben Bits: Der Wert
/// entsteht aus 53 beziehungsweise 24 Bit des Generators durch eine exakte
/// Skalierung.
pub fn signal(seed: u64, wide: bool, out: &mut [u8; BYTES_F64]) -> usize {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    if wide {
        for chunk in out.chunks_exact_mut(8) {
            let v = (next() >> 11) as f64 * (1.0 / (1u64 << 52) as f64) - 1.0;
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        BYTES_F64
    } else {
        for chunk in out[..BYTES_F32].chunks_exact_mut(4) {
            let v = (next() >> 40) as f32 * (1.0 / (1u32 << 23) as f32) - 1.0;
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        BYTES_F32
    }
}
