//! Die kuratierten Natives und die korrekt gerundete Mathematik auf dem
//! ESP32-C6 (13.8): je Vektor aus `grammar/takt-native.md` und aus Stufe 2
//! von `grammar/libtaktm.md` das Ergebnis und der Stack-Bedarf jedes
//! Einstiegs aus `takt-native-abi`, den der erzeugte Code ruft (FB-293),
//! bei der Mathematik dazu die Zyklen eines Aufrufs (FB-344).
//!
//! Dasselbe Programm wie auf dem F401 (`takt-bringup-stm32f401`):
//! `takt_conformance::natives` vergleicht die Ergebnisse mit dem Wirt und
//! den Stack mit der Zusage `stack` (4.5, 12.3), `takt_conformance::math`
//! die Mathematik mit der Norm.
#![no_std]
#![no_main]

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{WfiSleep, cycles, stack};
use takt_native::Native;
use takt_native_abi::measure;
use takt_rt_baremetal::bench::{write_math, write_native};
use takt_rt_baremetal::{DRAIN_ROUNDS, Sleep};

esp_bootloader_esp_idf::esp_app_desc!();

mod vectors {
    include!(concat!(env!("OUT_DIR"), "/native_vectors.rs"));
}

mod math_vectors {
    include!(concat!(env!("OUT_DIR"), "/math_vectors.rs"));
}

/// Wie tief unter dem Stackzeiger der Bedarf gemessen wird: weit ueber
/// jeder Zusage der Natives, die hier laufen (die groessten sind die Jobs
/// aus `takt-crypto`, 4.5). Wer mehr braucht, meldet das Fenster und
/// bricht damit seine Zusage sichtbar.
const WINDOW: usize = 12_288;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    let mut uart = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    cycles::enable();

    uart.newline();
    uart.write("takt natives esp32c6");
    uart.newline();
    for (i, (name, inputs)) in vectors::VECTORS.iter().enumerate() {
        let measured =
            Native::by_name(name).and_then(|f| measure::vector(f, inputs, &mut |call| stack::usage_of(WINDOW, call)));
        let Some(run) = measured else { continue };
        write_native(&mut uart, i, run.result(), run.stack, run.others());
        // Die naechste Messung sperrt die Interrupts; was im Ring steht,
        // muss vorher an die Leitung, sonst liefe er ueber.
        uart.drain(DRAIN_ROUNDS);
    }
    // Die korrekt gerundete Mathematik (4.2): dieselben Einstiege `takt_m_*`,
    // die der erzeugte Code ruft, mit Stack und Zyklen eines Aufrufs.
    for (i, &(name, wide, x, y)) in math_vectors::VECTORS.iter().enumerate() {
        let Some(entry) = takt_native_abi::math::by_name(name, wide) else { continue };
        let (mut result, mut spent) = (0, 0);
        let used = stack::usage_of(WINDOW, || {
            let start = cycles::now();
            result = entry.call(x, y);
            spent = cycles::now().wrapping_sub(start);
        });
        write_math(&mut uart, i, result, used, spent);
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
