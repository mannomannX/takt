//! Der Alarm der Interruptform (12.11) auf TIM2: freilaufend ueber 32 Bit
//! mit `TIMER_HZ`, der Vergleich CC1 auf die Frist, die `service` nennt,
//! und der Ueberlauf als Epoche der Zeitachse. Die Rechnung dazu steht in
//! `takt_board_support::alarm`; hier stehen die Register.
//!
//! TIM2 gehoert in dieser Form dem Alarm: Die ISR des Boards ruft
//! [`on_interrupt`], und ist die Frist erreicht, rechnet sie den Schritt.
//! Den Job-Interrupt waehlt das Bring-up: eine freie Leitung niedrigster
//! Prioritaet, die nur von Hand ansteht.

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::interrupt::Mutex;
use cortex_m::peripheral::NVIC;
use stm32f4::stm32f401::{Interrupt, TIM2};
use takt_board_support::alarm::{Compare, compare, counts, counts_at, ns, passed};

/// Wie oft TIM2 uebergelaufen ist; die ISR zaehlt.
static EPOCH: AtomicU32 = AtomicU32::new(0);

/// Der Zielstand des Alarms auf der 64-Bit-Zeitachse; `None` ohne Alarm.
/// 64 Bit hat der Cortex-M4 nicht atomar: ein kritischer Abschnitt.
static TARGET: Mutex<Cell<Option<u64>>> = Mutex::new(Cell::new(None));

/// TIM2 als freilaufender Zaehler mit `prescaler`: von 0 bis `u32::MAX`,
/// der Ueberlauf meldet sich, der Vergleich erst, wenn ein Alarm steht.
pub(crate) fn start(rcc: &stm32f4::stm32f401::RCC, tim2: &TIM2, prescaler: u16) {
    rcc.apb1enr().modify(|_, w| w.tim2en().set_bit());
    let _ = rcc.apb1enr().read();
    tim2.psc().write(|w| unsafe { w.psc().bits(prescaler) });
    tim2.arr().write(|w| unsafe { w.bits(u32::MAX) });
    // Uebernehmen und das dabei gesetzte `UIF` wieder loeschen, wie beim
    // periodischen Tick (`start_tim2`): Der erste Ueberlauf kommt vom Zaehler.
    tim2.egr().write(|w| w.ug().set_bit());
    tim2.sr().write(|w| unsafe { w.bits(0) });
    EPOCH.store(0, Ordering::Relaxed);
    tim2.dier().write(|w| w.uie().set_bit());
    tim2.cr1().modify(|_, w| w.cen().set_bit());
}

fn tim2() -> &'static stm32f4::stm32f401::tim2::RegisterBlock {
    // SAFETY: TIM2 gehoert in der Interruptform dem Alarm; gelesen wird der
    // Zaehler, geschrieben nur `CCR1`, `DIER` und `SR`.
    unsafe { &*TIM2::ptr() }
}

/// Der Stand der Zeitachse: Epoche und Zaehler, gelesen, bis die Epoche
/// stehen bleibt; ein Ueberlauf, dessen Interrupt noch ansteht, zaehlt mit.
pub fn now_counts() -> u64 {
    let t = tim2();
    loop {
        let epoch = EPOCH.load(Ordering::Relaxed);
        let low = t.cnt().read().bits();
        let wrapped = t.sr().read().uif().bit_is_set();
        if EPOCH.load(Ordering::Relaxed) == epoch {
            return counts(epoch, low, wrapped);
        }
    }
}

/// Stellt den Vergleich auf `target`: in dieser Epoche ueber `CCR1`, hinter
/// dem naechsten Ueberlauf gar nicht — dessen Interrupt fragt erneut —, und
/// eine Frist, die schon da ist oder beim Stellen verstrich, laesst die ISR
/// sofort anstehen.
fn aim(target: u64) {
    let t = tim2();
    match compare(target, now_counts()) {
        Compare::Due => NVIC::pend(Interrupt::TIM2),
        Compare::At(low) => {
            t.ccr1().write(|w| unsafe { w.bits(low) });
            t.sr().write(|w| unsafe { w.bits(!(1 << 1)) });
            t.dier().modify(|_, w| w.cc1ie().set_bit());
            if passed(target, now_counts()) {
                NVIC::pend(Interrupt::TIM2);
            }
        }
        Compare::Later => {
            t.dier().modify(|_, w| w.cc1ie().clear_bit());
        }
    }
}

/// Aus der ISR von TIM2: zaehlt einen Ueberlauf, loescht den Vergleich und
/// sagt, ob die Frist erreicht ist. Ist sie es, steht danach kein Alarm
/// mehr; der Schritt stellt den naechsten.
pub fn on_interrupt() -> bool {
    let t = tim2();
    let sr = t.sr().read();
    if sr.uif().bit_is_set() {
        // `rc_w0`: Nullen loeschen, Einsen lassen stehen.
        t.sr().write(|w| unsafe { w.bits(!1) });
        EPOCH.fetch_add(1, Ordering::Relaxed);
    }
    if sr.cc1if().bit_is_set() {
        t.sr().write(|w| unsafe { w.bits(!(1 << 1)) });
    }
    let target = cortex_m::interrupt::free(|cs| TARGET.borrow(cs).get());
    match target {
        Some(target) if passed(target, now_counts()) => {
            cortex_m::interrupt::free(|cs| TARGET.borrow(cs).set(None));
            t.dier().modify(|_, w| w.cc1ie().clear_bit());
            true
        }
        // Nach einem Ueberlauf liegt das Ziel vielleicht in dieser Epoche.
        Some(target) => {
            aim(target);
            false
        }
        None => false,
    }
}

/// Der Alarm des Boards fuer `takt_rt_baremetal::interrupt` und die Uhr
/// der Interruptform: die Zeitachse in Nanosekunden seit dem Start.
#[derive(Clone, Copy, Debug)]
pub struct Tim2Alarm {
    timer_hz: u32,
    /// Die Leitung des Job-Interrupts.
    jobs: Interrupt,
}

impl Tim2Alarm {
    /// Der Alarm auf TIM2 mit `timer_hz`; Jobs laufen im Interrupt `jobs`.
    pub fn new(timer_hz: u32, jobs: Interrupt) -> Tim2Alarm {
        Tim2Alarm { timer_hz, jobs }
    }

    /// Jetzt, in Nanosekunden seit dem Start des Zaehlers.
    pub fn now_ns(&self) -> i64 {
        ns(self.timer_hz, now_counts())
    }
}

impl takt_rt_baremetal::interrupt::Alarm for Tim2Alarm {
    fn arm(&mut self, at: i64) {
        let target = counts_at(self.timer_hz, at);
        cortex_m::interrupt::free(|cs| TARGET.borrow(cs).set(Some(target)));
        aim(target);
    }

    fn pend_jobs(&mut self) {
        NVIC::pend(self.jobs);
    }
}

impl takt_rt_core::Clock for Tim2Alarm {
    fn now(&self) -> i64 {
        self.now_ns()
    }

    /// Die Interruptform wartet nie: Den naechsten Schritt bringt der
    /// Alarm. Wer trotzdem wartet, wartet hier auf die Zeitachse.
    fn wait_until(&mut self, deadline: i64) {
        while self.now_ns() < deadline {
            cortex_m::asm::nop();
        }
    }
}
