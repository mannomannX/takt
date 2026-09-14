//! Bring-up der Tickquelle auf dem STM32F401 (plan/m5.md, Schritt 4).
//!
//! **Was dieses Programm beantwortet.** Laeuft der Tick? Mit welcher
//! Periode? Und stimmt sie mit dem ueberein, was der Compiler annimmt
//! (7.1: „`tick` bleibt der nominale Wert der Semantik; die gemessene
//! Periode wird aufgezeichnet")? Das ist die erste Frage, die auf echter
//! Hardware zu stellen ist — alles Weitere von M5 setzt sie voraus.
//!
//! Es ist **kein Takt-Programm**: Es fuehrt keine Maschine aus, sondern
//! prueft die Schicht darunter. Der erzeugte Code kommt spaeter, wenn
//! `takt-llvm` fuer `thumbv7em` erzeugt und die Runtime ihn ruft.
//!
//! ## Was man sieht
//!
//! - **Die LED blinkt** im Sekundentakt. Steht sie, haengt das Programm;
//!   blinkt sie falsch, stimmt die Periode nicht. Das sieht man aus drei
//!   Metern, ohne Werkzeug.
//! - **USART1 (PA9, 115200 8N1)** meldet je Sekunde Tickzahl, gemessene
//!   Periode und die Abweichung von der nominalen.
//! - **Der DWT** misst eine feste Rechenschleife. Die Zahl ist der erste
//!   Datenpunkt fuer `c_target` (13.8) — noch keine Kalibrierung, aber
//!   der Beleg, dass der Zaehler laeuft und plausible Werte liefert.
//!
//! ## Flashen
//!
//! Ueber den HID-Bootloader des Boards (kein Debugger noetig):
//!
//! ```text
//! cargo build --release
//! llvm-objcopy -O binary target/thumbv7em-none-eabihf/release/takt-bringup-stm32f401 app.bin
//! hid-flash app.bin
//! ```
//!
//! Mit SWD-Probe stattdessen `probe-rs run --chip STM32F401CCUx`; dann
//! ist auch `memory.x` auf `0x0800_0000` zu setzen, weil der Bootloader
//! nicht mehr im Weg steht.

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und Registerzugriff; 9.5 fuehrt Treiber in der TCB")]

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{Board, CORE_HZ, Led, TIMER_HZ, Telemetry, cycles, tick};
use takt_board_support::Measurement;
use takt_rt_baremetal::{Period, TickSource};

/// Die Tickperiode, die `system: tick = 1 ms` entspraeche.
const TICK_NS: i64 = 1_000_000;

/// Wie viele Ticks zwischen zwei Meldungen liegen.
const REPORT_EVERY: u64 = 1_000;

/// Der DWT-Stand beim vorigen Interrupt.
///
/// Aus der Differenz zweier Ablesungen entsteht die gemessene Periode
/// (7.1, `tick_tolerance`).
static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Der Tick-Interrupt (12.3: „die ISR setzt ein Flag").
///
/// Sie tut so wenig wie moeglich: Flag loeschen, Zaehler erhoehen. Alles
/// Weitere gehoert in die Hauptschleife, damit ein langer Schritt als
/// Overrun sichtbar wird statt die naechste ISR zu verzoegern.
#[interrupt]
fn TIM2() {
    // Sicher, weil die ISR die einzige Stelle ist, die TIM2 nach dem
    // Start noch anfasst — `Peripherals::take` hat den Rest schon
    // vergeben.
    let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };

    // **Gemessen wird ueber den DWT, nicht ueber den Timer selbst.** Der
    // Timerzaehler steht beim Update-Interrupt immer nahe null — er ist
    // ja gerade uebergelaufen. Was die Periode wirklich sagt, ist der
    // Abstand zweier Interrupts, und den misst der Zyklenzaehler des
    // Kerns: Er laeuft unabhaengig vom Timer und mit `CORE_HZ`.
    let now = cycles::now();
    let elapsed = now.wrapping_sub(LAST_STAMP.swap(now, Ordering::Relaxed));

    tim2.sr().modify(|_, w| w.uif().clear_bit());
    tick::on_timer_interrupt(elapsed);
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let board = Board::WEACT_BLACKPILL;

    let mut clock = match takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.TIM2, TICK_NS) {
        Ok(c) => c,
        // Ohne Takt gibt es keine Telemetrie, mit der man es melden
        // koennte — also bleibt die LED als einziges Signal. Sie an zu
        // lassen heisst: Der Start ist gescheitert.
        Err(_) => {
            let led = Led::new(dp.GPIOC, &dp.RCC, board);
            led.on();
            loop {
                cortex_m::asm::wfi();
            }
        }
    };

    let led = Led::new(dp.GPIOC, &dp.RCC, board);
    let mut uart = Telemetry::new(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, 115_200).expect("115200 Baud bei 84 MHz");

    // Der Zyklenzaehler (12.3, 13.8). `enable` braucht beide: Ohne den
    // Debug-Block bleibt der DWT stumm auf null stehen.
    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);

    // Den Tick-Interrupt freigeben — ohne ihn zaehlt niemand.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2) };

    banner(&mut uart, &clock);

    let mut next_report = REPORT_EVERY;
    loop {
        clock.wait_for_tick();
        let now = clock.ticks();

        if now >= next_report {
            next_report += REPORT_EVERY;
            led.toggle();
            report(&mut uart, &clock, now);
        }
    }
}

