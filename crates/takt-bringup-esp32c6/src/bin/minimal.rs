//! Das kleinste Lebenszeichen (plan/esp32c6.md Schritt 2): Start ueber
//! `esp-hal`, eine Zeile ueber USB-Serial-JTAG, dann Stillstand.
//!
//! Die RGB-LED der DevKits haengt an einem WS2812, nicht an einem Pin;
//! das Lebenszeichen ist darum die Zeile, nicht ein Blinken.
#![no_std]
#![no_main]

use esp_hal::main;
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    println!("takt esp32c6: minimal");
    loop {
        // Nichts zu tun: `wfi` hielte den JTAG-Kanal nicht wach, also spinnt der Kern.
        core::hint::spin_loop();
    }
}
