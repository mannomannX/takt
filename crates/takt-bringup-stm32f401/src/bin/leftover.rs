//! Ein Vorgaenger, der eine Region der MPU stehen laesst (12.3, FB-752).
//!
//! Wer ohne Reset in den DFU-Bootloader springt, nimmt seine Regionen nicht
//! mit. Zephyrs Rueckgabe schaltete nur die MPU aus; das naechste Abbild
//! schaltete sie wieder ein, und Zephyrs Waechter ueber `z_main_stack` galt
//! wieder: Das Bemalen des Stacks blieb an ihm haengen. Dieses Programm tut
//! dasselbe mit Absicht, damit der Board-Test den Fall ohne Zephyr nachstellt
//! (`a_region_left_by_the_previous_image_changes_nothing`): Es meldet sich,
//! belegt die letzte Region nur lesbar ueber [`LEFT_AT`], schaltet die MPU aus
//! und gibt das Board ohne Reset an den Bootloader.

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
#![allow(unsafe_code, reason = "Interrupt-Handler und Register der MPU; 9.5 fuehrt Treiber in der TCB")]
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

use cortex_m::peripheral::MPU;
use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{BAUD, Board, CORE_HZ};
use takt_board_support::mpu::{Access, Region};

/// Wo die Region stehen bleibt: 24 KiB unter dem Ende des RAM, wo Zephyrs
/// `z_main_stack` begann. Dort bemalt jedes Abbild seinen Stack, in Rust wie
/// in C.
const LEFT_AT: u32 = 0x2000_A000;

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
    let (Some(dp), Some(cp)) = (Peripherals::take(), cortex_m::Peripherals::take()) else { halt() };
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
    uart.newline();
    uart.write("takt leftover stm32f401");
    uart.newline();
    uart.write("takt end");
    uart.newline();
    uart.finish(takt_rt_baremetal::DRAIN_ROUNDS);
    leave(&cp.MPU);
    takt_board_stm32f401::bootloader::enter()
}

/// Belegt die letzte Region nur lesbar ueber [`LEFT_AT`] und schaltet die
/// MPU aus, wie Zephyrs Rueckgabe es tat: Die Region bleibt eingeschaltet
/// stehen, bis ein Abbild die MPU wieder einschaltet.
fn leave(mpu: &MPU) {
    let region = takt_board_stm32f401::mpu::regions(mpu).saturating_sub(1);
    let left = Region { base: LEFT_AT, size: 32, eighths: 8 };
    // SAFETY: schaltet die MPU aus; von diesem Programm laeuft danach nur
    // noch der Sprung in den Bootloader.
    unsafe { mpu.ctrl.write(0) };
    // SAFETY: waehlt die letzte Region, bei ausgeschalteter MPU.
    unsafe { mpu.rnr.write(region) };
    // SAFETY: ihre Basis, an 32 Byte ausgerichtet.
    unsafe { mpu.rbar.write(left.rbar(region)) };
    // SAFETY: 32 Byte nur lesbar, die Region eingeschaltet.
    unsafe { mpu.rasr.write(left.rasr(Access::ReadOnly)) };
}

/// Ohne Peripherie, Takt oder Leitung gibt es niemanden, dem das Board sich
/// melden koennte.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
