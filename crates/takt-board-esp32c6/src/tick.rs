//! Tick aus dem SYSTIMER-Alarm (7.1, 12.3); die gemessene Periode in
//! Schritten desselben Zaehlers.
//!
//! Der Zaehler gehoert in die ISR, nicht in die Schleife: Wer in der
//! Hauptschleife zaehlt, verliert jeden Tick, den ein zu langer Schritt
//! ueberdeckt — und damit die Information, die den Overrun belegt.

use core::sync::atomic::{AtomicU32, Ordering};

use takt_board_support::Counter64;
use takt_rt_baremetal::TickSource;

static TICKS: Counter64 = Counter64::new();
static LAST_COUNTS: AtomicU32 = AtomicU32::new(0);

/// Von der Alarm-ISR gerufen: zaehlt den Tick und merkt sich die
/// gemessene Periode in SYSTIMER-Schritten.
pub fn on_timer_interrupt(elapsed_counts: u32) {
    TICKS.tick();
    LAST_COUNTS.store(elapsed_counts, Ordering::Relaxed);
}

/// Tick-Ereignisse seit dem Start.
pub fn count() -> u64 {
    TICKS.get()
}

/// Der SYSTIMER als Tick-Quelle: nominale und gemessene Periode in
/// Zaehlschritten.
#[derive(Clone, Copy, Debug)]
pub struct SystimerTick {
    counts_per_tick: u32,
    timer_hz: u32,
}

impl SystimerTick {
    /// Eine Tick-Quelle mit `counts_per_tick` Schritten eines Zaehlers
    /// von `timer_hz`.
    pub fn new(timer_hz: u32, counts_per_tick: u32) -> SystimerTick {
        SystimerTick { counts_per_tick, timer_hz }
    }

    /// Die nominale Periode in Nanosekunden.
    pub fn nominal_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, self.counts_per_tick)
    }
}

impl TickSource for SystimerTick {
    fn ticks(&self) -> u64 {
        count()
    }

    fn last_period_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, LAST_COUNTS.load(Ordering::Relaxed))
    }

    fn wait_for_tick(&mut self) {
        let start = count();
        while count() == start {
            crate::wfi();
        }
    }
}
