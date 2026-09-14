//! Der Basis-Tick aus TIM2 (7.1, 12.3).
//!
//! 12.3 gibt das Muster vor: „Tick aus Hardware-Timer-Interrupt; die ISR
//! setzt ein Flag und sampelt ggf. zeitkritische Inputs; die Hauptschleife
//! fuehrt den Tick aus. Ist das Flag beim naechsten Interrupt noch
//! gesetzt → `Runtime(Overrun)`."
//!
//! **Warum TIM2 und nicht SysTick.** SysTick liegt im Kern und waere
//! portabler, hat aber nur 24 Bit und keine Compare-Kanaele. TIM2 ist auf
//! dem F401 32-bittig und traegt vier Compare-Einheiten — dieselbe
//! Peripherie, die spaeter die geplanten Ausgaben stellt (`at`, 7.5). Ein
//! Zeitgeber fuer beides ist besser als zwei, die gegeneinander driften.
//!
//! Die Zaehlerlogik selbst steht in `takt-board-support`: Sie ist reine
//! Rechnung, hat einen Ueberlauf, der auf Hardware nach 49 Tagen eintritt,
//! und gehoert darum dorthin, wo sie getestet wird.

use core::sync::atomic::{AtomicU32, Ordering};

use takt_board_support::Counter64;
use takt_rt_baremetal::TickSource;

/// Wie oft die ISR gefeuert hat, seit dem Start.
static TICKS: Counter64 = Counter64::new();

/// Die zuletzt gemessene Periode in Timer-Schritten.
///
/// Sie entsteht aus dem Zaehlerstand beim Interrupt: Ein Timer, der bei
/// `arr` ueberlaeuft, sollte genau `arr + 1` gezaehlt haben. Weicht es
/// ab, driftet die Uhr — das prueft `tick_tolerance` (7.1).
static LAST_PERIOD: AtomicU32 = AtomicU32::new(0);

/// Vom Interrupt-Handler des Boards zu rufen.
///
/// Sie steht hier und nicht im `#[interrupt]`-Rumpf, weil das Attribut
/// zum Board-Programm gehoert und nicht in eine Bibliothek: Ein Crate,
/// das einen Interrupt-Vektor belegt, laesst sich nicht zweimal in
/// dasselbe Programm binden.
pub fn on_timer_interrupt(measured_counts: u32) {
    TICKS.tick();
    LAST_PERIOD.store(measured_counts, Ordering::Relaxed);
}

/// Der Tickzaehler.
pub fn count() -> u64 {
    TICKS.get()
}

/// Die Tickquelle des Boards: TIM2 im Aufwaertszaehlbetrieb.
///
/// Sie haelt keinen veraenderlichen Zustand — der Zaehler steht in den
/// `static`s, die die ISR fuehrt. Was hier liegt, ist die Umrechnung von
/// Timer-Schritten in Zeit.
#[derive(Clone, Copy, Debug)]
pub struct Tim2Tick {
    /// Timer-Schritte je Basis-Tick.
    counts_per_tick: u32,
    /// Die Frequenz, mit der der Timer zaehlt.
    timer_hz: u32,
}

impl Tim2Tick {
    /// Bindet die Tickquelle an die Konfiguration des Timers.
    pub fn new(timer_hz: u32, counts_per_tick: u32) -> Tim2Tick {
        Tim2Tick { counts_per_tick, timer_hz }
    }

    /// Die nominale Periode in Nanosekunden aus der Timerkonfiguration.
    ///
    /// Sie dient dem Abgleich mit `system: tick`: Weichen beide ab, ist
    /// der Timer falsch konfiguriert, und das faellt beim Start auf statt
    /// spaeter als Drift.
    pub fn nominal_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, self.counts_per_tick)
    }
}

impl TickSource for Tim2Tick {
    fn ticks(&self) -> u64 {
        count()
    }

    fn last_period_ns(&self) -> i64 {
        takt_board_support::clock::period_ns(self.timer_hz, LAST_PERIOD.load(Ordering::Relaxed))
    }

    fn wait_for_tick(&mut self) {
        let start = count();
        // `wfi` statt Warteschleife: Ein Kern, der zwischen den Ticks
        // rechnet, verbraucht Strom fuer nichts und heizt die Messung auf
        // (12.3). Der Timer-Interrupt weckt ihn.
        while count() == start {
            cortex_m::asm::wfi();
        }
    }
}
