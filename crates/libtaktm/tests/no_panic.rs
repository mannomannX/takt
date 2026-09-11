//! 13.8 verlangt Panic-Freiheit unter Fuzzing.
//!
//! Die Vektoren prüfen, dass die *richtigen* Werte herauskommen; dieser
//! Test prüft, dass überhaupt einer herauskommt — für jedes Bitmuster,
//! auch für NaN, Unendlich und die Subnormalen, die in keinem Vektor
//! stehen können (NaN hat kein eindeutiges Muster).
//!
//! Der Generator ist ein xorshift ohne Abhängigkeit und mit festem
//! Startwert: Ein Fehlschlag ist reproduzierbar, und der Testlauf ist es
//! auch — dieselbe Linie wie überall sonst (9.4.4).

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

/// Die Muster, die erfahrungsgemäß Fehler finden: Ränder, Sonderwerte
/// und was der Zufall dazu liefert.
fn patterns(n: usize) -> Vec<u64> {
    let mut out = vec![
        0x0000_0000_0000_0000, // +0
        0x8000_0000_0000_0000, // -0
        0x0000_0000_0000_0001, // kleinster Subnormaler
        0x000F_FFFF_FFFF_FFFF, // größter Subnormaler
        0x0010_0000_0000_0000, // kleinster Normaler
        0x3FF0_0000_0000_0000, // 1
        0x7FEF_FFFF_FFFF_FFFF, // größter endlicher Wert
        0x7FF0_0000_0000_0000, // +inf
        0xFFF0_0000_0000_0000, // -inf
        0x7FF8_0000_0000_0000, // stilles NaN
        0x7FF0_0000_0000_0001, // signalisierendes NaN
        0xFFF8_0000_0000_0000, // negatives NaN
    ];
    let mut rng = Rng(0x2026_0911);
    out.extend((0..n).map(|_| rng.next()));
    out
}

#[test]
fn no_input_makes_a_function_panic() {
    let bits = patterns(20_000);
    for &b in &bits {
        let x = f64::from_bits(b);
        let y = f32::from_bits(b as u32);

        // Ohne `std` gibt es diese sechs nicht (plan/m4.md 2.3c).
        #[cfg(feature = "std")]
        {
            let _ = libtaktm::sqrt_f64(x);
            let _ = libtaktm::round_f64(x);
            let _ = libtaktm::floor_f64(x);
            let _ = libtaktm::ceil_f64(x);
            let _ = libtaktm::trunc_f64(x);
            let _ = libtaktm::sqrt_f32(y);
            let _ = libtaktm::round_f32(y);
            let _ = libtaktm::floor_f32(y);
            let _ = libtaktm::ceil_f32(y);
            let _ = libtaktm::trunc_f32(y);
        }
        let _ = libtaktm::fabs_f64(x);
        let _ = libtaktm::fabs_f32(y);
    }

    // Die mehrstelligen Funktionen über Paaren und Tripeln derselben Menge.
    for w in bits.windows(3).step_by(7) {
        let (a, b, c) = (f64::from_bits(w[0]), f64::from_bits(w[1]), f64::from_bits(w[2]));
        let _ = libtaktm::copysign_f64(a, b);
        let _ = libtaktm::copysign_f32(f32::from_bits(w[0] as u32), f32::from_bits(w[1] as u32));
        #[cfg(feature = "std")]
        {
            let _ = libtaktm::fma_f64(a, b, c);
            let _ = libtaktm::fma_f32(
                f32::from_bits(w[0] as u32),
                f32::from_bits(w[1] as u32),
                f32::from_bits(w[2] as u32),
            );
        }
    }
}

/// `abs` löscht das Vorzeichenbit und sonst nichts — auch bei NaN, wo
/// die Nutzlast erhalten bleiben muss.
#[test]
fn abs_only_clears_the_sign_bit() {
    for &b in &patterns(5_000) {
        let got = libtaktm::fabs_f64(f64::from_bits(b)).to_bits();
        assert_eq!(got, b & !(1u64 << 63), "abs({b:016x}) loescht mehr als das Vorzeichen");
    }
}

/// `copysign` nimmt Betrag und Vorzeichen aus verschiedenen Quellen.
#[test]
fn copysign_takes_sign_from_the_second_argument() {
    let bits = patterns(2_000);
    for w in bits.windows(2) {
        let got = libtaktm::copysign_f64(f64::from_bits(w[0]), f64::from_bits(w[1])).to_bits();
        let want = (w[0] & !(1u64 << 63)) | (w[1] & (1u64 << 63));
        assert_eq!(got, want, "copysign({:016x}, {:016x})", w[0], w[1]);
    }
}
