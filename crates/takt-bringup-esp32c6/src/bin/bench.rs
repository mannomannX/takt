//! `takt bench` auf dem ESP32-C6 (13.8): das Messprogramm als Baustein,
//! wie jeder Wirt es bindet (`takt bench --emit embed`, plan/m11.md 2.12).
//!
//! Dasselbe wie auf dem F401 (`takt-bringup-stm32f401`): Das Board stellt
//! den Zyklenzaehler des Kerns und die Konsole und faehrt jeden Schritt bei
//! gesperrten Interrupts. Messprogramm und Haken liegen im RAM (12.3,
//! `takt_bench_ram.x`): Aus dem Flash-Cache maesse der Kern den Cache mit.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Messprogramms; 9.5 fuehrt Treiber in der TCB")]

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{CORE_HZ, Telemetry, WfiSleep, cycles};
use takt_rt_baremetal::{DRAIN_ROUNDS, Sleep};

esp_bootloader_esp_idf::esp_app_desc!();

/// Das Messprogramm (`TAKT_BENCH_RS`).
mod takt_bench {
    #![allow(dead_code, reason = "die Kennung `SUITE` braucht nur, wer das Protokoll selbst prueft")]
    include!(env!("TAKT_BENCH_RS"));
}

/// Wie oft gemessen wird: `TAKT_TICKS` beim Bau, sonst tausendmal.
const RUNS: Option<&str> = option_env!("TAKT_TICKS");

/// Der Wirt des Messprogramms.
struct Host {
    uart: Telemetry,
}

impl takt_bench::Host for Host {
    #[esp_hal::ram]
    fn cycles(&mut self) -> u32 {
        cycles::now()
    }

    /// Jede Zeile geht ganz an die Konsole, bevor der naechste Schritt die
    /// Interrupts sperrt.
    fn put(&mut self, byte: u8) {
        self.uart.write_byte(byte);
        if byte == b'\n' {
            self.uart.flush();
            self.uart.drain(DRAIN_ROUNDS);
        }
    }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    let uart = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    cycles::enable();

    let runs = RUNS.and_then(|r| r.parse().ok()).unwrap_or(1000);
    let mut host = Host { uart };
    takt_bench::begin(&mut host, CORE_HZ);
    takt_bench::steps(&mut host, runs, |measure| critical_section::with(|_| measure()));
    takt_bench::end(&mut host);

    // Danach bleibt die Konsole offen: `TAKT` setzt den Chip zurueck (FB-264).
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        host.uart.flush();
    }
}
