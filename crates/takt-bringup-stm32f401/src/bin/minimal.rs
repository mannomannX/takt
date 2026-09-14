//! Das kleinstmoegliche Programm: LED, Timer, sonst nichts.
//!
//! **Wozu ein zweites Programm.** Als das Bring-up-Programm ein
//! unerwartetes Bild zeigte — weiches Glimmen statt Blinken —, liessen
//! sich vier Erklaerungen konstruieren und keine pruefen: Jede haette an
//! einer anderen Stelle liegen koennen (Takt, Timer, Zaehler, LED, UART,
//! DWT). Dieses Programm laesst alles weg, was nicht gebraucht wird, und
//! beantwortet damit eine einzige Frage:
//!
//! **Zaehlt der Tick mit 1 kHz?**
//!
//! Blinkt die LED im Sekundentakt, stimmt die ganze Kette bis hierher —
//! PLL, TIM2, ISR, Zaehler —, und der Fehler liegt in dem, was das
//! groessere Programm zusaetzlich tut. Glimmt sie weiter, liegt er davor.
//!
//! Das ist die Halbierung des Suchraums mit den Mitteln, die auf einem
//! Board ohne Debugger zur Verfuegung stehen.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und Registerzugriff; 9.5 fuehrt Treiber in der TCB")]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{Board, Led, tick};

/// Die Tickperiode: 1 ms.
const TICK_NS: i64 = 1_000_000;

#[interrupt]
fn TIM2() {
    let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };
    // `rc_w0`: Nullen loeschen, Einsen lassen stehen.
    tim2.sr().write(|w| unsafe { w.bits(!1) });
    // Die Periode interessiert hier nicht — nur, dass gezaehlt wird.
    tick::on_timer_interrupt(0);
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    // Scheitert der Takt, bleibt die LED an — hier genuegt das, weil es
    // nur zwei Ausgaenge gibt: blinkt oder blinkt nicht.
    if takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS).is_err() {
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    }

    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2) };

    // Die Schleife fragt den Zaehler direkt, ohne `wait_for_tick`: Wenn
    // der Tick laeuft, wechselt die LED jede Sekunde. Ein `wfi` haelt den
    // Kern zwischen den Interrupts an.
    let mut next = 500;
    loop {
        cortex_m::asm::wfi();
        if tick::count() >= next {
            next += 500;
            led.toggle();
        }
    }
}
