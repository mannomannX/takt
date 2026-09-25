//! `takt bench` auf der Black Pill (13.8): ein Messkern und, wenn gebunden,
//! seine C-Referenz, je `TAKT_TICKS`-mal gemessen.
//!
//! Der Kern ist das Takt-Programm aus `TAKT_PROGRAM`, gebaut wie fuer das
//! Binary `takt`; gemessen wird ein ganzer Tick des Rahmens
//! (`takt_mcu_tick`), mit dem Zyklenzaehler und bei gesperrten
//! Interrupts — eine ISR mitten in der Messung ist Last, nicht Kosten des
//! Programms. Die C-Referenz aus `TAKT_BENCH_C` rechnet dasselbe; ihr
//! `digest` muss dem des Kerns gleichen, sonst vergliche das Verhaeltnis
//! zwei verschiedene Rechnungen.
//!
//! Dazu die Stack-Tiefe unter Last (Painting) und der Subnormal-Vektor
//! (4.2). Die Zeilen liest `takt_conformance::bench`.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und C-ABI; 9.5 fuehrt Treiber in der TCB")]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, Generated, WfiSleep, cycles, stack};
use takt_rt_baremetal::Sleep;
use takt_rt_baremetal::bench::{series, subnormal_failures, write_series, write_value};
use takt_rt_core::{Program, tick_end};

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

/// Durchlaeufe vor der Messung: Caches und Vorabrufpuffer des Flash
/// fuellen sich, und die Initialisierung der Maschinen ist vorbei.
const WARMUP: u32 = 8;

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

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

#[entry]
fn main() -> ! {
    // Zuerst: Was spaeter an Stack gebraucht wird, soll auf dem Muster landen.
    stack::paint();
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let board = Board::WEACT_BLACKPILL;
    let Ok(_timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, takt::TICK_NS) else {
        halt();
    };
    let Ok(mut uart) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        halt();
    };
    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    // Nur die Leitung: Der Timer tickt, aber niemand wartet auf ihn.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1) };

    uart.newline();
    uart.write("takt bench stm32f401");
    uart.newline();
    write_value(&mut uart, "core_hz", u64::from(CORE_HZ));

    let runs = RUNS.and_then(|r| r.parse().ok()).unwrap_or(1000);
    let mut program = Generated::init(false);
    let mut k = 0u64;
    for _ in 0..WARMUP {
        program.tick(k, tick_end(k, takt::TICK_NS));
        k += 1;
    }
    let takt = cortex_m::interrupt::free(|_| {
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
        let c = cortex_m::interrupt::free(|_| series(runs, cycles::now, || unsafe { takt_bench_reference() }));
        // SAFETY: liest den Zustand der Referenz.
        write_series(&mut uart, "c", &c, unsafe { takt_bench_reference_digest() });
    }

    write_value(&mut uart, "stack", u64::from(stack::high_water()));
    write_value(&mut uart, "subnormal", u64::from(subnormal_failures()));
    uart.write("takt end");
    uart.newline();
    uart.drain(takt_rt_baremetal::DRAIN_ROUNDS);

    // Danach bleibt die Leitung offen: Der Host holt das Board mit `TAKT`
    // fuer den naechsten Kern zurueck (FB-275).
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        uart.flush();
    }
}

/// Ohne Takt oder Leitung gibt es nichts zu messen und niemanden, dem es
/// zu sagen waere.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
