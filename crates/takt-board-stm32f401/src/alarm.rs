//! Der Alarm der Interruptform (12.11) auf TIM2: freilaufend ueber 32 Bit
//! mit `TIMER_HZ`, der Vergleich CC1 auf die Frist, die `service` nennt,
//! und der Ueberlauf als Epoche der Zeitachse. Die Rechnung dazu steht in
//! `takt_board_support::alarm`; hier stehen die Register.
//!
//! **Zwei Programme auf einem Kern** (12.11, M11 Schritt 16) teilen die
//! Zeitachse: Jedes hat seinen Vergleichskanal ([`CHANNELS`], CC1 und CC2),
//! und die ISR von TIM2 sagt je Kanal, ob seine Frist erreicht ist
//! ([`on_interrupt_all`]).
//!
//! TIM2 gehoert in dieser Form dem Alarm: Die ISR des Boards ruft
//! [`on_interrupt`], und ist die Frist erreicht, rechnet sie den Schritt.
//! Den Job-Interrupt waehlt das Bring-up: eine freie Leitung niedrigster
//! Prioritaet, die nur von Hand ansteht. Der eigene Kern nutzt dieselbe
//! Zeitachse und stellt den Alarm als Wecker auf jede Tickgrenze
//! ([`crate::tick`], FB-742).

use core::cell::Cell;
use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::interrupt::Mutex;
use cortex_m::peripheral::NVIC;
use stm32f4::stm32f401::{Interrupt, TIM2};
use takt_board_support::alarm::{Compare, compare, counts, passed};
use takt_board_support::clock::Scale;

/// Wie oft TIM2 uebergelaufen ist; die ISR zaehlt.
static EPOCH: AtomicU32 = AtomicU32::new(0);

/// So viele Vergleichskanaele stellt der Alarm: CC1 und CC2.
pub const CHANNELS: usize = 2;

/// Der Zielstand je Kanal auf der 64-Bit-Zeitachse; `None` ohne Alarm.
/// 64 Bit hat der Cortex-M4 nicht atomar: ein kritischer Abschnitt.
static TARGET: [Mutex<Cell<Option<u64>>>; CHANNELS] = [const { Mutex::new(Cell::new(None)) }; CHANNELS];

/// TIM2 als freilaufender Zaehler mit `prescaler`: von 0 bis `u32::MAX`,
/// der Ueberlauf meldet sich, der Vergleich erst, wenn ein Alarm steht.
pub(crate) fn start(rcc: &stm32f4::stm32f401::RCC, tim2: &TIM2, prescaler: u16) {
    rcc.apb1enr().modify(|_, w| w.tim2en().set_bit());
    let _ = rcc.apb1enr().read();
    // SAFETY: `PSC` nimmt jeden 16-Bit-Wert.
    tim2.psc().write(|w| unsafe { w.psc().bits(prescaler) });
    // SAFETY: `ARR` von TIM2 ist 32 Bit breit und nimmt jeden Wert.
    tim2.arr().write(|w| unsafe { w.bits(u32::MAX) });
    // Uebernehmen und das dabei gesetzte `UIF` wieder loeschen: `UG` erzeugt
    // dasselbe Ereignis wie ein Ueberlauf, und bliebe es stehen, zaehlte die
    // erste ISR eine Epoche, die es nie gab. Der erste Ueberlauf kommt vom
    // Zaehler.
    tim2.egr().write(|w| w.ug().set_bit());
    // SAFETY: `SR` ist `rc_w0`; Nullen loeschen jedes Flag.
    tim2.sr().write(|w| unsafe { w.bits(0) });
    EPOCH.store(0, Ordering::SeqCst);
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
        let epoch = EPOCH.load(Ordering::SeqCst);
        let low = t.cnt().read().bits();
        let wrapped = t.sr().read().uif().bit_is_set();
        if EPOCH.load(Ordering::SeqCst) == epoch {
            return counts(epoch, low, wrapped);
        }
    }
}

/// Das Flag `CCxIF` von Kanal `ch` in `SR`: CC1IF ist Bit 1, CC2IF Bit 2.
fn flag(ch: usize) -> u32 {
    if ch == 0 { 0b010 } else { 0b100 }
}

/// Der Zielstand von Kanal `ch`.
fn target_of(ch: usize) -> &'static Mutex<Cell<Option<u64>>> {
    let [first, second] = &TARGET;
    if ch == 0 { first } else { second }
}

/// Schaltet den Interrupt des Vergleichs von Kanal `ch` ein oder aus.
fn enable(ch: usize, on: bool) {
    let t = tim2();
    match ch {
        0 => t.dier().modify(|_, w| w.cc1ie().bit(on)),
        _ => t.dier().modify(|_, w| w.cc2ie().bit(on)),
    };
}

