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

/// Der Abstand der letzten zwei Interrupts, in Kernzyklen.
///
/// **Warum Kernzyklen und nicht Timer-Schritte.** Der Timerzaehler steht
/// beim Update-Interrupt immer nahe null — er ist gerade uebergelaufen,
/// und sein Stand sagt nichts ueber den Abstand. Was die Periode wirklich
/// misst, ist der Zyklenzaehler des Kerns (DWT): Er laeuft unabhaengig
/// vom Timer, mit `CORE_HZ`, und der Abstand zweier Ablesungen *ist* die
/// Periode. Ein Timer, der sich selbst misst, koennte seine eigene Drift
/// nicht sehen.
static LAST_CYCLES: AtomicU32 = AtomicU32::new(0);

/// Vom Interrupt-Handler des Boards zu rufen.
///
/// `elapsed_cycles` ist der Abstand zum vorigen Interrupt in Kernzyklen,
/// gemessen am DWT. Die Funktion steht hier und nicht im
/// `#[interrupt]`-Rumpf, weil das Attribut zum Board-Programm gehoert und
/// nicht in eine Bibliothek: Ein Crate, das einen Interrupt-Vektor
/// belegt, laesst sich nicht zweimal in dasselbe Programm binden.
pub fn on_timer_interrupt(elapsed_cycles: u32) {
    TICKS.tick();
    LAST_CYCLES.store(elapsed_cycles, Ordering::Relaxed);
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
    /// Die Frequenz des Kerns — sie deutet [`LAST_CYCLES`].
    core_hz: u32,
}

impl Tim2Tick {
    /// Bindet die Tickquelle an die Konfiguration des Timers.
    pub fn new(timer_hz: u32, counts_per_tick: u32, core_hz: u32) -> Tim2Tick {
        Tim2Tick { counts_per_tick, timer_hz, core_hz }
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
        // Die Messung steht in Kernzyklen; `Measurement` rechnet sie um —
        // dieselbe Rechnung, die auch `takt bench` benutzen wird (13.8).
        takt_board_support::Measurement { cycles: LAST_CYCLES.load(Ordering::Relaxed), core_hz: self.core_hz }.ns()
    }

    /// Ticks mal Schritte je Tick plus der Stand von TIM2 innerhalb des
    /// laufenden Ticks; gelesen, bis Tickzahl und Zaehler zusammenpassen,
    /// weil der Ueberlauf dazwischen kommen kann.
    fn now_ns(&self) -> i64 {
        // SAFETY: `CNT` wird nur gelesen; das Register gehoert dem Board.
        let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };
        let counts = loop {
            let before = count();
            let cnt = u64::from(tim2.cnt().read().bits());
            if count() == before {
                break before * u64::from(self.counts_per_tick) + cnt;
            }
        };
        takt_board_support::clock::elapsed_ns(self.timer_hz, counts)
    }

    /// Ein `wfi`, ausser der Tick ist schon da; geprueft bei gesperrten
    /// Interrupts (FB-296). Der Tick weckt, aber auch die Leitung, die
    /// zwischen den Ticks ihren FIFO leert und nachgefuellt werden will.
    fn wait_event(&mut self, target: u64) {
        cortex_m::interrupt::free(|_| {
            if count() < target {
                cortex_m::asm::wfi();
            }
        });
    }
}
