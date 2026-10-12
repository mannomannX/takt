//! Der Zeittreiber von `embassy-time` (Form `rtos` unter Embassy, 12.11;
//! M11 Schritt 14d) auf der Zeitachse der Interruptform: TIM2 freilaufend
//! mit `TIMER_HZ`, ein Schritt je Tick von `embassy-time`, der Vergleich CC1
//! auf den naechsten Weckzeitpunkt ([`crate::alarm`]). Die ISR von TIM2
//! ruft [`on_interrupt`].
//!
//! Die Uhr der Runtime liest dieselbe Zeitachse (`Tim2Alarm`): Eine Frist
//! von `service` ist ein Zeitpunkt dieses Treibers.

use core::cell::RefCell;
use core::task::Waker;

use cortex_m::interrupt::{CriticalSection, Mutex};
use embassy_time_queue_utils::Queue;

use crate::alarm;

#[expect(
    clippy::as_conversions,
    reason = "MISRA C:2025 R.10.5: im Konstantenausdruck verbreitert nur `as`; u32 nach u64 ist verlustfrei"
)]
const _: () = assert!(embassy_time_driver::TICK_HZ == crate::TIMER_HZ as u64, "ein Tick je Schritt von TIM2");

/// Die Weckzeitpunkte der Aufgaben.
struct Tim2Driver {
    queue: Mutex<RefCell<Queue>>,
}

impl Tim2Driver {
    /// Weckt, was faellig ist, und stellt den Alarm auf den naechsten
    /// Weckzeitpunkt.
    fn rearm(&self, cs: &CriticalSection) {
        let next = self.queue.borrow(cs).borrow_mut().next_expiration(alarm::now_counts());
        alarm::set_target((next != u64::MAX).then_some(next));
    }
}

impl embassy_time_driver::Driver for Tim2Driver {
    fn now(&self) -> u64 {
        alarm::now_counts()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        cortex_m::interrupt::free(|cs| {
            if self.queue.borrow(cs).borrow_mut().schedule_wake(at, waker) {
                self.rearm(cs);
            }
        });
    }
}

embassy_time_driver::time_driver_impl!(
    static DRIVER: Tim2Driver = Tim2Driver { queue: Mutex::new(RefCell::new(Queue::new())) }
);

/// Aus der ISR von TIM2: Ist ein Weckzeitpunkt erreicht, weckt der Treiber,
/// was faellig ist, und stellt den naechsten.
pub fn on_interrupt() {
    if alarm::on_interrupt() {
        cortex_m::interrupt::free(|cs| DRIVER.rearm(cs));
    }
}
