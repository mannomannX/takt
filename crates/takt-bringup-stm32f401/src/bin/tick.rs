//! Die Tickquelle messen (plan/m5.md, Schritt 4; Vorlaeufer von 13.8).
//!
//! **Was dieses Programm beantwortet.** Laeuft der Tick? Mit welcher
//! Periode? Und stimmt sie mit dem ueberein, was der Compiler annimmt
//! (7.1: „`tick` bleibt der nominale Wert der Semantik; die gemessene
//! Periode wird aufgezeichnet")? Das ist die erste Frage, die auf echter
//! Hardware zu stellen ist — alles Weitere von M5 setzt sie voraus.
//!
//! **Wozu es neben `minimal` bleibt.** `minimal` beantwortet eine Frage
//! mit der LED und kommt ohne UART und DWT aus; dieses Programm misst und
//! berichtet. Es ist damit der einzige Nutzer von `cycles::measure` —
//! derselben Messung, aus der `takt bench` die Kostentabelle `c_target`
//! gewinnt (13.8). Wer sie entfernt, muesste sie fuer die Kalibrierung neu
//! schreiben.
//!
//! Es hiess frueher `main.rs` und sah dadurch aus wie das Hauptprogramm
//! des Crates. Es ist eines von vier gleichrangigen: `blink` schaltet
//! einen Pin, `minimal` prueft den Tick, `tick` misst ihn, `takt` fuehrt
//! ein Takt-Programm aus.
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
//! - **USART1 (PA9, 921600 8N1)** meldet je Sekunde Tickzahl, gemessene
//!   Periode und die Abweichung von der nominalen.
//! - **Der DWT** misst eine feste Rechenschleife. Die Zahl ist der erste
//!   Datenpunkt fuer `c_target` (13.8) — noch keine Kalibrierung, aber
//!   der Beleg, dass der Zaehler laeuft und plausible Werte liefert.
//!
//! ## Flashen
//!
//! Ueber den DFU-Bootloader des ROM nach Sektor 4, hinter Sprungbrett und
//! Journal (`memory.x`; kein Debugger noetig):
//!
//! ```text
//! cargo build --release --bin tick
//! llvm-objcopy -O binary target/thumbv7em-none-eabihf/release/tick app.bin
//! dfu-util -a 0 -s 0x08010000:leave -D app.bin
//! ```

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und Registerzugriff; 9.5 fuehrt Treiber in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "Stufe 2 des Regelwerks, mit `unsafe` (plan/cert.md Abschnitt 6)"
    )
)]

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

/// Der Tick-Interrupt (12.3): Er weckt an der Grenze, misst ihren Abstand
/// zur vorigen am DWT und stellt die naechste; die Zahl der Ticks steht in
/// der Zeitachse von TIM2, nicht in ihm (FB-742). Alles Weitere gehoert in
/// die Hauptschleife, damit ein langer Schritt als Overrun sichtbar wird
/// statt die naechste ISR zu verzoegern.
#[interrupt]
fn TIM2() {
    tick::on_interrupt();
}

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

