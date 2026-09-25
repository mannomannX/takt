//! Die kuratierten Natives auf dem ESP32-C6 (13.8): je Vektor aus
//! `grammar/takt-native.md` das Ergebnis und der Stack-Bedarf des Aufrufs.
//!
//! Dasselbe Programm wie auf dem F401 (`takt-bringup-stm32f401`):
//! `takt_conformance::natives` vergleicht die Ergebnisse mit dem Wirt und
//! den Stack mit der Zusage `stack` (4.5, 12.3).
#![no_std]
#![no_main]

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{WfiSleep, stack};
use takt_native::{Native, Output};
use takt_rt_baremetal::bench::write_native;
use takt_rt_baremetal::{DRAIN_ROUNDS, Sleep};

esp_bootloader_esp_idf::esp_app_desc!();

mod vectors {
    include!(concat!(env!("OUT_DIR"), "/native_vectors.rs"));
}

/// Wie tief unter dem Stackzeiger der Bedarf gemessen wird: weit ueber
/// jeder Zusage der Natives, die hier laufen (die groesste,
/// `hmac_sha256`, sagt 768 Byte zu, 4.5). Wer mehr braucht, meldet das
/// Fenster und bricht damit seine Zusage sichtbar.
const WINDOW: usize = 4096;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    let mut uart = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);

    uart.newline();
    uart.write("takt natives esp32c6");
    uart.newline();
    for (i, (name, inputs)) in vectors::VECTORS.iter().enumerate() {
        let Some(f) = Native::by_name(name) else { continue };
        let mut out = None;
        let used = stack::usage_of(WINDOW, || out = core::hint::black_box(takt_native::call(f, inputs)));
        let mut bytes = [0u8; 32];
        let result: &[u8] = match out {
            Some(Output::Scalar(v)) => {
                bytes[..8].copy_from_slice(&v.to_be_bytes());
                &bytes[..8]
            }
            Some(Output::Digest(d)) => {
                bytes.copy_from_slice(&d);
                &bytes
            }
            None => &[],
        };
        write_native(&mut uart, i, result, used);
        // Die naechste Messung sperrt die Interrupts; was im Ring steht,
        // muss vorher an die Leitung, sonst liefe er ueber.
        uart.drain(DRAIN_ROUNDS);
    }
    uart.write("takt end");
    uart.newline();
    uart.drain(DRAIN_ROUNDS);

    // Danach bleibt die Konsole offen: `TAKT` setzt den Chip zurueck (FB-264).
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        uart.flush();
    }
}
