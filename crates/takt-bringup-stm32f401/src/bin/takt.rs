//! Ein Takt-Programm auf der MCU (M5, 12.1, 12.3).
//!
//! **Das erste Programm dieser Reihe, das wirklich Takt ausfuehrt.**
//! `blink` schaltet einen Pin, `minimal` prueft die Tickquelle — beide
//! sind Rust. Dieses hier fuehrt den Code aus, den `takt-llvm` aus einer
//! `.takt`-Datei erzeugt hat, unter der Tickschleife aus
//! `takt-rt-core`.
//!
//! ## Wie es entsteht
//!
//! Drei Teile werden gebunden:
//!
//! 1. **Der erzeugte Code** — `takt-llvm` uebersetzt das Programm nach
//!    LLVM-IR, clang macht daraus ein Objekt fuer `thumbv7em`.
//! 2. **Der Rahmen** — `takt-conformance::mcu` erzeugt das C-Stueck, das
//!    Prozessabbild und Latch haelt und `<maschine>_step` ruft.
//! 3. **Dieses Programm** — es setzt das Board auf, liefert die
//!    Telemetriefunktionen und laesst die Schleife laufen.
//!
//! `tools/takt-on-board.sh` macht alle drei Schritte.
//!
//! ## Was man sieht
//!
//! Auf USART1 (PA9, 115200 8N1) erscheint, was der Latch enthaelt —
//! dieselben `out`-Zeilen, die der Interpreter schreibt
//! (`grammar/trace.md`). Damit laesst sich der Hardwarelauf gegen
//! `takt sim` halten, und das ist der Kern des M5-Exits.
//!
//! **Die LED folgt dem Ausgang `led` des Programms**, nicht einem eigenen
//! Zaehler. Das ist der Unterschied zwischen einer Demonstration und einem
//! Ziel: Blinkt sie, dann weil eine Takt-Maschine den Zustand gewechselt
//! hat. Eine frueher Fassung liess sie unabhaengig vom Latch blinken, und
//! damit sagte sie ueber das Programm genau nichts.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und C-ABI; 9.5 fuehrt Treiber in der TCB")]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{Board, CORE_HZ, Generated, Led, Telemetry, cycles, tick};
use takt_rt_baremetal::TickSource;
use takt_rt_core::Program;

/// Die Konstanten des uebersetzten Programms (`takt build --emit consts-rs`).
///
/// **Sie stehen nicht hier, weil sie schon woanders stehen.** Die
/// Tickperiode gehoert in `system: tick`, die Ausgangsindizes in die
/// Kanalreihenfolge; beides von Hand nachzuschreiben war genau der Fehler,
/// der die logische Zeit einmal zehnfach zu schnell laufen liess — das
/// Programm sagte 10 ms, dieses Modul 1 ms, und niemand kannte beides.
mod takt {
    // Nicht jedes Programm braucht jede Konstante; `OUTPUTS` etwa nutzt
    // nur, wer ueber alle Ausgaenge laeuft.
    #![allow(dead_code)]

    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}

use takt::{OUT_LED, TICK_NS};

/// Wie viele Ticks zwischen zwei Trace-Zeilen liegen.
///
/// Nicht je Tick: Eine Zeile ueber UART dauert bei 115200 Baud rund
/// 1,7 ms, laenger als die Tickperiode. 12.8 nennt `states` als
/// Instrumentierungs-Default fuer `baremetal`, nicht `statements`.
const TRACE_EVERY: u64 = 100;

/// Der DWT-Stand beim vorigen Interrupt.
static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Die Telemetrie, die der erzeugte Rahmen ruft.
///
/// **Sie steht hier und nicht im Board-Crate**, weil der Rahmen sie als
/// C-Symbol erwartet und das Board nicht weiss, wohin ein Trace gehen
/// soll — USART, Ringpuffer oder nirgendwohin.
static mut UART: Option<Telemetry> = None;

/// Schreibt eine Zeichenkette (vom Rahmen gerufen).
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Zeiger auf statischen
/// Text; er stammt aus dem erzeugten C-Code und lebt so lange wie das
/// Programm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    let mut p = text;
    // Eine Schranke statt `while *p != 0`: Ein Zeiger ohne Null waere
    // sonst eine Endlosschleife, und die Regel aus 4.1 gilt hier von
    // Hand, weil die TCB sie nicht geschenkt bekommt.
    for _ in 0..256 {
        let b = unsafe { *p };
        if b == 0 {
            return;
        }
        uart.write_byte(b);
        p = unsafe { p.add(1) };
    }
}

/// Schreibt eine Zahl mit Trennzeichen (vom Rahmen gerufen).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    uart.write_i64(value);
    uart.write_byte(b' ');
}

#[interrupt]
fn TIM2() {
    let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };
    let now = cycles::now();
    let elapsed = now.wrapping_sub(LAST_STAMP.swap(now, Ordering::Relaxed));
    // `rc_w0`: Nullen loeschen, Einsen lassen stehen.
    tim2.sr().write(|w| unsafe { w.bits(!1) });
    tick::on_timer_interrupt(elapsed);
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    let Ok(mut clock) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        // Ohne Takt keine Telemetrie: Die LED bleibt an.
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };

    let Ok(uart) = Telemetry::new(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, 115_200) else {
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };
    // Erst jetzt sichtbar machen: Ein Trace vor der Einrichtung schriebe
    // in ein nicht konfiguriertes Register.
    unsafe { UART = Some(uart) };

    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2) };

    banner(clock.nominal_ns());

    let mut program = Generated::init(false);
    let mut next_trace = TRACE_EVERY;

    loop {
        clock.wait_for_tick();
        let k = clock.ticks();
        program.tick(k, k as i64 * TICK_NS);

        // **Schritt 10: der Latch geht nach draussen** (12.1). Erst hier
        // wird aus dem gerechneten Zustand eine Wirkung. `led` ist
        // `active low`, darum die Umkehrung — das weiss das Board, nicht
        // das Programm.
        if program.output(OUT_LED) != 0 {
            led.on();
        } else {
            led.off();
        }

        if k >= next_trace {
            next_trace = k + TRACE_EVERY;
            program.dump();
        }
    }
}

/// Was beim Start feststeht.
fn banner(nominal_ns: i64) {
    let Some(uart) = (unsafe { (*&raw mut UART).as_mut() }) else { return };
    uart.newline();
    uart.write("takt auf stm32f401");
    uart.newline();
    uart.write("  Kerntakt ");
    uart.write_u64(u64::from(CORE_HZ));
    uart.write(" Hz, Tick ");
    uart.write_i64(nominal_ns);
    uart.write(" ns");
    uart.newline();
    uart.write("  DWT ");
    uart.write(if cycles::running() { "laeuft" } else { "STEHT" });
    uart.newline();
    uart.newline();
}
