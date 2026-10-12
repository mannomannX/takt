//! Das kleinste Lebenszeichen (plan/esp32c6.md Schritt 2): Start ueber
//! `esp-hal`, eine Zeile ueber USB-Serial-JTAG, dann Stillstand.
//!
//! Die RGB-LED der DevKits haengt an einem WS2812, nicht an einem Pin;
//! das Lebenszeichen ist darum die Zeile, nicht ein Blinken.
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

use esp_hal::main;
use takt_rt_baremetal::DRAIN_ROUNDS;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut telemetry = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    telemetry.write("takt esp32c6: minimal");
    telemetry.newline();
    telemetry.drain(DRAIN_ROUNDS);
    loop {
        // Nichts zu tun: `wfi` hielte den JTAG-Kanal nicht wach, also spinnt der Kern.
        core::hint::spin_loop();
    }
}