#[entry]
fn main() -> ! {
    // `take` gibt die Peripherie beim ersten Aufruf; ohne sie gibt es weder
    // LED noch Leitung, und der Kern schlaeft.
    let (Some(dp), Some(cp)) = (Peripherals::take(), cortex_m::Peripherals::take()) else {
        loop {
            cortex_m::asm::wfi();
        }
    };
    let board = Board::WEACT_BLACKPILL;

    // Die LED zuerst: Sie ist das einzige Anzeigegeraet, das ohne Takt
    // und ohne Telemetrie funktioniert, und darum das erste, was steht.
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    let mut clock = match takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) {
        Ok(c) => c,
        Err(e) => blink_error(&led, error_code(e)),
    };

    // **Kein `expect` hier.** Ein Panic haelt an, und `panic-halt` laesst
    // die LED stehen, wo sie gerade war — ein Zustand, der wie „laeuft"
    // aussehen kann. Ein Fehlercode blinkt stattdessen.
    let Ok(mut uart) =
        takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, takt_board_stm32f401::BAUD)
    else {
        blink_error(&led, 4);
    };

    // Der Zyklenzaehler (12.3, 13.8). `enable` braucht beide: Ohne den
    // Debug-Block bleibt der DWT stumm auf null stehen.
    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);

    // Den Tick-Interrupt freigeben — ohne ihn zaehlt niemand.
    // SAFETY: Der Handler oben ist bereit und teilt nur den Zaehler der Ticks.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2) };
    // SAFETY: Der Handler der Leitung teilt nur ihren FIFO, der fuer einen
    // Schreiber und einen Leser gebaut ist.
    unsafe { cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1) };

    banner(&mut uart, &clock);
    // Die Taktregister roh: Ein Takt, der nicht der konfigurierte ist,
    // faellt sonst nicht auf, weil Timer und Sollwert an derselben Uhr
    // haengen.
    for (name, v) in [
        ("RCC_CR     ", dp.RCC.cr().read().bits()),
        ("RCC_CFGR   ", dp.RCC.cfgr().read().bits()),
        ("RCC_PLLCFGR", dp.RCC.pllcfgr().read().bits()),
    ] {
        uart.write("  ");
        uart.write(name);
        for b in v.to_be_bytes() {
            uart.write(" ");
            uart.write_hex8(b);
        }
        uart.newline();
        uart.flush();
    }

    let mut next_report = REPORT_EVERY;
    loop {
        clock.wait_for_tick();
        // Je Tick einmal: Die Leitung fuehrt einen Wunsch des Hosts aus.
        uart.flush();
        let now = clock.ticks();

        if now >= next_report {
            // **Erst die LED, dann der Bericht.** Die Telemetrie dauert
            // rund eine Millisekunde und blockiert solange; stuende sie
            // davor, verschoebe sie die Flanke um einen Tick. Sichtbar
            // waere das nicht, aber die LED ist hier das Messgeraet, und
            // ein Messgeraet, das auf die Ausgabe wartet, misst die
            // Ausgabe mit.
            led.toggle();
            report(&mut uart, &clock, now);

            // `next_report` folgt der *tatsaechlichen* Tickzahl, nicht
            // dem Sollwert: Nach einem langen Bericht sind schon Ticks
            // vergangen, und ein starres `+= REPORT_EVERY` liefe ihnen
            // hinterher, bis die Bedingung dauernd erfuellt waere — die
            // LED flackerte dann statt zu blinken.
            next_report = now.saturating_add(REPORT_EVERY);
        }
    }
}

/// Der Fehlercode einer gescheiterten Initialisierung.
///
/// Eine Zahl, die man blinken kann — mehr braucht es nicht, und mehr
/// geht auch nicht, solange weder Takt noch Telemetrie stehen.
fn error_code(e: takt_board_stm32f401::InitError) -> u8 {
    use takt_board_stm32f401::InitError;
    match e {
        InitError::Period(_) => 1,
        InitError::ClockNotReady => 2,
        InitError::UnsupportedCrystal => 3,
    }
}

/// Blinkt einen Fehlercode und kehrt nicht zurueck.
///
/// **Warum nicht Dauerlicht.** Eine LED, die einfach an ist, sagt „etwas
/// ist schiefgegangen" und sonst nichts — und sie ist von einem
/// haengenden Programm nicht zu unterscheiden. `n` kurze Blitze, dann
/// eine Pause, sagt *welcher* Fehler. Das ist die einzige Diagnose, die
/// ohne Debugger und ohne Telemetrie funktioniert.
///
/// Die Zeitbasis ist eine Zaehlschleife, keine Uhr: Wer hier ankommt,
/// hat keinen verlaesslichen Takt. Die Blitze sind darum ungenau, aber
/// zaehlbar — und das genuegt.
fn blink_error(led: &Led, code: u8) -> ! {
    // Grob bemessen fuer den HSI-Startakt (16 MHz); mit PLL ist es
    // schneller, bleibt aber erkennbar.
    const SHORT: u32 = 400_000;
    const LONG: u32 = 3_000_000;
    loop {
        for _ in 0..code {
            led.on();
            spin(SHORT);
            led.off();
            spin(SHORT);
        }
        spin(LONG);
    }
}

/// Eine Warteschleife, die der Optimierer nicht wegrechnet.
fn spin(n: u32) {
    for _ in 0..n {
        core::hint::black_box(());
    }
}

/// Was beim Start feststeht.
fn banner(uart: &mut Telemetry, clock: &takt_board_stm32f401::Tim2Tick) {
    uart.newline();
    uart.flush();
    uart.write("takt bring-up stm32f401");
    uart.newline();
    uart.flush();
    uart.write("  Kerntakt      ");
    uart.write_u64(u64::from(CORE_HZ));
    uart.write(" Hz");
    uart.newline();
    uart.flush();
    uart.write("  Timertakt     ");
    uart.write_u64(u64::from(TIMER_HZ));
    uart.write(" Hz");
    uart.newline();
    uart.flush();
    uart.write("  Tick nominal  ");
    uart.write_i64(clock.nominal_ns());
    uart.write(" ns");
    uart.newline();
    uart.flush();
    uart.write("  DWT           ");
    uart.write(if cycles::running() { "laeuft" } else { "STEHT — Messungen sind wertlos" });
    uart.newline();
    uart.flush();
    uart.newline();
    uart.flush();
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
    uart.flush();
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