/// Was beim Start feststeht.
fn banner(uart: &mut Telemetry, clock: &takt_board_stm32f401::Tim2Tick) {
    uart.newline();
    uart.write("takt bring-up stm32f401");
    uart.newline();
    uart.write("  Kerntakt      ");
    uart.write_u64(u64::from(CORE_HZ));
    uart.write(" Hz");
    uart.newline();
    uart.write("  Timertakt     ");
    uart.write_u64(u64::from(TIMER_HZ));
    uart.write(" Hz");
    uart.newline();
    uart.write("  Tick nominal  ");
    uart.write_i64(clock.nominal_ns());
    uart.write(" ns");
    uart.newline();
    uart.write("  DWT           ");
    uart.write(if cycles::running() { "laeuft" } else { "STEHT — Messungen sind wertlos" });
    uart.newline();
    uart.newline();
}

/// Ein Bericht je Sekunde.
///
/// Drei Zahlen, jede mit einem Zweck: die Tickzahl belegt, dass der
/// Zaehler laeuft; die gemessene Periode gegen die nominale ist der
/// Kern von `tick_tolerance` (7.1); die Zyklen einer festen Schleife
/// sind der erste Datenpunkt fuer `c_target` (13.8).
fn report(uart: &mut Telemetry, clock: &takt_board_stm32f401::Tim2Tick, ticks: u64) {
    let period = Period { nominal_ns: clock.nominal_ns(), measured_ns: clock.last_period_ns() };

    uart.write("t=");
    uart.write_u64(ticks);
    uart.write("  Periode ");
    uart.write_i64(period.measured_ns);
    uart.write(" ns (nominal ");
    uart.write_i64(period.nominal_ns);
    uart.write(", Abweichung ");
    uart.write_u64(period.deviation_ns());
    uart.write(" ns");
    if period.out_of_tolerance(2) {
        uart.write(" — UEBER 2 pct, das waere Runtime(Hardware) nach 10 Ticks");
    }
    uart.write(")  Messschleife ");
    uart.write_i64(measure_reference().ns());
    uart.write(" ns");
    uart.newline();
}

/// Eine feste Rechenschleife, in Zyklen gemessen.
///
/// **Der Vorlaeufer von `takt bench`** (13.8). Noch keine Kalibrierung —
/// dafuer braucht es die Referenzkerne aus 13.8 und eine Messreihe. Was
/// diese Schleife zeigt, ist, dass der DWT laeuft und plausible Werte
/// liefert: Etwa 1000 Additionen bei 84 MHz sollten im Bereich einiger
/// Mikrosekunden liegen.
///
/// `black_box` haelt den Optimierer davon ab, die Schleife wegzurechnen —
/// ohne ihn misst man eine leere Funktion.
fn measure_reference() -> Measurement {
    cycles::measure(CORE_HZ, || {
        let mut acc: u32 = 0;
        for i in 0..1000u32 {
            acc = core::hint::black_box(acc.wrapping_add(i));
        }
    })
}
