//! Die kuratierten Natives und die korrekt gerundete Mathematik auf der
//! Black Pill (13.8): je Vektor aus `grammar/takt-native.md` und aus Stufe 2
//! von `grammar/libtaktm.md` das Ergebnis und der Stack-Bedarf jedes
//! Einstiegs aus `takt-native-abi`, den der erzeugte Code ruft (FB-293),
//! bei der Mathematik dazu die Zyklen eines Aufrufs (FB-344).
//!
//! `takt_conformance::natives` vergleicht die Ergebnisse mit dem Wirt —
//! bitgleich ueber die Ziele ist die Bedingung, unter der eine Funktion in
//! der kuratierten Menge bleibt — und den Stack mit der Zusage `stack`
//! (4.5, 12.3); `takt_conformance::math` die Mathematik mit der Norm.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler; 9.5 fuehrt Treiber in der TCB")]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, WfiSleep, cycles, stack};
use takt_native::Native;
use takt_native_abi::measure;
use takt_rt_baremetal::Sleep;
use takt_rt_baremetal::bench::{write_math, write_native};

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

/// Die Tickperiode des Timers; das Messprogramm wartet auf keinen Tick.
const TICK_NS: i64 = 1_000_000;

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    let board = Board::WEACT_BLACKPILL;
    let Ok(_timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        halt();
    };
    let Ok(mut uart) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        halt();
    };
    // SAFETY: Der Handler ist oben definiert und teilt nur den FIFO der
    // Leitung, der fuer einen Schreiber und einen Leser gebaut ist.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1) };

    uart.newline();
    uart.write("takt natives stm32f401");
    uart.newline();
    for (i, (name, inputs)) in vectors::VECTORS.iter().enumerate() {
        let measured =
            Native::by_name(name).and_then(|f| measure::vector(f, inputs, &mut |call| stack::usage_of(WINDOW, call)));
        let Some(run) = measured else { continue };
        write_native(&mut uart, i, run.result(), run.stack, run.others());
        // Die naechste Messung sperrt die Interrupts; was im Ring steht,
        // muss vorher an die Leitung, sonst liefe er ueber.
        uart.drain(takt_rt_baremetal::DRAIN_ROUNDS);
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
        uart.drain(takt_rt_baremetal::DRAIN_ROUNDS);
    }
    uart.write("takt end");
    uart.newline();
    uart.drain(takt_rt_baremetal::DRAIN_ROUNDS);

    // Danach bleibt die Leitung offen: Der Host holt das Board mit `TAKT`
    // fuer das naechste Programm zurueck (FB-275).
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
