//! Die Messschleife von `takt driver-test --board` (13.8): ein Output-Pin
//! auf einen Input-Pin gebrueckt, Zeiten aus dem Zyklenzaehler.
//!
//! **Was gemessen wird.** Je Tick liest der Rahmen den Eingang und
//! schreibt danach den Ausgang (12.1). Daraus entstehen die drei Werte der
//! Hardware-Konfiguration (8.10):
//!
//! * `guard_ns` des Outputs: vom Aufruf des Schreibtreibers, bis der Pegel
//!   am Eingang ansteht — die Treiberlatenz aus der Latenzformel (7.5).
//! * `jitter_ns` des Outputs: wie weit der Schreibaufruf um den Beginn des
//!   Ticks streut, gemessen gegen das Lesen, mit dem der Tick beginnt.
//!   Dass die Rahmen geplante Ausgaben nur zu Tickbeginn anwenden (9.8),
//!   misst die Schleife nicht; es ist `tick_granular` (7.5).
//! * `latency_ns` des Inputs: wie lange ein Lesevorgang dauert, bis der
//!   Wert im Prozessabbild steht. Dass zwischen zwei Abtastungen ein Tick
//!   liegt, rechnet die Latenzformel selbst.
//!
//! Die Schleife prueft sich dabei selbst: Liest der Eingang nicht den
//! zuletzt geschriebenen Pegel, oder kommt ein Pegel nicht binnen Frist
//! an, ist die Bruecke offen, und keine Zahl gilt.

use core::fmt::{self, Write};

/// Was die Schleife ueber einen Lauf sammelt, in Zyklen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WireStats {
    /// Schreibvorgaenge, deren Pegel ankam.
    pub arrived: u32,
    /// Schreibvorgaenge, deren Pegel nicht binnen Frist am Eingang stand.
    pub lost: u32,
    /// Lesevorgaenge, die nicht den zuletzt geschriebenen Pegel sahen.
    pub wrong: u32,
    /// Laengste Zeit vom Schreibaufruf bis zum Pegel am Eingang.
    pub guard: u32,
    /// Fruehester und spaetester Schreibaufruf nach dem Lesen des Ticks.
    pub offset: Option<(u32, u32)>,
    /// Laengster Lesevorgang.
    pub read: u32,
    /// Beginn des letzten Lesevorgangs.
    read_at: Option<u32>,
    /// Zuletzt geschriebener Pegel.
    level: Option<bool>,
}

impl WireStats {
    /// Ein Lesevorgang von `start` bis `end`, der `level` sah.
    pub fn read(&mut self, start: u32, end: u32, level: bool) {
        self.read = self.read.max(end.wrapping_sub(start));
        self.read_at = Some(start);
        if self.level.is_some_and(|l| l != level) {
            self.wrong = self.wrong.saturating_add(1);
        }
    }

    /// Ein Schreibvorgang von `level`, begonnen bei `start`; `arrived` ist
    /// der Zaehlerstand, bei dem der Eingang ihn zeigte, `None` nach
    /// Ablauf der Frist.
    pub fn wrote(&mut self, start: u32, arrived: Option<u32>, level: bool) {
        self.level = Some(level);
        let Some(at) = arrived else {
            self.lost = self.lost.saturating_add(1);
            return;
        };
        self.arrived = self.arrived.saturating_add(1);
        self.guard = self.guard.max(at.wrapping_sub(start));
        if let Some(read_at) = self.read_at {
            let o = start.wrapping_sub(read_at);
            self.offset = Some(self.offset.map_or((o, o), |(lo, hi)| (lo.min(o), hi.max(o))));
        }
    }

    /// Hat die Schleife gemessen, und war die Bruecke geschlossen?
    pub fn closed(&self) -> bool {
        self.arrived > 0 && self.lost == 0 && self.wrong == 0
    }

    /// Die Zeile fuer den Host (`takt_conformance::wire::parse`), Zeiten in Nanosekunden,
    /// aufgerundet: Eine Latenz, die abgerundet wird, verspricht zu viel.
    pub fn report(&self, core_hz: u32, out: &mut impl Write) -> fmt::Result {
        let ns = |cycles: u32| (u64::from(cycles) * 1_000_000_000).div_ceil(u64::from(core_hz.max(1)));
        let spread = self.offset.map_or(0, |(lo, hi)| hi - lo);
        write!(
            out,
            "takt schleife angekommen {} verloren {} falsch {} guard_ns {} jitter_ns {} latency_ns {}",
            self.arrived,
            self.lost,
            self.wrong,
            ns(self.guard),
            ns(spread),
            ns(self.read)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Line([u8; 160], usize);

    impl Write for Line {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let end = self.1 + s.len();
            self.0.get_mut(self.1..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
            self.1 = end;
            Ok(())
        }
    }

    fn line(stats: &WireStats, core_hz: u32) -> Line {
        let mut l = Line([0; 160], 0);
        stats.report(core_hz, &mut l).expect("passt");
        l
    }

    fn text(l: &Line) -> &str {
        core::str::from_utf8(&l.0[..l.1]).expect("ASCII")
    }

    #[test]
    fn a_closed_loop_reports_guard_spread_and_read_time() {
        let mut s = WireStats::default();
        // Tick 1: lesen 100..110, schreiben ab 150, Pegel bei 162.
        s.read(100, 110, false);
        s.wrote(150, Some(162), true);
        // Tick 2: sieht den Pegel, schreibt spaeter.
        s.read(1100, 1108, true);
        s.wrote(1170, Some(1180), false);
        assert!(s.closed());
        assert_eq!((s.guard, s.offset, s.read), (12, Some((50, 70)), 10));
        let l = line(&s, 1_000_000_000);
        assert_eq!(text(&l), "takt schleife angekommen 2 verloren 0 falsch 0 guard_ns 12 jitter_ns 20 latency_ns 10");
    }

    #[test]
    fn an_open_bridge_is_not_a_measurement() {
        let mut s = WireStats::default();
        s.read(0, 5, false);
        s.wrote(10, None, true);
        s.read(1000, 1005, false);
        assert!(!s.closed());
        assert_eq!((s.lost, s.wrong), (1, 1));
    }

    #[test]
    fn nanoseconds_round_up_on_a_slow_clock() {
        // 84 MHz: ein Zyklus sind 11,9 ns; abgerundet versprache die
        // Zeile eine kuerzere Latenz, als der Treiber hat.
        let mut s = WireStats::default();
        s.read(0, 1, false);
        s.wrote(2, Some(3), true);
        assert!(text(&line(&s, 84_000_000)).ends_with("guard_ns 12 jitter_ns 0 latency_ns 12"));
    }

    #[test]
    fn the_counter_may_wrap_between_read_and_write() {
        let mut s = WireStats::default();
        s.read(u32::MAX - 4, u32::MAX, false);
        s.wrote(3, Some(9), true);
        assert_eq!((s.guard, s.offset), (6, Some((8, 8))));
    }
}
