//! Die gemessene Tickperiode (7.1, 12.6 Zeile 7).
//!
//! **Hier wird gemessen, nicht geurteilt.** Die Regel — „erst nach `N`
//! aufeinanderfolgenden Verletzungen ist es `Runtime(Hardware)`" — steht
//! bereits in `takt_hal::Edge::period` und gilt fuer jedes Profil gleich.
//! Sie ein zweites Mal zu schreiben hiesse, zwei Stellen zu haben, an denen
//! „`2 pct for 10 ticks`" steht, und eine davon wuerde eines Tages anders
//! gepflegt.
//!
//! Was dem `baremetal`-Profil eigen ist, ist die *Quelle* der Zahl: Auf
//! Linux misst die Runtime ihre eigene Schlafzeit, auf der MCU liefert der
//! Timer die Periode (12.3). [`Period`] haelt diese Quelle fest und reicht
//! weiter, was der Rand braucht.

/// Die gemessene Periode gegenueber der nominalen (7.1).
///
/// `tick` in `system:` bleibt der nominale Wert der Semantik; was der
/// Timer liefert, ist eine Beobachtung. Beide auseinanderzuhalten ist der
/// Grund, warum diese Struktur existiert und nicht einfach ein `i64`
/// herumgereicht wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    /// Der nominale Wert aus `system: tick`, in Nanosekunden.
    pub nominal_ns: i64,
    /// Was der Timer zuletzt geliefert hat, in Nanosekunden.
    pub measured_ns: i64,
}

impl Period {
    /// Die Abweichung in Nanosekunden, vorzeichenlos.
    pub fn deviation_ns(self) -> u64 {
        self.measured_ns.abs_diff(self.nominal_ns)
    }

    /// Die Toleranzschwelle zu einem Anteil in Prozent.
    ///
    /// 7.1 nennt `2 pct` als Default. Die Schwelle wird aus dem
    /// *nominalen* Wert gebildet, nicht aus dem gemessenen: Sonst
    /// verschoebe eine driftende Uhr ihre eigene Schranke mit.
    pub fn threshold_ns(self, percent: i64) -> i64 {
        self.nominal_ns.saturating_mul(percent) / 100
    }

    /// Liegt die Periode ausserhalb der Toleranz?
    ///
    /// Das ist noch **kein** Fault: Ob daraus `Runtime(Hardware)` wird,
    /// entscheidet die Lauflaenge (`for N ticks`, 7.1), und das rechnet
    /// `takt_hal::Edge::period` fuer alle Profile gleich.
    pub fn out_of_tolerance(self, percent: i64) -> bool {
        self.deviation_ns() > self.threshold_ns(percent).unsigned_abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    #[test]
    fn a_period_within_two_percent_is_tolerated() {
        let p = Period { nominal_ns: MS, measured_ns: MS + 19_000 };
        assert!(!p.out_of_tolerance(2), "19 us sind unter 2 pct von 1 ms");
    }

    #[test]
    fn a_period_beyond_two_percent_is_not() {
        let p = Period { nominal_ns: MS, measured_ns: MS + 21_000 };
        assert!(p.out_of_tolerance(2), "21 us sind ueber 2 pct von 1 ms");
    }

    #[test]
    fn a_short_period_counts_like_a_long_one() {
        let fast = Period { nominal_ns: MS, measured_ns: MS - 21_000 };
        let slow = Period { nominal_ns: MS, measured_ns: MS + 21_000 };
        assert_eq!(fast.deviation_ns(), slow.deviation_ns(), "die Richtung entscheidet nicht");
        assert!(fast.out_of_tolerance(2));
    }

    /// Die Schwelle haengt am nominalen Wert, nicht am gemessenen.
    ///
    /// Sonst zoege eine driftende Uhr ihre eigene Schranke hinter sich
    /// her und faende sich immer in der Toleranz.
    #[test]
    fn the_threshold_comes_from_the_nominal_value() {
        let drifted = Period { nominal_ns: MS, measured_ns: 2 * MS };
        assert_eq!(drifted.threshold_ns(2), MS / 50, "2 pct von 1 ms, nicht von 2 ms");
    }
}
