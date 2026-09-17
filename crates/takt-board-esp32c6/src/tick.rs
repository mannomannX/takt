//! Tick aus dem SYSTIMER (7.1, 12.3).
//!
//! **Der Zaehler ist die Zeit, der Interrupt nur das Wecken.** Die Zahl
//! der Ticks kommt aus dem Stand des SYSTIMER, nicht aus einem Zaehler in
//! der ISR: Steht der Kern laenger als eine Periode — eine Sektorloeschung
//! im Flash sperrt Interrupts fuer zweistellige Millisekunden —, faellt
//! nur *ein* Alarm an, und ein ISR-Zaehler verloere die Ticks dazwischen
//! still. Der Timer verliert sie nicht.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::timer::systimer::{SystemTimer, Unit};
use takt_rt_baremetal::TickSource;

static COUNTS_PER_TICK: AtomicU32 = AtomicU32::new(0);
static LAST_COUNTS: AtomicU32 = AtomicU32::new(0);

/// Von `init` gesetzt: so viele SYSTIMER-Schritte ist ein Tick lang.
pub(crate) fn set_counts_per_tick(counts: u32) {
    COUNTS_PER_TICK.store(counts, Ordering::Relaxed);
}

/// Von der Alarm-ISR gerufen: merkt sich die gemessene Periode in
/// SYSTIMER-Schritten.
#[esp_hal::ram]
pub fn on_timer_interrupt(elapsed_counts: u32) {
    LAST_COUNTS.store(elapsed_counts, Ordering::Relaxed);
}

/// Tick-Ereignisse seit dem Start des SYSTIMER.
///
/// Im RAM (12.3): Ein Flash-Schreibvorgang schaltet den Cache ab, und der
/// Zaehler wird waehrenddessen gelesen.
#[esp_hal::ram]
pub fn count() -> u64 {
    match u64::from(COUNTS_PER_TICK.load(Ordering::Relaxed)) {
        0 => 0,
        counts => SystemTimer::unit_value(Unit::Unit0) / counts,
    }
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
    #[esp_hal::ram]
    fn ticks(&self) -> u64 {
        count()
    }

    #[esp_hal::ram]
    fn last_period_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, LAST_COUNTS.load(Ordering::Relaxed))
    }

    /// Im RAM, weil sie waehrend eines Flash-Schreibvorgangs laeuft (12.3).
    #[esp_hal::ram]
    fn wait_for_tick(&mut self) {
        let start = count();
        while count() == start {
            crate::wfi();
        }
    }
}
