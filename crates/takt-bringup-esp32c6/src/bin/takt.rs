//! Ein Takt-Programm auf dem ESP32-C6 (plan/esp32c6.md Schritt 4).
//!
//! Dieselbe Schleife wie beim F401: `TimerClock` ueber dem SYSTIMER-Alarm,
//! das erzeugte Programm hinter `Generated`, der Trace ueber
//! USB-Serial-JTAG. Welches Programm laeuft, sagt `takt.toml`.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{Generated, Telemetry, Ws2812};
use takt_rt_core::{Clock, Program};

esp_bootloader_esp_idf::esp_app_desc!();

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::TICK_NS;

/// Alle wie viele Ticks der Zustand ausgegeben wird. USB-Serial-JTAG ist
/// schnell, aber das FIFO blockiert, wenn der Host nicht liest; ein
/// Abzug je 100 Ticks haelt den Tick frei.
const TRACE_EVERY: u64 = 100;

/// Konformitaetslauf (plan/esp32c6.md 5): `TAKT_TICKS` beim Bau gesetzt
/// heisst jeden Tick ausgeben, nach so vielen Ticks `takt end` und Halt.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

static mut UART: Option<Telemetry> = None;
static mut LED: Option<Ws2812> = None;

/// Vom Rahmen gerufen: eine Zeile Trace, nullterminiert.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    let mut p = text;
    for _ in 0..256 {
        // SAFETY: der Rahmen uebergibt einen nullterminierten String; die
        // Schleife endet spaetestens nach 256 Byte.
        let b = unsafe { *p };
        if b == 0 {
            return;
        }
        uart.write_byte(b);
        p = unsafe { p.add(1) };
    }
}

/// Vom Rahmen gerufen: eine Zahl im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    uart.write_i64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    uart.write_u64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Fliesskommazahl im Trace, als kuerzeste
/// Ziffernfolge, die den Wert eindeutig zurueckgibt (Bitgleichheit, 4.2).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    let _ = write!(uart, "{value:?} ");
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    let _ = write!(uart, "0x{value:02x}");
}

/// Der Output `ui_led` des Programms auf der RGB-LED.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_ui_led(value: u8) {
    let Some(led) = (unsafe { (*&raw mut LED).as_mut() }) else { return };
    if value != 0 {
        led.on();
    } else {
        led.off();
    }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let mut uart = Telemetry::new();
    let Ok(timer) = takt_board_esp32c6::init(peripherals.SYSTIMER, TICK_NS) else {
        uart.write("takt: Periode nicht einrichtbar");
        uart.newline();
        loop {
            core::hint::spin_loop();
        }
    };
    uart.write("takt esp32c6: tick ");
    uart.write_i64(timer.nominal_ns());
    uart.write(" ns");
    uart.newline();
    unsafe { UART = Some(uart) };
    if let Ok(led) = Ws2812::new(peripherals.RMT, peripherals.GPIO8) {
        unsafe { LED = Some(led) };
    }

    let limit: u64 = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    let trace_every = if limit > 0 { 1 } else { TRACE_EVERY };
    let mut clock = takt_rt_baremetal::TimerClock::new(timer, TICK_NS);
    let mut program = Generated::init(false);
    if limit > 0 {
        program.dump();
    }
    let mut next_trace = trace_every;
    let mut k: u64 = 0;
    let mut reported: u64 = 0;
    loop {
        clock.wait_until(0);
        k += 1;
        program.tick(k, k as i64 * TICK_NS);
        program.commit();
        if k >= next_trace {
            next_trace = k + trace_every;
            program.dump();
            report(&clock, &mut reported);
        }
        if limit > 0 && k >= limit {
            break;
        }
    }
    if let Some(uart) = unsafe { (*&raw mut UART).as_mut() } {
        uart.write("takt end");
        uart.newline();
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Verpasste Ticks, sobald ihre Zahl steigt.
fn report(clock: &takt_rt_baremetal::TimerClock<takt_board_esp32c6::SystimerTick>, last: &mut u64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    let missed = clock.missed();
    if missed > *last {
        *last = missed;
        uart.write("  verpasste Ticks: ");
        uart.write_u64(missed);
        uart.newline();
    }
}
