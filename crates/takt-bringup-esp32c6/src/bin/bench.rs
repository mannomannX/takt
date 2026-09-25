//! `takt bench` auf dem ESP32-C6 (13.8): ein Messkern und, wenn gebunden,
//! seine C-Referenz, je `TAKT_TICKS`-mal gemessen.
//!
//! Dasselbe Programm wie auf dem F401 (`takt-bringup-stm32f401`): ein
//! ganzer Tick des Rahmens, gemessen mit dem Zyklenzaehler des Kerns bei
//! gesperrten Interrupts; die C-Referenz rechnet dasselbe und muss
//! denselben `digest` ergeben. Dazu Stack-Tiefe und Subnormal-Vektor.
//! Kern, Rahmen und Referenz liegen im RAM (`rwtext_hook.x`, 12.3): Der
//! Flash-Cache mass sonst den Cache mit.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{CORE_HZ, Generated, WfiSleep, cycles, stack};
use takt_rt_baremetal::bench::{series, subnormal_failures, write_series, write_value};
use takt_rt_baremetal::{DRAIN_ROUNDS, Sleep};
use takt_rt_core::{Program, tick_end};

esp_bootloader_esp_idf::esp_app_desc!();

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}

mod reference {
    include!(concat!(env!("OUT_DIR"), "/bench_reference.rs"));
}

unsafe extern "C" {
    /// Ein Durchlauf der C-Referenz, wie ein Tick des Kerns.
    fn takt_bench_reference();
    /// Was die C-Referenz nach allen Durchlaeufen ergibt.
    fn takt_bench_reference_digest() -> u64;
}

/// Wie oft gemessen wird: `TAKT_TICKS` beim Bau, sonst tausendmal.
const RUNS: Option<&str> = option_env!("TAKT_TICKS");

/// Durchlaeufe vor der Messung: Die Initialisierung der Maschinen ist vorbei.
const WARMUP: u32 = 8;

/// Vom Rahmen gerufen: Ein Messkern schreibt keinen Trace.
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Text; er wird nicht gelesen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(_text: *const u8) {}

/// Vom Rahmen gerufen: Ein Messkern schreibt keinen Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(_value: i64) {}

/// Vom Rahmen gerufen: Ein Messkern schreibt keinen Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(_value: u64) {}

/// Vom Rahmen gerufen: Ein Messkern schreibt keinen Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(_value: f64) {}

/// Vom Rahmen gerufen: Ein Messkern schreibt keinen Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(_value: u8) {}

#[main]
fn main() -> ! {
    // Zuerst: Was spaeter an Stack gebraucht wird, soll auf dem Muster landen.
    stack::paint();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    let mut uart = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    cycles::enable();

    uart.newline();
    uart.write("takt bench esp32c6");
    uart.newline();
    write_value(&mut uart, "core_hz", u64::from(CORE_HZ));

    let runs = RUNS.and_then(|r| r.parse().ok()).unwrap_or(1000);
    let mut program = Generated::init(false);
    let mut k = 0u64;
    for _ in 0..WARMUP {
        program.tick(k, tick_end(k, takt::TICK_NS));
        k += 1;
    }
    let takt = critical_section::with(|_| {
        series(runs, cycles::now, || {
            program.tick(k, tick_end(k, takt::TICK_NS));
            k += 1;
        })
    });
    write_series(&mut uart, "takt", &takt, program.output(0) as u64);

    if reference::PRESENT {
        // Das Programm hat mit `init` Tick 0 hinter sich; die Referenz
        // rechnet ihn hier nach. Sonst laege der Kern einen Tick vorn, und
        // die Digests liessen sich nicht vergleichen (FB-287).
        // SAFETY: die C-Referenz, ein Durchlauf ohne Argumente.
        unsafe { takt_bench_reference() };
        for _ in 0..WARMUP {
            // SAFETY: die C-Referenz, ein Durchlauf ohne Argumente.
            unsafe { takt_bench_reference() };
        }
        // SAFETY: wie oben; die Referenz haelt ihren Zustand selbst.
        let c = critical_section::with(|_| series(runs, cycles::now, || unsafe { takt_bench_reference() }));
        // SAFETY: liest den Zustand der Referenz.
        write_series(&mut uart, "c", &c, unsafe { takt_bench_reference_digest() });
    }

    write_value(&mut uart, "stack", u64::from(stack::high_water()));
    write_value(&mut uart, "subnormal", u64::from(subnormal_failures()));
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
