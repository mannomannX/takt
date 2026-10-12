//! Der Zeittreiber von `embassy-time` (Form `rtos` unter Embassy, 12.11;
//! M11 Schritt 14d) auf der Zeitachse der Interruptform: Einheit 0 des
//! SYSTIMER mit `TIMER_HZ`, ein Schritt je Tick von `embassy-time`,
//! Vergleicher 0 auf dem naechsten Weckzeitpunkt ([`crate::alarm`]). Die ISR
//! des Vergleichers ruft [`on_interrupt`].
//!
//! Die Uhr der Runtime liest dieselbe Zeitachse (`SystimerAlarm`): Eine
//! Frist von `service` ist ein Zeitpunkt dieses Treibers.

use core::cell::RefCell;
use core::task::Waker;

use critical_section::{CriticalSection, Mutex};
use embassy_time_queue_utils::Queue;

use crate::alarm;

#[expect(
    clippy::as_conversions,
    reason = "MISRA C:2025 R.10.5: im Konstantenausdruck verbreitert nur `as`; u32 nach u64 ist verlustfrei"
)]
const _: () = assert!(embassy_time_driver::TICK_HZ == crate::TIMER_HZ as u64, "ein Tick je Schritt des SYSTIMER");

/// Die Weckzeitpunkte der Aufgaben.
struct SystimerDriver {
    queue: Mutex<RefCell<Queue>>,
}

impl SystimerDriver {
    /// Weckt, was faellig ist, und stellt den Alarm auf den naechsten
    /// Weckzeitpunkt.
    #[esp_hal::ram]
    fn rearm(&self, cs: CriticalSection<'_>) {
        let next = self.queue.borrow_ref_mut(cs).next_expiration(alarm::now_counts());
        alarm::set_target((next != u64::MAX).then_some(next));
    }
}

impl embassy_time_driver::Driver for SystimerDriver {
    #[esp_hal::ram]
    fn now(&self) -> u64 {
        alarm::now_counts()
    }

    #[esp_hal::ram]
    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            if self.queue.borrow_ref_mut(cs).schedule_wake(at, waker) {
                self.rearm(cs);
            }
        });
    }
}

embassy_time_driver::time_driver_impl!(
    static DRIVER: SystimerDriver = SystimerDriver { queue: Mutex::new(RefCell::new(Queue::new())) }
);

/// Aus der ISR des Vergleichers: Ist ein Weckzeitpunkt erreicht, weckt der
/// Treiber, was faellig ist, und stellt den naechsten.
#[esp_hal::ram]
pub fn on_interrupt() {
    if alarm::on_interrupt() {
        critical_section::with(|cs| DRIVER.rearm(cs));
    }
}
