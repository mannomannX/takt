//! Der Tick allein (plan/esp32c6.md Schritt 3): Alarm des SYSTIMER,
//! Zaehler in der ISR, gemessene Periode aus dem Zyklenzaehler. Jede
//! Sekunde eine Zeile ueber USB-Serial-JTAG — Tickzaehler, gemessene
//! Periode, verpasste Ticks.
#![no_std]
#![no_main]

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{CORE_HZ, Ws2812};
use takt_rt_baremetal::{DRAIN_ROUNDS, TickSource};

esp_bootloader_esp_idf::esp_app_desc!();

const TICK_NS: i64 = 1_000_000;
const REPORT_EVERY: u64 = 1_000;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let mut uart = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    let mut clock = match takt_board_esp32c6::init(peripherals.SYSTIMER, TICK_NS) {
        Ok(c) => c,
        Err(e) => {
            uart.write("tick: Periode nicht einrichtbar: ");
            uart.write(match e {
                takt_board_esp32c6::InitError::Period(_) => "Periode",
                takt_board_esp32c6::InitError::SubMicrosecond => "keine ganze Mikrosekunde",
            });
            uart.newline();
            uart.drain(DRAIN_ROUNDS);
            loop {
                core::hint::spin_loop();
            }
        }
    };
    uart.write("takt esp32c6: tick ");
    uart.write_i64(clock.nominal_ns());
    uart.write(" ns nominal, Kern ");
    uart.write_u64(u64::from(CORE_HZ));
    uart.write(" Hz");
    uart.newline();
    uart.drain(DRAIN_ROUNDS);

    let mut led = Ws2812::new(peripherals.RMT, peripherals.GPIO8).ok();
    let mut lit = false;
    let mut next_report = REPORT_EVERY;
    let mut last = clock.ticks();
    let mut missed: u64 = 0;
    loop {
        clock.wait_for_tick();
        let now = clock.ticks();
        missed += now.saturating_sub(last).saturating_sub(1);
        last = now;
        if now >= next_report {
            next_report = now + REPORT_EVERY;
            lit = !lit;
            if let Some(led) = led.as_mut() {
                if lit { led.on() } else { led.off() }
            }
            uart.write("ticks ");
            uart.write_u64(now);
            uart.write("  periode_ns ");
            uart.write_i64(clock.last_period_ns());
            uart.write("  verpasst ");
            uart.write_u64(missed);
            uart.newline();
            uart.drain(DRAIN_ROUNDS);
        }
    }
}
