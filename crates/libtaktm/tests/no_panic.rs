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

/// INT-026: Die transzendenten Funktionen beider Breiten mit beliebigen
/// Bitmustern — darin NaN, ±∞, Subnormale und Muster, deren Reduktion die
/// 256-Bit-Rechnung und die Tabellen bis an ihre Raender fuehrt. Der Test
/// laeuft im Debug-Profil, wo jeder Ueberlauf einer Ganzzahl und jeder
/// Index ausserhalb panikt. Ein NaN geht als NaN hinaus.
#[test]
fn no_input_makes_a_transcendental_function_panic() {
    type Unary64 = fn(f64) -> f64;
    type Unary32 = fn(f32) -> f32;
    let wide: [(&str, Unary64); 8] = [
        ("exp", libtaktm::exp_f64),
        ("log", libtaktm::log_f64),
        ("sin", libtaktm::sin_f64),
        ("cos", libtaktm::cos_f64),
        ("tan", libtaktm::tan_f64),
        ("asin", libtaktm::asin_f64),
        ("acos", libtaktm::acos_f64),
        ("atan", libtaktm::atan_f64),
    ];
    let narrow: [(&str, Unary32); 8] = [
        ("exp", libtaktm::exp_f32),
        ("log", libtaktm::log_f32),
        ("sin", libtaktm::sin_f32),
        ("cos", libtaktm::cos_f32),
        ("tan", libtaktm::tan_f32),
        ("asin", libtaktm::asin_f32),
        ("acos", libtaktm::acos_f32),
        ("atan", libtaktm::atan_f32),
    ];
    let bits = patterns(4_000);
    // Jeder Exponent beider Breiten, darin die Subnormalen, mit zufaelliger
    // Mantisse und beiden Vorzeichen: Zufaellige Muster treffen kleine
    // Betraege und die Naehe von eins sonst kaum.
    let mut rng = Rng(0x0026_2026);
    let mut swept = Vec::new();
    for e in 0..2048u64 {
        for sign in [0, 1u64 << 63] {
            let narrow = (e & 0xff) << 23 | (rng.next() & 0x7f_ffff) | (sign >> 32);
            swept.push(sign | e << 52 | (rng.next() >> 12));
            swept.push(narrow);
        }
    }
    for &b in bits.iter().chain(&swept) {
        let (x, y) = (f64::from_bits(b), f32::from_bits(b as u32));
        for (name, f) in wide {
            assert!(!x.is_nan() || f(x).is_nan(), "{name}({b:016x})");
            let _ = f(x);
        }
        for (name, f) in narrow {
            assert!(!y.is_nan() || f(y).is_nan(), "{name}_f32({:08x})", b as u32);
            let _ = f(y);
        }
    }
    for w in bits.windows(2).step_by(3) {
        let (a, b) = (f64::from_bits(w[0]), f64::from_bits(w[1]));
        let (c, d) = (f32::from_bits(w[0] as u32), f32::from_bits(w[1] as u32));
        let _ = (libtaktm::atan2_f64(a, b), libtaktm::pow_f64(a, b));
        let _ = (libtaktm::atan2_f32(c, d), libtaktm::pow_f32(c, d));
    }
}

/// INT-026, INT-008: `x · num / den` mit beliebigen Bitmustern und
/// Faktoren von null bis 128 Bit, auch Zaehler oder Nenner null.
#[test]
fn no_input_makes_the_unit_scaling_panic() {
    let bits = patterns(4_000);
    let mut rng = Rng(0x0308_2026);
    for &b in &bits {
        let num = (u128::from(rng.next()) << 64 | u128::from(rng.next())) >> (rng.next() % 129).min(127);
        let den = (u128::from(rng.next()) << 64 | u128::from(rng.next())) >> (rng.next() % 129).min(127);
        for (num, den) in [(num, den), (0, den), (num, 0), (u128::MAX, 1), (1, u128::MAX)] {
            let _ = libtaktm::scale_f64(f64::from_bits(b), num, den);
            let _ = libtaktm::scale_f32(f32::from_bits(b as u32), num, den);
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
