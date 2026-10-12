//! Der Basis-Tick aus TIM2 (7.1, 12.3).
//!
//! 12.3 gibt das Muster vor: „Tick aus Hardware-Timer-Interrupt; die ISR
//! setzt ein Flag und sampelt ggf. zeitkritische Inputs; die Hauptschleife
//! fuehrt den Tick aus. Ist das Flag beim naechsten Interrupt noch gesetzt
//! → `Runtime(Overrun)`."
//!
//! **Der Zaehler ist die Zeit, der Interrupt nur das Wecken** (FB-742, wie
//! FB-198 auf dem C6). TIM2 laeuft frei als Zeitachse, dieselbe wie in der
//! Interruptform ([`crate::alarm`]), und ihr Vergleich weckt den Kern an
//! jeder Tickgrenze. Die Zahl der Ticks kommt aus dem Stand der Zeitachse,
//! nicht aus den Aufrufen der ISR: Steht der Kern laenger als eine Periode —
//! eine Sektorloeschung des Journals haelt ihn 197 ms an, denn jeder
//! Befehlsabruf wartet auf das Flash —, faellt danach nur *eine* ISR an, und
//! ein Zaehler in ihr verloere die Perioden dazwischen. So aber sieht die
//! Schleife den Rueckstand als `drift` und holt jede Grenze nach (7.3).
//!
//! **Warum TIM2 und nicht SysTick.** SysTick liegt im Kern und waere
//! portabler, hat aber nur 24 Bit und keine Compare-Kanaele. TIM2 ist auf
//! dem F401 32-bittig und traegt vier Compare-Einheiten — dieselbe
//! Peripherie, die spaeter die geplanten Ausgaben stellt (`at`, 7.5). Ein
//! Zeitgeber fuer beides ist besser als zwei, die gegeneinander driften.
//!
//! Die Rechnung selbst steht in `takt_board_support::alarm`, wo sie am Wirt
//! geprueft ist; hier stehen nur die Register.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::interrupt::Mutex;
use takt_board_support::alarm::{next_boundary, ticks_since};
use takt_rt_baremetal::TickSource;

use crate::alarm;

/// Timer-Schritte je Tick, von [`start`] gesetzt.
static COUNTS_PER_TICK: AtomicU32 = AtomicU32::new(0);

/// Der Abstand der Weckgrenzen in Timer-Schritten: ein Tick, solange das
/// Pruefgeraet ihn nicht streckt ([`stretch`]).
static PERIOD: AtomicU32 = AtomicU32::new(0);

/// Der Ursprung des Rasters und die naechste Weckgrenze auf der
/// 64-Bit-Zeitachse. 64 Bit hat der Cortex-M4 nicht atomar: ein kritischer
/// Abschnitt.
static ORIGIN: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));
static WAKE: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));

/// Der Abstand der letzten zwei Weckgrenzen, in Kernzyklen.
///
/// **Warum Kernzyklen und nicht Timer-Schritte.** Was die Periode wirklich
/// misst, ist der Zyklenzaehler des Kerns (DWT): Er laeuft unabhaengig vom
/// Timer, mit `CORE_HZ`, und der Abstand zweier Ablesungen *ist* die
/// Periode. Ein Timer, der sich selbst misst, koennte seine eigene Drift
/// nicht sehen.
static LAST_CYCLES: AtomicU32 = AtomicU32::new(0);

/// Der DWT-Stand an der vorigen Weckgrenze.
static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Startet TIM2 als Zeitachse und den Wecker auf die erste Grenze: Das
/// Raster beginnt beim Stand, den der Start liest (FB-352 auf dem C6), so
/// liegt jede Grenze genau auf ihrem Wecken.
pub(crate) fn start(rcc: &stm32f4::stm32f401::RCC, tim2: &stm32f4::stm32f401::TIM2, prescaler: u16, counts: u32) {
    COUNTS_PER_TICK.store(counts, Ordering::SeqCst);
    PERIOD.store(counts, Ordering::SeqCst);
    alarm::start(rcc, tim2, prescaler);
    let origin = alarm::now_counts();
    let first = origin.saturating_add(u64::from(counts));
    cortex_m::interrupt::free(|cs| {
        ORIGIN.borrow(cs).set(origin);
        WAKE.borrow(cs).set(first);
    });
    alarm::set_target(Some(first));
}

