//! Das Sprungbrett der Black Pill (FB-385): Sektor 1, wohin der
//! HID-Bootloader nach jedem Reset springt.
//!
//! Die Anwendung beginnt in Sektor 4, weil Sektor 2 und 3 das
//! `persist`-Journal halten (`takt_board_stm32f401::nvm`). Das Sprungbrett
//! prueft ihre Vektortabelle und startet sie; ist keine da — nach dem
//! ersten Schreiben des Sprungbretts oder einem abgebrochenen Abbild —,
//! gibt es das Board an den DFU-Bootloader des ROM, und der Host schreibt
//! ohne Hand am Board.
//!
//! **Es fasst nichts an, was die Anwendung vorfindet**: keinen Takt, keine
//! Peripherie, kein Reset-Flag. Die Anwendung beginnt wie direkt hinter dem
//! HID-Bootloader und setzt `VTOR` selbst (`set-vtor` im Bring-up).

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Sprung in die Anwendung ueber ihre Vektortabelle; 9.5 fuehrt den Start in der TCB")]

use cortex_m_rt::entry;
use takt_board_stm32f401::bootloader;

/// Wo die Anwendung beginnt: Sektor 4.
const APP: u32 = 0x0801_0000;

/// Wo sie endet: 256 KiB Flash der Variante CC.
const APP_END: u32 = 0x0804_0000;

/// Das RAM, in das ihr Stackzeiger zeigen muss.
const RAM: core::ops::RangeInclusive<u32> = 0x2000_0000..=0x2001_0000;

#[entry]
fn main() -> ! {
    let table = APP as *const u32;
    // SAFETY: die ersten beiden Worte der Anwendung, im Flash, das immer
    // lesbar abgebildet ist; geloescht stehen dort `0xFFFF_FFFF`.
    let (sp, reset) = unsafe { (table.read_volatile(), table.add(1).read_volatile()) };
    if RAM.contains(&sp) && (APP..APP_END).contains(&reset) && reset % 2 == 1 {
        // SAFETY: Die Vektortabelle ist plausibel: Stackzeiger im RAM,
        // Resetvektor in der Anwendung und Thumb. Das Sprungbrett kehrt
        // nicht zurueck, und nichts von ihm lebt danach weiter.
        unsafe { cortex_m::asm::bootload(table) }
    }
    bootloader::enter()
}

/// Eine Panik kann hier nicht entstehen; kaeme sie doch, bleibt das Board
/// ueber den DFU-Bootloader erreichbar.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    bootloader::enter()
}
