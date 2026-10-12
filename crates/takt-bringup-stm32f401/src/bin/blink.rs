//! Nur die LED. Kein Timer, keine PLL, keine eigenen Crates.
//!
//! **Die unterste Stufe der Fehlersuche.** Als `minimal` dasselbe Bild
//! zeigte wie das grosse Programm, war klar, dass die Ursache tiefer
//! liegt als in der Tickkette — aber nicht, ob in unserem Code, im
//! Flashvorgang oder am Board. Dieses Programm laesst *alles* weg: Es
//! laeuft auf dem internen 16-MHz-Oszillator, schaltet einen Pin und
//! zaehlt eine Schleife.
//!
//! Blinkt es sichtbar, ist die Kette bis zum Pin in Ordnung, und der
//! Fehler liegt in dem, was `minimal` zusaetzlich tut — PLL, TIM2, ISR.
//! Glimmt es weiter, liegt er davor: im Linker-Skript, im Flashvorgang
//! oder in der Annahme, welcher Pin die LED traegt.

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
#![allow(unsafe_code, reason = "Registerzugriff ohne Abstraktion; das ist der Zweck")]
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
use stm32f4::stm32f401::Peripherals;

#[entry]
fn main() -> ! {
    // `take` gibt die Peripherie beim ersten Aufruf; ohne sie haelt der Kern.
    let Some(dp) = Peripherals::take() else {
        loop {
            cortex_m::asm::wfi();
        }
    };

    // GPIOC-Takt freigeben, mit dem Dummy-Read aus der ST-Errata.
    dp.RCC.ahb1enr().modify(|_, w| w.gpiocen().set_bit());
    let _ = dp.RCC.ahb1enr().read();

    // PC13 als Ausgang: Bits 26 und 27 im Moder-Register auf 0b01.
    // SAFETY: aendert nur das Feld von PC13, und `0b01` ist dort ein Wert,
    // den `MODER` annimmt.
    dp.GPIOC.moder().modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << 26)) | (0b01 << 26)) });

    loop {
        // Bit 13 setzen: Pin high, LED aus (aktiv low).
        // SAFETY: `BSRR` setzt genau dieses Bit; die uebrigen Pins bleiben.
        dp.GPIOC.bsrr().write(|w| unsafe { w.bits(1 << 13) });
        spin();
        // Bit 29 setzen: Pin low, LED an.
        // SAFETY: `BSRR` loescht genau dieses Bit; die uebrigen Pins bleiben.
        dp.GPIOC.bsrr().write(|w| unsafe { w.bits(1 << 29) });
        spin();
    }
}

/// Eine knappe halbe Sekunde bei 16 MHz — grob, aber sichtbar.
fn spin() {
    for _ in 0..800_000u32 {
        core::hint::black_box(());
    }
}
