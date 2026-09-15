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

    // **`write` mit Maske, nicht `modify`.** `TIMx_SR` ist `rc_w0`:
    // Eine Null loescht, eine Eins laesst stehen. Ein `modify` liest
    // zuerst und schriebe ein Flag, das zwischen Lesen und Schreiben
    // gesetzt wurde, mit einer Null zurueck — es waere weg, ohne dass es
    // jemand gesehen hat. Heute ist nur `UIF` aktiv, aber die
    // Compare-Kanaele fuer `at` (7.5) kommen, und dann verloere dieser
    // Handler still ihre Ereignisse. Dieselbe Falle beschreibt FB-11 fuer
    // die Registerfeld-Zugriffsarten.
    tim2.sr().write(|w| unsafe { w.bits(!1) });
    tick::on_timer_interrupt(elapsed);
}

#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
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
    let Ok(mut uart) = Telemetry::new(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, 115_200) else {
        blink_error(&led, 4);
    };

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
            // **Erst die LED, dann der Bericht.** Die Telemetrie dauert
            // bei 115200 Baud rund zehn Millisekunden und blockiert
            // solange; stuende sie davor, verschoebe sie die Flanke um
            // zehn Ticks. Sichtbar waere das nicht, aber die LED ist hier
            // das Messgeraet, und ein Messgeraet, das auf die Ausgabe
            // wartet, misst die Ausgabe mit.
            led.toggle();
            report(&mut uart, &clock, now);

            // `next_report` folgt der *tatsaechlichen* Tickzahl, nicht
            // dem Sollwert: Nach einem langen Bericht sind schon Ticks
            // vergangen, und ein starres `+= REPORT_EVERY` liefe ihnen
            // hinterher, bis die Bedingung dauernd erfuellt waere — die
            // LED flackerte dann statt zu blinken.
            next_report = now + REPORT_EVERY;
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
