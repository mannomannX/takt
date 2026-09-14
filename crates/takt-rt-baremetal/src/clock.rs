//! Der Tick aus dem Hardware-Timer (12.3, 7.1).
//!
//! 12.3 gibt das Muster vor: „Tick aus Hardware-Timer-Interrupt; die ISR
//! setzt ein Flag und sampelt ggf. zeitkritische Inputs; die Hauptschleife
//! fuehrt den Tick aus. Ist das Flag beim naechsten Interrupt noch gesetzt
//! → `Runtime(Overrun)`."
//!
//! **Der Unterschied zu `linux_rt` ist die Richtung der Zeit.** Dort
//! schlaeft die Runtime bis zu einer absoluten Deadline und *bestimmt*
//! damit, wann der naechste Tick beginnt (12.2). Hier bestimmt es der
//! Timer, und die Schleife wartet auf ihn. Beides erfuellt denselben
//! Trait, weil beides dieselbe Frage beantwortet — „ist der naechste Tick
//! faellig?" —, aber die Antwort kommt aus entgegengesetzten Richtungen.
//!
//! Die Folge steht in 12.3 und ist der Grund fuer [`TimerClock::missed`]:
//! Auf Linux erkennt man einen zu langen Schritt daran, dass die Deadline
//! schon vergangen war. Hier erkennt man ihn daran, dass der Zaehler der
//! ISR um mehr als eins gesprungen ist.

use crate::board::TickSource;
use crate::tolerance::Period;
use takt_rt_core::Clock;

/// Die Tickquelle eines Boards als [`Clock`] des Kerns.
///
/// Sie besitzt den Timer nicht, sie liest ihn: Der Besitz liegt beim
/// Board-Crate, das auch die ISR stellt.
#[derive(Debug)]
pub struct TimerClock<T> {
    timer: T,
    /// Nominale Periode aus `system: tick`, in Nanosekunden.
    nominal_ns: i64,
    /// Stand des ISR-Zaehlers beim letzten Tickbeginn.
    last_count: u64,
    /// Wie viele Tick-Ereignisse die Schleife verpasst hat.
    missed: u64,
}

impl<T: TickSource> TimerClock<T> {
    /// Bindet einen Timer an die nominale Periode.
    pub fn new(timer: T, nominal_ns: i64) -> TimerClock<T> {
        let last_count = timer.ticks();
        TimerClock { timer, nominal_ns, last_count, missed: 0 }
    }

    /// Die zuletzt gemessene Periode gegenueber der nominalen (7.1).
    pub fn period(&self) -> Period {
        Period { nominal_ns: self.nominal_ns, measured_ns: self.timer.last_period_ns() }
    }

    /// Wie viele Tick-Ereignisse die Schleife insgesamt verpasst hat.
    ///
    /// Jedes verpasste Ereignis ist ein `Runtime(Overrun)` (12.3, 7.3).
    /// Gezaehlt wird hier und nicht in der Schleife, weil nur der
    /// ISR-Zaehler sie sieht: Ein Schritt, der zwei Perioden dauert,
    /// laesst die Schleife genau einmal warten — die verlorene Periode
    /// steht allein im Zaehler.
    pub fn missed(&self) -> u64 {
        self.missed
    }

    /// Den Timer zurueckgeben (fuer den Start und fuer Tests).
    pub fn into_inner(self) -> T {
        self.timer
    }
}

impl<T: TickSource> Clock for TimerClock<T> {
    fn now(&self) -> i64 {
        // Die logische Zeit ist die Zahl der Tick-Ereignisse mal der
        // *nominalen* Periode — nicht die Summe der gemessenen. 7.1:
        // „`tick` bleibt der nominale Wert der Semantik." Waere es
        // anders, liefe die logische Zeit mit der Uhr davon, und zwei
        // Laeufe desselben Programms auf verschieden schnellen Uhren
        // haetten verschiedene Traces (Satz 9.4.1).
        self.timer.ticks().saturating_mul(self.nominal_ns.unsigned_abs()) as i64
    }

    fn wait_until(&mut self, _deadline: i64) {
        // Die Deadline interessiert nicht: Der Timer bestimmt den Takt,
        // nicht die Schleife. Was zaehlt, ist, ob schon ein Ereignis
        // vorliegt — dann ist der Schritt zu lang gewesen und die
        // Schleife laeuft ohne Warten weiter.
        let before = self.timer.ticks();
        let elapsed = before.saturating_sub(self.last_count);
        if elapsed == 0 {
            self.timer.wait_for_tick();
        } else {
            // Mehr als ein Ereignis seit dem letzten Tickbeginn: Der
            // Schritt hat seine Periode ueberschritten.
            self.missed = self.missed.saturating_add(elapsed.saturating_sub(1));
        }
        self.last_count = self.timer.ticks();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// Ein Timer, dessen Zaehler der Test stellt — die Attrappe, die
    /// belegt, dass dieses Crate ohne Board baut (plan/m5.md 2.2).
    #[derive(Debug, Default)]
    struct FakeTimer {
        count: Cell<u64>,
        period_ns: i64,
        waits: Cell<u64>,
    }

    impl TickSource for FakeTimer {
        fn ticks(&self) -> u64 {
            self.count.get()
        }

        fn last_period_ns(&self) -> i64 {
            self.period_ns
        }

        fn wait_for_tick(&mut self) {
            self.waits.set(self.waits.get() + 1);
            self.count.set(self.count.get() + 1);
        }
    }

    const MS: i64 = 1_000_000;

    #[test]
    fn the_logical_time_counts_nominal_periods() {
        let timer = FakeTimer { count: Cell::new(3), period_ns: MS + 5_000, ..FakeTimer::default() };
        let clock = TimerClock::new(timer, MS);
        assert_eq!(clock.now(), 3 * MS, "die gemessene Periode aendert die logische Zeit nicht");
    }

    #[test]
    fn a_pending_event_means_the_step_was_too_long() {
        let timer = FakeTimer { count: Cell::new(0), period_ns: MS, ..FakeTimer::default() };
        let mut clock = TimerClock::new(timer, MS);
        // Zwei Ereignisse, waehrend der Schritt lief.
        clock.timer.count.set(2);
        clock.wait_until(0);
        assert_eq!(clock.missed(), 1, "zwei Ereignisse seit dem Tickbeginn heisst eine verlorene Periode");
        assert_eq!(clock.timer.waits.get(), 0, "wer schon zu spaet ist, wartet nicht noch");
    }

    #[test]
    fn without_a_pending_event_the_loop_waits() {
        let timer = FakeTimer { count: Cell::new(0), period_ns: MS, ..FakeTimer::default() };
        let mut clock = TimerClock::new(timer, MS);
        clock.wait_until(0);
        assert_eq!(clock.timer.waits.get(), 1);
        assert_eq!(clock.missed(), 0);
    }
}
