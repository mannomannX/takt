//! Tick aus dem SYSTIMER (7.1, 12.3).
//!
//! **Der Zaehler ist die Zeit, der Interrupt nur das Wecken.** Die Zahl
//! der Ticks kommt aus dem Stand des SYSTIMER, nicht aus einem Zaehler in
//! der ISR: Steht der Kern laenger als eine Periode — eine Sektorloeschung
//! im Flash sperrt Interrupts fuer zweistellige Millisekunden —, faellt
//! nur *ein* Alarm an, und ein ISR-Zaehler verloere die Ticks dazwischen
//! still. Der Timer verliert sie nicht.
//!
//! **Das Raster beginnt, wo der Alarm beginnt** (FB-352). Gezaehlt wird ab
//! dem Stand, den `init` vor dem Laden des Alarms liest, nicht ab null.
//! Mit null als Ursprung lag der Alarm in beliebiger Phase zum Raster; fiel
//! er knapp vor eine Grenze, weckte er den Kern bei noch altem Zaehler, der
//! Kern schlief eine Periode weiter, und die Ticks kamen paarweise alle
//! zwei Perioden. So liegt jede Grenze am oder vor ihrem Alarm.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use critical_section::Mutex;
use esp_hal::timer::systimer::{SystemTimer, Unit};
use takt_rt_baremetal::TickSource;

static COUNTS_PER_TICK: AtomicU32 = AtomicU32::new(0);
static LAST_COUNTS: AtomicU32 = AtomicU32::new(0);
static ORIGIN: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));

/// Von `init` gesetzt: so viele SYSTIMER-Schritte ist ein Tick lang.
pub(crate) fn set_counts_per_tick(counts: u32) {
    COUNTS_PER_TICK.store(counts, Ordering::Relaxed);
}

/// Von `init` gesetzt, bevor der Alarm laeuft: der Ursprung des Rasters.
pub(crate) fn set_origin(stamp: u64) {
    critical_section::with(|cs| ORIGIN.borrow(cs).set(stamp));
}

/// SYSTIMER-Schritte seit dem Ursprung; im RAM wie seine Aufrufer.
#[esp_hal::ram]
fn since_origin() -> u64 {
    let origin = critical_section::with(|cs| ORIGIN.borrow(cs).get());
    SystemTimer::unit_value(Unit::Unit0).wrapping_sub(origin)
}

/// Von der Alarm-ISR gerufen: merkt sich die gemessene Periode in
/// SYSTIMER-Schritten.
#[esp_hal::ram]
pub fn on_timer_interrupt(elapsed_counts: u32) {
    LAST_COUNTS.store(elapsed_counts, Ordering::Relaxed);
    // 4.5: Ein Job, der gerade rechnet, gibt den Kern an die Hauptschleife.
    crate::jobs::preempt();
}

/// Tick-Ereignisse seit dem Ursprung des Rasters.
///
/// Im RAM (12.3): Ein Flash-Schreibvorgang schaltet den Cache ab, und der
/// Zaehler wird waehrenddessen gelesen.
#[esp_hal::ram]
pub fn count() -> u64 {
    match u64::from(COUNTS_PER_TICK.load(Ordering::Relaxed)) {
        0 => 0,
        counts => since_origin() / counts,
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
    fn now_ns(&self) -> i64 {
        // Dasselbe Raster wie `ticks`: Die Uhr rechnet ihr Ziel als
        // `Frist / Periode`.
        takt_board_support::clock::elapsed_ns(self.timer_hz, since_origin())
    }

    #[esp_hal::ram]
    fn last_period_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, LAST_COUNTS.load(Ordering::Relaxed))
    }

    /// Ein `wfi`, ausser der Tick ist schon da; geprueft bei gesperrten
    /// Interrupts (FB-296) — `wfi` weckt auch dann, und die ISR laeuft
    /// danach. Im RAM, weil sie waehrend eines Flash-Schreibvorgangs laeuft
    /// (12.3).
    #[esp_hal::ram]
    fn wait_event(&mut self, target: u64) {
        critical_section::with(|_| {
            if count() < target {
                crate::wfi();
            }
        });
    }
}