/// Stellt den Vergleich von Kanal `ch` auf `target`: in dieser Epoche ueber
/// `CCR1` bzw. `CCR2`, hinter dem naechsten Ueberlauf gar nicht — dessen
/// Interrupt fragt erneut —, und eine Frist, die schon da ist oder beim
/// Stellen verstrich, laesst die ISR sofort anstehen.
fn aim(ch: usize, target: u64) {
    let t = tim2();
    match compare(target, now_counts()) {
        Compare::Due => NVIC::pend(Interrupt::TIM2),
        Compare::At(low) => {
            match ch {
                // SAFETY: `CCR1` von TIM2 ist 32 Bit breit und nimmt jeden Wert.
                0 => t.ccr1().write(|w| unsafe { w.bits(low) }),
                // SAFETY: ebenso `CCR2`.
                _ => t.ccr2().write(|w| unsafe { w.bits(low) }),
            };
            // SAFETY: `SR` ist `rc_w0`; das Muster loescht nur `CCxIF` dieses Kanals.
            t.sr().write(|w| unsafe { w.bits(!flag(ch)) });
            enable(ch, true);
            if passed(target, now_counts()) {
                NVIC::pend(Interrupt::TIM2);
            }
        }
        Compare::Later => enable(ch, false),
    }
}

/// Stellt den Alarm des ersten Kanals auf `target`, oder nimmt ihn zurueck.
pub(crate) fn set_target(target: Option<u64>) {
    set_target_on(0, target);
}

/// Stellt den Alarm von Kanal `ch` auf `target` auf der Zeitachse, oder
/// nimmt ihn zurueck.
fn set_target_on(ch: usize, target: Option<u64>) {
    cortex_m::interrupt::free(|cs| target_of(ch).borrow(cs).set(target));
    match target {
        Some(target) => aim(ch, target),
        None => enable(ch, false),
    }
}

/// Aus der ISR von TIM2, wenn nur der erste Kanal einen Alarm stellt: wie
/// [`on_interrupt_all`] fuer ihn.
pub fn on_interrupt() -> bool {
    let [first, _] = on_interrupt_all();
    first
}

/// Aus der ISR von TIM2: zaehlt einen Ueberlauf, loescht die Vergleiche und
/// sagt je Kanal, ob seine Frist erreicht ist. Ist sie es, steht auf ihm
/// danach kein Alarm mehr; der Schritt stellt den naechsten.
pub fn on_interrupt_all() -> [bool; CHANNELS] {
    let t = tim2();
    let sr = t.sr().read();
    if sr.uif().bit_is_set() {
        // SAFETY: `rc_w0`: Nullen loeschen, Einsen lassen stehen; das Muster
        // loescht nur `UIF`.
        t.sr().write(|w| unsafe { w.bits(!1) });
        EPOCH.fetch_add(1, Ordering::SeqCst);
    }
    let mut due = [false; CHANNELS];
    for (ch, d) in due.iter_mut().enumerate() {
        if sr.bits() & flag(ch) != 0 {
            // SAFETY: wie `UIF`, hier nur `CCxIF` dieses Kanals.
            t.sr().write(|w| unsafe { w.bits(!flag(ch)) });
        }
        let target = cortex_m::interrupt::free(|cs| target_of(ch).borrow(cs).get());
        match target {
            Some(target) if passed(target, now_counts()) => {
                cortex_m::interrupt::free(|cs| target_of(ch).borrow(cs).set(None));
                enable(ch, false);
                *d = true;
            }
            // Nach einem Ueberlauf liegt das Ziel vielleicht in dieser Epoche.
            Some(target) => aim(ch, target),
            None => {}
        }
    }
    due
}

/// Der Alarm des Boards fuer `takt_rt_baremetal::interrupt` und die Uhr
/// der Interruptform: die Zeitachse in Nanosekunden seit dem Start.
#[derive(Clone, Copy, Debug)]
pub struct Tim2Alarm {
    scale: Scale,
    /// Die Leitung des Job-Interrupts.
    jobs: Interrupt,
    /// Der Vergleichskanal, `0` fuer CC1.
    channel: usize,
}

impl Tim2Alarm {
    /// Der Alarm auf TIM2 mit `timer_hz`; Jobs laufen im Interrupt `jobs`.
    pub fn new(timer_hz: u32, jobs: Interrupt) -> Tim2Alarm {
        Tim2Alarm { scale: Scale::of(timer_hz), jobs, channel: 0 }
    }

    /// Derselbe Alarm auf Vergleichskanal `channel` (unter [`CHANNELS`]),
    /// fuer ein zweites Programm auf dem Kern (12.11).
    pub fn on_channel(self, channel: usize) -> Tim2Alarm {
        Tim2Alarm { channel: if channel == 0 { 0 } else { 1 }, ..self }
    }

    /// Jetzt, in Nanosekunden seit dem Start des Zaehlers.
    pub fn now_ns(&self) -> i64 {
        self.scale.ns(now_counts())
    }
}

impl takt_rt_baremetal::interrupt::Alarm for Tim2Alarm {
    fn arm(&mut self, at: i64) {
        set_target_on(self.channel, Some(self.scale.counts_at(at)));
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