/// Streckt den Abstand der Weckgrenzen um `percent` Prozent, mit null
/// wieder auf die nominale — das Pruefgeraet fuer 12.6 Zeile 7.
///
/// Einen Zeitgeber, der seine Periode verfehlt, sieht nur die Messung der
/// ISR; ein Boardtest braucht einen, der es auf Verlangen tut. Es gilt ab
/// der naechsten Grenze, und der Wecker laeuft von seiner letzten weiter
/// (wie auf dem C6). Die Zahl der Ticks bleibt am Raster der Zeitachse.
pub fn stretch(percent: u32) {
    let counts = u64::from(COUNTS_PER_TICK.load(Ordering::SeqCst));
    let stretched = counts.saturating_add(counts.saturating_mul(u64::from(percent)) / 100);
    PERIOD.store(u32::try_from(stretched).unwrap_or(u32::MAX), Ordering::SeqCst);
}

/// Vom Interrupt-Handler `TIM2` des Programms zu rufen: zaehlt einen
/// Ueberlauf der Zeitachse; an einer Weckgrenze misst es ihren Abstand zur
/// vorigen am DWT, stellt die naechste, und ein Job, der gerade rechnet,
/// gibt den Kern an die Hauptschleife (4.5).
///
/// Die Funktion steht hier und nicht im `#[interrupt]`-Rumpf, weil das
/// Attribut zum Board-Programm gehoert und nicht in eine Bibliothek: Ein
/// Crate, das einen Interrupt-Vektor belegt, laesst sich nicht zweimal in
/// dasselbe Programm binden.
pub fn on_interrupt() {
    if !alarm::on_interrupt() {
        return;
    }
    let now = crate::cycles::now();
    LAST_CYCLES.store(now.wrapping_sub(LAST_STAMP.swap(now, Ordering::SeqCst)), Ordering::SeqCst);
    let period = PERIOD.load(Ordering::SeqCst);
    let next = cortex_m::interrupt::free(|cs| {
        let wake = WAKE.borrow(cs);
        wake.set(next_boundary(wake.get(), period, alarm::now_counts()));
        wake.get()
    });
    alarm::set_target(Some(next));
    crate::jobs::preempt();
}

/// Timer-Schritte seit dem Ursprung des Rasters.
fn since_origin() -> u64 {
    let origin = cortex_m::interrupt::free(|cs| ORIGIN.borrow(cs).get());
    alarm::now_counts().saturating_sub(origin)
}

/// Die Ticks seit dem Start, am Stand der Zeitachse.
pub fn count() -> u64 {
    ticks_since(0, COUNTS_PER_TICK.load(Ordering::SeqCst), since_origin())
}

/// Die Tickquelle des Boards: TIM2 als freilaufende Zeitachse mit einem
/// Wecker an jeder Grenze.
///
/// Sie haelt keinen veraenderlichen Zustand — Zeitachse und Wecker stehen
/// in den `static`s, die die ISR fuehrt. Was hier liegt, ist die
/// Umrechnung von Timer-Schritten in Zeit.
#[derive(Clone, Copy, Debug)]
pub struct Tim2Tick {
    /// Timer-Schritte je Basis-Tick.
    counts_per_tick: u32,
    /// Die Frequenz, mit der der Timer zaehlt.
    timer_hz: u32,
    /// Ihr Massstab, einmal gerechnet: Die Uhr wird im Tick mehrmals gelesen.
    scale: takt_board_support::clock::Scale,
    /// Die Frequenz des Kerns — sie deutet [`LAST_CYCLES`].
    core_hz: u32,
}

impl Tim2Tick {
    /// Bindet die Tickquelle an die Konfiguration des Timers.
    pub fn new(timer_hz: u32, counts_per_tick: u32, core_hz: u32) -> Tim2Tick {
        Tim2Tick { counts_per_tick, timer_hz, scale: takt_board_support::clock::Scale::of(timer_hz), core_hz }
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
        // dieselbe Rechnung, die auch `takt bench` benutzt (13.8).
        takt_board_support::Measurement { cycles: LAST_CYCLES.load(Ordering::SeqCst), core_hz: self.core_hz }.ns()
    }

    /// Die Zeitachse seit dem Ursprung des Rasters: dasselbe Raster wie
    /// [`TickSource::ticks`], denn die Uhr rechnet ihr Ziel als `Frist /
    /// Periode`.
    fn now_ns(&self) -> i64 {
        self.scale.ns(since_origin())
    }

    /// Ein `wfi`, ausser der Tick ist schon da; geprueft bei gesperrten
    /// Interrupts (FB-296). Der Wecker weckt an der Grenze, aber auch die
    /// Leitung, die zwischen den Ticks ihren FIFO leert und nachgefuellt
    /// werden will.
    fn wait_event(&mut self, target: u64) {
        cortex_m::interrupt::free(|_| {
            if count() < target {
                cortex_m::asm::wfi();
            }
        });
    }
}
