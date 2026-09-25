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
//! Einen zu langen Schritt sieht die Schleife an `drift` (7.3): Die Uhr
//! liefert die feine Zeit des Timers, die Frist steht in der Schleife.

use crate::board::TickSource;
use crate::tolerance::Period;
use takt_rt_core::Clock;

/// Die Tickquelle eines Boards als [`Clock`] des Kerns.
///
/// Sie besitzt den Timer nicht, sie liest ihn: Der Besitz liegt beim
/// Board-Crate, das auch die ISR stellt.
pub struct TimerClock<T, F = fn()> {
    timer: T,
    /// Nominale Periode aus `system: tick`, in Nanosekunden.
    nominal_ns: i64,
    /// Laeuft nach jedem Wecken vor der Frist, etwa um den Ring zu leeren.
    idle: F,
}

impl<T: core::fmt::Debug, F> core::fmt::Debug for TimerClock<T, F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TimerClock").field("timer", &self.timer).field("nominal_ns", &self.nominal_ns).finish()
    }
}

fn nothing() {}

impl<T: TickSource> TimerClock<T> {
    /// Bindet einen Timer an die nominale Periode.
    pub fn new(timer: T, nominal_ns: i64) -> TimerClock<T> {
        TimerClock { timer, nominal_ns, idle: nothing }
    }

    /// Mit einer Arbeit fuer die Zeit zwischen den Ticks.
    pub fn with_idle<F: FnMut()>(self, idle: F) -> TimerClock<T, F> {
        TimerClock { timer: self.timer, nominal_ns: self.nominal_ns, idle }
    }
}

impl<T: TickSource, F: FnMut()> TimerClock<T, F> {
    /// Die zuletzt gemessene Periode gegenueber der nominalen (7.1).
    pub fn period(&self) -> Period {
        Period { nominal_ns: self.nominal_ns, measured_ns: self.timer.last_period_ns() }
    }

    /// Den Timer zurueckgeben (fuer den Start und fuer Tests).
    pub fn into_inner(self) -> T {
        self.timer
    }
}

impl<T: TickSource, F: FnMut()> Clock for TimerClock<T, F> {
    /// Die feine Zeit des Timers, fuer `took` und `drift` (7.3); die
    /// logische Zeit rechnet die Schleife aus der Tickzahl (7.1).
    fn now(&self) -> i64 {
        self.timer.now_ns()
    }

    /// Wartet auf das Tick-Ereignis, in dem `deadline` liegt.
    ///
    /// **Die Frist, nicht das naechste Ereignis.** Nach virtuellen Ticks
    /// (9.9) rueckt die Schleife ihre Frist um mehrere Perioden vor; wer
    /// dann nur auf das naechste Ereignis wartete, liefe der Zeit davon.
    /// Liegt die Frist schon zurueck, wird nicht mehr gewartet.
    fn wait_until(&mut self, deadline: i64) {
        let target = if self.nominal_ns > 0 { u64::try_from(deadline / self.nominal_ns).unwrap_or(0) } else { 0 };
        while self.timer.ticks() < target {
            self.timer.wait_event();
            (self.idle)();
        }
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

        fn now_ns(&self) -> i64 {
            self.count.get() as i64 * self.period_ns
        }

        fn wait_for_tick(&mut self) {
            self.waits.set(self.waits.get() + 1);
            self.count.set(self.count.get() + 1);
        }
    }

    const MS: i64 = 1_000_000;

    #[test]
    fn the_clock_reads_the_timer_finely() {
        let timer = FakeTimer { count: Cell::new(3), period_ns: MS + 5_000, ..FakeTimer::default() };
        let clock = TimerClock::new(timer, MS);
        assert_eq!(clock.now(), 3 * (MS + 5_000), "die Uhr ist die des Timers, nicht die nominale");
    }

    #[test]
    fn the_clock_waits_for_the_deadline() {
        let timer = FakeTimer { count: Cell::new(0), period_ns: MS, ..FakeTimer::default() };
        let mut clock = TimerClock::new(timer, MS);
        clock.wait_until(2 * MS);
        assert_eq!(clock.timer.waits.get(), 2, "zwei Ereignisse bis zur Frist");
    }

    /// Wer schon zu spaet ist, wartet nicht noch; den Rueckstand sieht die
    /// Schleife als `drift` (7.3), nicht die Uhr.
    #[test]
    fn a_deadline_in_the_past_does_not_wait() {
        let timer = FakeTimer { count: Cell::new(3), period_ns: MS, ..FakeTimer::default() };
        let mut clock = TimerClock::new(timer, MS);
        clock.wait_until(MS);
        assert_eq!(clock.timer.waits.get(), 0);
        assert_eq!(clock.now() - MS, 2 * MS, "zwei Perioden Rueckstand");
    }

    #[test]
    fn virtual_ticks_move_the_deadline_and_the_clock_follows() {
        let timer = FakeTimer { count: Cell::new(1), period_ns: MS, ..FakeTimer::default() };
        let mut clock = TimerClock::new(timer, MS);
        // Die Schleife schlief drei virtuelle Ticks: Frist von 2 auf 5 ms.
        clock.wait_until(5 * MS);
        assert_eq!(clock.timer.waits.get(), 4, "die Uhr wartet die geschlafenen Perioden wirklich ab");
    }
}
