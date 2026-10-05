//! `x · num / den`, korrekt gerundet (3.2, INT-008): die Einheitenumrechnung
//! gegen die exakte Referenz in Bruechen (`tools/libtaktm.py scale`).
//!
//! Die Vektoren decken Faktoren von 1 bis 128 Bit, die Faktoren der
//! Praxis (bar/psi, degF), Gleichstaende, den Ueberlauf und den
//! subnormalen Bereich ab.

/// Die Zahl der Vektoren in `scale.txt`: Eine gekuerzte Datei bestuende
/// sonst still.
const VECTORS: usize = 800;

#[test]
fn every_scale_vector_matches_the_exact_reference() {
    let text = include_str!("scale.txt");
    let mut seen = 0;
    let mut failed = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let (head, want) = line.split_once(" -> ").unwrap_or_else(|| panic!("`{line}`"));
        let mut parts = head.split_whitespace();
        let width = parts.next().unwrap_or_default();
        assert_eq!(parts.next(), Some("scale:"), "`{line}`");
        let x = u64::from_str_radix(parts.next().unwrap_or_default(), 16).expect("x");
        let num: u128 = parts.next().unwrap_or_default().parse().expect("Zaehler");
        let den: u128 = parts.next().unwrap_or_default().parse().expect("Nenner");
        let want = u64::from_str_radix(want, 16).expect("Ergebnis");
        let got = match width {
            "f64" => libtaktm::scale_f64(f64::from_bits(x), num, den).to_bits(),
            "f32" => u64::from(libtaktm::scale_f32(f32::from_bits(x as u32), num, den).to_bits()),
            other => panic!("Breite `{other}`"),
        };
        if got != want {
            failed.push(format!("{line}: {got:x}"));
        }
        seen += 1;
    }
    assert_eq!(seen, VECTORS, "Vektoren in scale.txt");
    assert!(failed.is_empty(), "{} abweichend:\n{}", failed.len(), failed.join("\n"));
}

/// 3.2: `1e300 psi .to(bar)` ist endlich — kein Zwischenwert laeuft ueber,
/// obwohl `1e300 * 6894757293168` jenseits von `f64::MAX` laege.
#[test]
fn a_finite_result_has_no_overflowing_intermediate() {
    let psi_to_bar = (6_894_757_293_168u128, 100_000u128 * 1_000_000_000);
    let bar = libtaktm::scale_f64(1e300, psi_to_bar.0, psi_to_bar.1);
    assert!(bar.is_finite(), "{bar}");
    let back = libtaktm::scale_f64(f64::MAX, 1, 2);
    assert_eq!(back, f64::MAX / 2.0);
}
