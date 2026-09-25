//! Das Messprogramm von `takt bench` auf dem Board (13.8).
//!
//! **Was hier steht, ist Messung, nicht Urteil.** Das Board zaehlt Zyklen
//! und schreibt sie auf die Leitung; was sie bedeuten — `c_target` je
//! Klasse, das Verhaeltnis Takt zu C, die Reserven —, rechnet der Host
//! (`takt_conformance::bench`), wo die Kostenanalyse des Programms und die
//! Mathematik stehen. Das Board soll nichts wissen, was es nicht messen
//! kann.
//!
//! **Die Zeilen.** Jede beginnt mit `bench`, damit der Host sie aus dem
//! Banner herausliest:
//!
//! ```text
//! bench core_hz 84000000
//! bench takt min 1204 mean 1210 max 1233 n 1000 digest 3735928559
//! bench c min 1180 mean 1181 max 1190 n 1000 digest 3735928559
//! bench stack 1432
//! bench subnormal 0
//! takt end
//! ```
//!
//! `digest` ist der Wert, den Takt-Kern und C-Referenz nach allen Laeufen
//! ergeben: gleich heisst gleiche Semantik, und erst dann ist das
//! Verhaeltnis der Zeiten eine Aussage ueber die Sprache.

use core::hint::black_box;

use crate::telemetry::{Port, Telemetry};

/// Eine Messreihe in Zyklen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Series {
    /// Die kuerzeste Messung.
    pub min: u32,
    /// Die laengste Messung; sie ist die Schranke (plan/m5.md 2.4).
    pub max: u32,
    /// Die Summe aller Messungen.
    pub total: u64,
    /// Wie viele Messungen es waren.
    pub n: u32,
}

impl Series {
    /// Eine leere Reihe.
    pub const fn new() -> Series {
        Series { min: u32::MAX, max: 0, total: 0, n: 0 }
    }

    /// Nimmt eine Messung auf.
    pub fn add(&mut self, cycles: u32) {
        self.min = self.min.min(cycles);
        self.max = self.max.max(cycles);
        self.total = self.total.saturating_add(u64::from(cycles));
        self.n = self.n.saturating_add(1);
    }

    /// Der Mittelwert, abgerundet; null ohne Messung.
    pub fn mean(&self) -> u32 {
        if self.n == 0 { 0 } else { (self.total / u64::from(self.n)) as u32 }
    }
}

impl Default for Series {
    fn default() -> Series {
        Series::new()
    }
}

/// Misst `f` `runs`-mal mit dem Zyklenzaehler `now`.
///
/// Jede Messung umschliesst nur `f`; was um sie herum geschieht, zaehlt
/// nicht. Der Aufrufer schliesst Interrupts aus, wenn er den Kern allein
/// messen will — eine ISR mitten in der Messung ist Last, nicht Kosten
/// des Programms, und gehoert in `T_IO` und den Tick-Jitter.
pub fn series(runs: u32, now: impl Fn() -> u32, mut f: impl FnMut()) -> Series {
    let mut s = Series::new();
    for _ in 0..runs {
        let start = now();
        f();
        s.add(now().wrapping_sub(start));
    }
    s
}

/// Eine Reihe als Zeile: `bench <label> min … mean … max … n … digest …`.
pub fn write_series<P: Port, const N: usize>(t: &mut Telemetry<P, N>, label: &str, s: &Series, digest: u64) {
    t.write("bench ");
    t.write(label);
    for (key, v) in [(" min ", s.min), (" mean ", s.mean()), (" max ", s.max), (" n ", s.n)] {
        t.write(key);
        t.write_u64(u64::from(v));
    }
    t.write(" digest ");
    t.write_u64(digest);
    t.newline();
    t.flush();
}

/// Eine Zahl als Zeile: `bench <label> <wert>`.
pub fn write_value<P: Port, const N: usize>(t: &mut Telemetry<P, N>, label: &str, value: u64) {
    t.write("bench ");
    t.write(label);
    t.write(" ");
    t.write_u64(value);
    t.newline();
    t.flush();
}

