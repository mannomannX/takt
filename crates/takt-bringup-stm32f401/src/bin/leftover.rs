//! Ein Vorgaenger, der eine Region der MPU stehen laesst (12.3, FB-752).
//!
//! Wer ohne Reset in den DFU-Bootloader springt, nimmt seine Regionen nicht
//! mit. Zephyrs Rueckgabe schaltete nur die MPU aus; das naechste Abbild
//! schaltete sie wieder ein, und Zephyrs Waechter ueber `z_main_stack` galt
//! wieder: Das Bemalen des Stacks blieb an ihm haengen. Dieses Programm stellt
//! den Fall ohne Zephyr nach (`a_region_left_by_the_previous_image_changes_nothing`):
//! Es merkt sich den Wunsch, dem naechsten Abbild eine Region zu
//! hinterlassen, meldet sich und wartet auf `TAKT` wie jedes Abbild; die
//! Region beschreibt das Sprungbrett nach dem Reset der Rueckgabe
//! ([`takt_board_stm32f401::bootloader::bequeath_region`]). Selbst springt
//! es nicht in den ROM-Bootloader: Aus einer laufenden Anwendung meldete der
//! sich nicht am USB an (FB-758).

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
#![allow(unsafe_code, reason = "Interrupt-Handler; 9.5 fuehrt Treiber in der TCB")]
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
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, WfiSleep};
use takt_rt_baremetal::Sleep;

/// Die Periode des Zeitgebers, den das Einrichten des Takts mitbringt; einen
/// Tick rechnet dieses Programm nicht.
const TICK_NS: i64 = 1_000_000;

/// Die Leitung sendet im Interrupt.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

#[entry]
fn main() -> ! {
    let Some(dp) = Peripherals::take() else { halt() };
    let board = Board::WEACT_BLACKPILL;
    let Ok(_timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        halt();
    };
    let Ok(mut uart) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        halt();
    };
    // SAFETY: Der Handler ist oben definiert und teilt nur den FIFO der
    // Leitung, der fuer einen Schreiber und einen Leser gebaut ist.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1) };
    takt_board_stm32f401::bootloader::bequeath_region();
    uart.newline();
    uart.write("takt leftover stm32f401");
    uart.newline();
    uart.write("takt end");
    uart.newline();
    uart.drain(takt_rt_baremetal::DRAIN_ROUNDS);
    // Bis `TAKT` das Board zurueckholt: `flush` fuehrt den Wunsch aus.
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        uart.flush();
    }
}

/// Ohne Peripherie, Takt oder Leitung gibt es niemanden, dem das Board sich
/// melden koennte.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
