//! Der Alarm der Interruptform (12.11) auf dem SYSTIMER: Einheit 0 ist mit
//! 52 Bit bei `TIMER_HZ` die Zeitachse des Laufs — sie laeuft in neun
//! Jahren nicht ueber, Epochen braucht es nicht —, und Vergleicher 0 steht
//! im Zielbetrieb auf der Frist, die `service` nennt. Jobs laufen im
//! Software-Interrupt `FROM_CPU_INTR1`; `FROM_CPU_INTR0` gehoert dem
//! Job-Faden des eigenen Kerns.
//!
//! Den Interrupt des Vergleichers und den der Jobs bindet das Bring-up mit
//! seinen Prioritaeten; die ISR des Alarms ruft [`on_interrupt`], die der
//! Jobs [`jobs_taken`].

use core::cell::{Cell, RefCell};

use critical_section::Mutex;
use esp_hal::interrupt::InterruptHandler;
use esp_hal::interrupt::software::SoftwareInterrupt;
use esp_hal::peripherals::SYSTIMER;
use esp_hal::timer::systimer::{SystemTimer, Unit};
use takt_board_support::alarm::{counts_at, ns, passed};

use crate::TIMER_HZ;

/// So weit liegt ein Ziel mindestens vor dem Zaehler: Ein Ziel, das beim
/// Stellen schon erreicht ist, loest nicht sicher aus. Zwei Mikrosekunden,
/// wie `esp_timer` aus ESP-IDF sie nimmt.
const MARGIN: u64 = 2 * TIMER_HZ as u64 / 1_000_000;

/// Der Stand der Einheit beim Start: Die Zeitachse zaehlt ab hier.
static ORIGIN: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));

/// Das Ziel des Alarms auf der Zeitachse; `None` ohne Alarm.
static TARGET: Mutex<Cell<Option<u64>>> = Mutex::new(Cell::new(None));

/// Der Software-Interrupt der Jobs.
static JOBS: Mutex<RefCell<Option<SoftwareInterrupt<'static, 1>>>> = Mutex::new(RefCell::new(None));

/// Vergleicher 0 im Zielbetrieb auf Einheit 0, sein Interrupt an
/// `alarm_handler`; die Jobs an `jobs_handler`. Noch steht kein Ziel.
pub(crate) fn start(
    systimer: SYSTIMER<'static>,
    alarm_handler: InterruptHandler,
    mut jobs: SoftwareInterrupt<'static, 1>,
    jobs_handler: InterruptHandler,
) {
    let alarm = SystemTimer::new(systimer).alarm0;
    let regs = SYSTIMER::regs();
    regs.conf().modify(|_, w| w.target0_work_en().clear_bit());
    regs.target_conf(0).modify(|_, w| w.period_mode().clear_bit().timer_unit_sel().clear_bit());
    regs.int_clr().write(|w| w.target(0).clear_bit_by_one());
    esp_hal::timer::Timer::set_interrupt_handler(&alarm, alarm_handler);
    regs.int_ena().modify(|_, w| w.target(0).set_bit());
    jobs.set_interrupt_handler(jobs_handler);
    critical_section::with(|cs| {
        ORIGIN.borrow(cs).set(SystemTimer::unit_value(Unit::Unit0));
        TARGET.borrow(cs).set(None);
        JOBS.borrow_ref_mut(cs).replace(jobs);
    });
}

/// Der Stand der Zeitachse.
#[esp_hal::ram]
fn now_counts() -> u64 {
    let origin = critical_section::with(|cs| ORIGIN.borrow(cs).get());
    SystemTimer::unit_value(Unit::Unit0).wrapping_sub(origin)
}

/// Stellt Vergleicher 0 auf `target`, mindestens [`MARGIN`] vor den
/// Zaehler: Eine Frist, die schon da ist, loest so gleich aus.
#[esp_hal::ram]
fn aim(target: u64) {
    let regs = SYSTIMER::regs();
    let origin = critical_section::with(|cs| ORIGIN.borrow(cs).get());
    loop {
        let at = origin.wrapping_add(target).max(SystemTimer::unit_value(Unit::Unit0) + MARGIN);
        regs.conf().modify(|_, w| w.target0_work_en().clear_bit());
        regs.trgt(0).hi().write(|w| w.hi().set((at >> 32) as u32));
        regs.trgt(0).lo().write(|w| w.lo().set((at & 0xFFFF_FFFF) as u32));
        regs.comp_load(0).write(|w| w.load().set_bit());
        regs.conf().modify(|_, w| w.target0_work_en().set_bit());
        if SystemTimer::unit_value(Unit::Unit0) < at {
            return;
        }
    }
}

/// Aus der ISR des Vergleichers: quittiert und sagt, ob die Frist erreicht
/// ist. Ist sie es, steht danach kein Alarm mehr; der Schritt stellt den
/// naechsten.
#[esp_hal::ram]
pub fn on_interrupt() -> bool {
    SYSTIMER::regs().int_clr().write(|w| w.target(0).clear_bit_by_one());
    critical_section::with(|cs| {
        let target = TARGET.borrow(cs);
        match target.get() {
            Some(t) if passed(t, now_counts()) => {
                target.set(None);
                true
            }
            Some(t) => {
                aim(t);
                false
            }
            None => false,
        }
    })
}

/// Aus der ISR der Jobs, vor ihrem Auftrag: nimmt den Software-Interrupt
/// zurueck, der sonst gleich wieder anstuende.
#[esp_hal::ram]
pub fn jobs_taken() {
    critical_section::with(|cs| {
        if let Some(jobs) = JOBS.borrow_ref(cs).as_ref() {
            jobs.reset();
        }
    });
}

/// Der Alarm des Boards fuer `takt_rt_baremetal::interrupt` und die Uhr
/// der Interruptform: die Zeitachse in Nanosekunden seit dem Start.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystimerAlarm;

impl SystimerAlarm {
    /// Jetzt, in Nanosekunden seit dem Start der Zeitachse.
    #[esp_hal::ram]
    pub fn now_ns(&self) -> i64 {
        ns(TIMER_HZ, now_counts())
    }
}

impl takt_rt_baremetal::interrupt::Alarm for SystimerAlarm {
    #[esp_hal::ram]
    fn arm(&mut self, at: i64) {
        let target = counts_at(TIMER_HZ, at);
        critical_section::with(|cs| {
            TARGET.borrow(cs).set(Some(target));
            aim(target);
        });
    }

    #[esp_hal::ram]
    fn pend_jobs(&mut self) {
        critical_section::with(|cs| {
            if let Some(jobs) = JOBS.borrow_ref(cs).as_ref() {
                jobs.raise();
            }
        });
    }
}

impl takt_rt_core::Clock for SystimerAlarm {
    #[esp_hal::ram]
    fn now(&self) -> i64 {
        self.now_ns()
    }

    /// Die Interruptform wartet nie: Den naechsten Schritt bringt der
    /// Alarm. Wer trotzdem wartet, wartet hier auf die Zeitachse.
    #[esp_hal::ram]
    fn wait_until(&mut self, deadline: i64) {
        while self.now_ns() < deadline {
            core::hint::spin_loop();
        }
    }
}
