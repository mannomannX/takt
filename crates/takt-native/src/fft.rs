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

/// Die Stufen der komplexen FFT: die Groesse einer Gruppe und der Schritt
/// durch die Twiddle-Tabelle, `N / size` — w = e^(-2 pi i k / size) =
/// W256^(k * N / size).
const STAGES: [(usize, usize); 7] = [(2, 128), (4, 64), (8, 32), (16, 16), (32, 8), (64, 4), (128, 2)];

macro_rules! width {
    ($name:ident, $t:ty, $bytes:expr, $cos:ident, $sin:ident) => {
        /// Die Transformation in einer Breite, von der kanonischen Form in
        /// die kanonische Form. Auf dem Stack liegen nur Real- und
        /// Imaginaerteil der 128 Paare (2 KiB in `f64`).
        fn $name(input: &[u8], out: &mut [u8]) {
            const W: usize = $bytes / N;
            // z_n = x_2n + i x_2n+1, in Bitumkehr-Reihenfolge abgelegt.
            let mut re: [$t; N / 2] = [0.0; N / 2];
            let mut im: [$t; N / 2] = [0.0; N / 2];
            for (n, pair) in (0..=u8::MAX).zip(input.chunks_exact(2 * W)) {
                let r = usize::from(n.reverse_bits() >> 1);
                if let (Some(x), Some(y), Some(zr), Some(zi)) =
                    (pair.first_chunk::<W>(), pair.last_chunk::<W>(), re.get_mut(r), im.get_mut(r))
                {
                    *zr = <$t>::from_le_bytes(*x);
                    *zi = <$t>::from_le_bytes(*y);
                }
            }
            // Radix 2, Stufe fuer Stufe: je Gruppe die untere Haelfte `a`
            // gegen die obere `b`.
            for (size, step) in STAGES {
                for (re_group, im_group) in re.chunks_exact_mut(size).zip(im.chunks_exact_mut(size)) {
                    let (Some((re_a, re_b)), Some((im_a, im_b))) =
                        (re_group.split_at_mut_checked(size / 2), im_group.split_at_mut_checked(size / 2))
                    else {
                        continue;
                    };
                    let a = re_a.iter_mut().zip(im_a.iter_mut());
                    let b = re_b.iter_mut().zip(im_b.iter_mut());
                    let w = $cos.iter().zip($sin.iter()).step_by(step);
                    for (((ar, ai), (br, bi)), (c, s)) in a.zip(b).zip(w) {
                        let (wc, ws) = (<$t>::from_bits(*c), <$t>::from_bits(*s));
                        // t = z_b * (wc - i ws)
                        let tr = *br * wc + *bi * ws;
                        let ti = *bi * wc - *br * ws;
                        *br = *ar - tr;
                        *bi = *ai - ti;
                        *ar += tr;
                        *ai += ti;
                    }
                }
            }
            let mut slots = out.chunks_exact_mut(W).filter_map(|slot| slot.first_chunk_mut::<W>());
            let mut write = |v: $t| {
                if let Some(slot) = slots.next() {
                    *slot = v.to_le_bytes();
                }
            };
            // X0 und X128 aus Z0: die Summe und die Differenz von Real- und
            // Imaginaerteil.
            write(re[0] + im[0]);
            write(re[0] - im[0]);
            // Ab k = 1: Zk vorwaerts, Z(128-k) rueckwaerts.
            let z = re.iter().zip(im.iter()).skip(1);
            let y = re.iter().zip(im.iter()).rev();
            let w = $cos.iter().zip($sin.iter()).skip(1);
            for (((zr, zi), (yr, yi)), (c, s)) in z.zip(y).zip(w) {
                let (zr, zi, yr, yi) = (*zr, *zi, *yr, *yi);
                // E = (Zk + conj Z(128-k)) / 2, O = (Zk - conj Z(128-k)) / 2i
                let er = (zr + yr) * 0.5;
                let ei = (zi - yi) * 0.5;
                let or = (zi + yi) * 0.5;
                let oi = (yr - zr) * 0.5;
                // Xk = E + O * (c - i s) mit c, s von 2 pi k / 256
                let (wc, ws) = (<$t>::from_bits(*c), <$t>::from_bits(*s));
                write(er + (or * wc + oi * ws));
                write(ei + (oi * wc - or * ws));
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
#[expect(
    clippy::as_conversions,
    reason = "MISRA C:2025 R.10.5: 53 und 24 Bit des Generators und die Zweierpotenzen sind in f64 und f32 exakt; aus u64 wandelt nur `as`"
)]
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