/// Eine Zeile `native <i> <ergebnis> stack <byte>` (13.8): das Ergebnis
/// eines Vektors Byte fuer Byte in Hex, dazu der Stack-Bedarf des Aufrufs.
/// `takt_conformance::natives` liest sie und vergleicht mit dem Wirt.
pub fn write_native<P: Port, const N: usize>(t: &mut Telemetry<P, N>, index: usize, result: &[u8], stack: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    t.write("native ");
    t.write_u64(index as u64);
    t.write(" ");
    for b in result {
        t.write_byte(HEX[usize::from(b >> 4)]);
        t.write_byte(HEX[usize::from(b & 15)]);
    }
    t.write(" stack ");
    t.write_u64(u64::from(stack));
    t.newline();
    t.flush();
}

/// Ein Fall des Subnormal-Vektors: zwei Operanden und das erwartete
/// Ergebnis als Bitmuster, dazu die Operation.
type Case<B, F> = (B, B, B, fn(F, F) -> F);

/// Der Referenzvektor fuer Subnormale (4.2, 13.8): die Zahl der Faelle,
/// deren Ergebnis nicht das IEEE-754-Bitmuster ist.
///
/// Null heisst: Die Arithmetik des Kerns rechnet mit Subnormalen, wie 4.2
/// es verlangt. Ein Kern mit Flush-to-Zero liefert statt der kleinen Werte
/// null und faellt hier in jedem Fall auf. `black_box` haelt den Compiler
/// davon ab, die Ergebnisse auf dem Wirt vorauszurechnen — gemessen wird
/// der Kern, nicht der Compiler.
pub fn subnormal_failures() -> u32 {
    let f32_cases: [Case<u32, f32>; 4] = [
        // kleinste Normale halbiert: eine Subnormale
        (0x0080_0000, 0x4000_0000, 0x0040_0000, |a, b| a / b),
        // kleinste Subnormale verdreifacht
        (0x0000_0001, 0x4040_0000, 0x0000_0003, |a, b| a * b),
        // zwei Subnormale ergeben eine Normale
        (0x0040_0000, 0x0040_0000, 0x0080_0000, |a, b| a + b),
        // Differenz zweier Normaler ist subnormal
        (0x0080_0001, 0x0080_0000, 0x0000_0001, |a, b| a - b),
    ];
    let f64_cases: [Case<u64, f64>; 4] = [
        (0x0010_0000_0000_0000, 0x4000_0000_0000_0000, 0x0008_0000_0000_0000, |a, b| a / b),
        (0x0000_0000_0000_0001, 0x4008_0000_0000_0000, 0x0000_0000_0000_0003, |a, b| a * b),
        (0x0008_0000_0000_0000, 0x0008_0000_0000_0000, 0x0010_0000_0000_0000, |a, b| a + b),
        (0x0010_0000_0000_0001, 0x0010_0000_0000_0000, 0x0000_0000_0000_0001, |a, b| a - b),
    ];
    let mut failures = 0;
    for (a, b, want, op) in f32_cases {
        let got = black_box(op)(black_box(f32::from_bits(a)), black_box(f32::from_bits(b)));
        failures += u32::from(got.to_bits() != want);
    }
    for (a, b, want, op) in f64_cases {
        let got = black_box(op)(black_box(f64::from_bits(a)), black_box(f64::from_bits(b)));
        failures += u32::from(got.to_bits() != want);
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_series_keeps_the_extremes_and_the_mean() {
        let mut s = Series::new();
        for c in [10, 30, 20] {
            s.add(c);
        }
        assert_eq!((s.min, s.max, s.mean(), s.n), (10, 30, 20, 3));
        assert_eq!(Series::new().mean(), 0);
    }

    /// Der Zaehler laeuft ueber; die Differenz bleibt richtig.
    #[test]
    fn a_measurement_across_the_wrap_counts_forward() {
        let clock = core::cell::Cell::new(u32::MAX - 4);
        let s = series(3, || clock.get(), || clock.set(clock.get().wrapping_add(10)));
        assert_eq!((s.min, s.max, s.n), (10, 10, 3));
    }

    /// Der Wirt rechnet IEEE-754 mit Subnormalen: Der Vektor besteht.
    #[test]
    fn the_host_passes_the_subnormal_vector() {
        assert_eq!(subnormal_failures(), 0);
    }
}
