//! `takt bench` auf der Black Pill (13.8): das Messprogramm als Baustein,
//! wie jeder Wirt es bindet (`takt bench --emit embed`, plan/m11.md 2.12).
//!
//! Das Board stellt die beiden Haken — den Zyklenzaehler des DWT und die
//! Leitung — und faehrt jeden Schritt bei gesperrten Interrupts: Eine ISR
//! mitten in einer Messung ist Last, nicht Kosten des Programms. Geschrieben
//! wird zwischen den Schritten; `takt bench --import` liest das Protokoll.
//! Die Lage des Programms im Flash nennt `TAKT_BENCH_SHIFT` (FB-367).

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und C-ABI; 9.5 fuehrt Treiber in der TCB")]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, Telemetry, WfiSleep, cycles};
use takt_rt_baremetal::{DRAIN_ROUNDS, Sleep};

/// Das Messprogramm (`TAKT_BENCH_RS`).
mod takt_bench {
    #![allow(dead_code, reason = "die Kennung `SUITE` braucht nur, wer das Protokoll selbst prueft")]
    include!(env!("TAKT_BENCH_RS"));
}

/// Wie oft gemessen wird: `TAKT_TICKS` beim Bau, sonst tausendmal.
const RUNS: Option<&str> = option_env!("TAKT_TICKS");

/// Um wie viele Byte `.text` verschoben liegt (`build.rs`).
const SHIFT: &str = env!("TAKT_BENCH_SHIFT");

/// Die Tickperiode des Timers; das Messprogramm wartet auf keinen Tick.
const TICK_NS: i64 = 1_000_000;

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

/// Der Wirt des Messprogramms.
struct Host {
    uart: Telemetry,
}

impl takt_bench::Host for Host {
    fn cycles(&mut self) -> u32 {
        cycles::now()
    }

    /// Jede Zeile geht ganz an die Leitung, bevor der naechste Schritt die
    /// Interrupts sperrt; sonst liefe der Ring ueber.
    fn put(&mut self, byte: u8) {
        self.uart.write_byte(byte);
        if byte == b'\n' {
            self.uart.flush();
            self.uart.drain(DRAIN_ROUNDS);
        }
    }
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let board = Board::WEACT_BLACKPILL;
    let Ok(_timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        halt();
    };
    let Ok(uart) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        halt();
    };
    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    // Nur die Leitung: Der Timer tickt, aber niemand wartet auf ihn.
    // SAFETY: Der Handler ist oben definiert und teilt nur den FIFO der
    // Leitung, der fuer einen Schreiber und einen Leser gebaut ist.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1) };

    let runs = RUNS.and_then(|r| r.parse().ok()).unwrap_or(1000);
    let mut host = Host { uart };
    takt_bench::begin(&mut host, CORE_HZ);
    for byte in b"bench shift ".iter().chain(SHIFT.as_bytes()).chain(b"\n") {
        takt_bench::Host::put(&mut host, *byte);
    }
    takt_bench::steps(&mut host, runs, |measure| cortex_m::interrupt::free(|_| measure()));
    takt_bench::end(&mut host);

    // Danach bleibt die Leitung offen: Der Host holt das Board mit `TAKT`
    // fuer das naechste Abbild zurueck (FB-275).
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        host.uart.flush();
    }
}

/// Ohne Takt oder Leitung gibt es nichts zu messen und niemanden, dem es
/// zu sagen waere.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
