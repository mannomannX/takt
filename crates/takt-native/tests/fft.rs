//! `fft256` gegen die Definition (4.5, 11.4): exakte Spektren einfacher
//! Folgen, die direkte DFT fuer erzeugte Folgen beider Breiten, die
//! Packung des Ergebnisses und die Laengen der kanonischen Form.

use takt_native::fft::{self, BYTES_F32, BYTES_F64, N};

fn encode64(x: &[f64]) -> Vec<u8> {
    x.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn decode64(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().expect("8 Byte"))).collect()
}

fn decode32(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(4).map(|c| f64::from(f32::from_le_bytes(c.try_into().expect("4 Byte")))).collect()
}

fn transform(input: &[u8]) -> Vec<u8> {
    let mut out = [0u8; BYTES_F64];
    let len = fft::fft256(input, &mut out).expect("Laenge der kanonischen Form");
    out[..len].to_vec()
}

/// Die direkte DFT in `f64`, in die Packung von CMSIS-DSP gebracht:
/// `[Re X0, Re X128, Re X1, Im X1, …]`. Die Winkel kommen ganzzahlig
/// reduziert in die Bibliothek, damit ihr Fehler klein bleibt.
fn dft(x: &[f64]) -> Vec<f64> {
    let bin = |k: usize| {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, v) in x.iter().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * ((k * n) % N) as f64 / N as f64;
            re += v * angle.cos();
            im -= v * angle.sin();
        }
        (re, im)
    };
    let mut out = vec![bin(0).0, bin(N / 2).0];
    for k in 1..N / 2 {
        let (re, im) = bin(k);
        out.extend([re, im]);
    }
    out
}

#[test]
fn simple_sequences_have_exact_spectra() {
    // Ein Impuls: jedes Xk ist 1.
    let mut impulse = vec![0.0; N];
    impulse[0] = 1.0;
    let got = decode64(&transform(&encode64(&impulse)));
    let mut want = vec![1.0, 1.0];
    for _ in 1..N / 2 {
        want.extend([1.0, 0.0]);
    }
    assert_eq!(got, want, "Impuls");
    // Eine Konstante: alles in X0.
    let got = decode64(&transform(&encode64(&[1.0; N])));
    assert_eq!(got[0], N as f64);
    assert!(got[1..].iter().all(|v| *v == 0.0), "Konstante: {:?}", &got[..8]);
    // +1, -1, +1, …: alles in X128.
    let alternating: Vec<f64> = (0..N).map(|n| if n % 2 == 0 { 1.0 } else { -1.0 }).collect();
    let got = decode64(&transform(&encode64(&alternating)));
    assert_eq!(got[1], N as f64);
    assert!(got.iter().enumerate().all(|(i, v)| i == 1 || *v == 0.0), "Wechsel: {:?}", &got[..8]);
}

#[test]
fn generated_sequences_match_the_direct_dft() {
    for seed in [1, 7, 42, 0xdead_beef] {
        for (wide, tolerance) in [(true, 1e-12), (false, 2e-5)] {
            let mut input = [0u8; BYTES_F64];
            let len = fft::signal(seed, wide, &mut input);
            let x = if wide { decode64(&input[..len]) } else { decode32(&input[..len]) };
            let got = if wide { decode64(&transform(&input[..len])) } else { decode32(&transform(&input[..len])) };
            let want = dft(&x);
            let scale = want.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            let worst = got.iter().zip(&want).fold(0.0f64, |m, (g, w)| m.max((g - w).abs())) / scale;
            assert!(worst < tolerance, "Seed {seed}, f64 {wide}: relativer Fehler {worst:e}");
        }
    }
}

#[test]
fn only_the_two_canonical_lengths_are_accepted() {
    let mut out = [0u8; BYTES_F64];
    for len in [0, 1, BYTES_F32 - 1, BYTES_F32 + 1, BYTES_F64 - 1, BYTES_F64 + 8] {
        assert_eq!(fft::fft256(&vec![0u8; len], &mut out), None, "{len} Byte");
    }
    assert_eq!(fft::fft256(&[0u8; BYTES_F32], &mut out), Some(BYTES_F32));
    assert_eq!(fft::fft256(&[0u8; BYTES_F64], &mut out), Some(BYTES_F64));
}

#[test]
fn generated_inputs_are_named_by_width_and_seed() {
    let mut a = [0u8; BYTES_F64];
    let mut b = [0u8; BYTES_F64];
    assert_eq!(fft::generated("@f64/7", &mut a), Some(BYTES_F64));
    assert_eq!(fft::signal(7, true, &mut b), BYTES_F64);
    assert_eq!(a, b);
    assert_eq!(fft::generated("@f32/7", &mut a), Some(BYTES_F32));
    for bad in ["f64/7", "@f16/7", "@f64/", "@f64/x", "@f64", "@f64:7"] {
        assert_eq!(fft::generated(bad, &mut a), None, "{bad}");
    }
    // Die Werte liegen in [-1, 1).
    let x = decode64(&b);
    assert!(x.iter().all(|v| (-1.0..1.0).contains(v)), "{:?}", &x[..4]);
}
