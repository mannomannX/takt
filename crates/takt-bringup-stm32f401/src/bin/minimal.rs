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

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und Registerzugriff; 9.5 fuehrt Treiber in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "Stufe 2 des Regelwerks, mit `unsafe` (plan/cert.md Abschnitt 6)"
    )
)]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{Board, Led, tick};

/// Die Tickperiode: 1 ms.
const TICK_NS: i64 = 1_000_000;

#[interrupt]
fn TIM2() {
    // SAFETY: TIM2 gehoert der Tickquelle; die ISR schreibt nur `SR`.
    let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };
    // SAFETY: `rc_w0`: Nullen loeschen, Einsen lassen stehen; das Muster
    // loescht nur `UIF`.
    tim2.sr().write(|w| unsafe { w.bits(!1) });
    // Die Periode interessiert hier nicht — nur, dass gezaehlt wird.
    tick::on_timer_interrupt(0);
}

#[entry]
fn main() -> ! {
    let Some(dp) = Peripherals::take() else { halt() };
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    // Scheitert der Takt, bleibt die LED an — hier genuegt das, weil es
    // nur zwei Ausgaenge gibt: blinkt oder blinkt nicht.
    if takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS).is_err() {
        led.on();
        halt();
    }

    // SAFETY: Der Handler oben ist bereit und teilt nur den Zaehler der Ticks.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2) };

    // Die Schleife fragt den Zaehler direkt, ohne `wait_for_tick`: Wenn
    // der Tick laeuft, wechselt die LED jede Sekunde. Ein `wfi` haelt den
    // Kern zwischen den Interrupts an.
    let mut next = 500;
    loop {
        cortex_m::asm::wfi();
        if tick::count() >= next {
            next = next.saturating_add(500);
            led.toggle();
        }
    }
}

/// Ohne Peripherie oder Takt gibt es nichts zu zeigen: Der Kern schlaeft.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
