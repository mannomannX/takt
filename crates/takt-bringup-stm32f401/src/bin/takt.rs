//! Ein Takt-Programm auf der MCU (M5, 12.1, 12.3).
//!
//! **Das einzige Programm dieser Reihe, das wirklich Takt ausfuehrt.**
//! `blink` schaltet einen Pin, `minimal` prueft die Tickquelle, `tick`
//! misst sie — alle drei sind Rust. Dieses hier fuehrt den Code aus, den `takt-llvm` aus einer
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
//! Auf USART1 (PA9, 921600 8N1) erscheint, was der Latch enthaelt —
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

use core::fmt::Write as _;
use core::sync::atomic::{AtomicI32, AtomicU32, Ordering};

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Peripherals, interrupt};
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, Generated, Iwdg, Led, Telemetry, WfiSleep, cycles, platform, tick};
use takt_board_support::platform::image_state;
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, Sleep, TimerClock};
use takt_rt_core::{Clock, FakeNvm, Persist, PlatformCommand, Policy, Profile, Runtime};

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{NVM_BLOCKING_NS, OVERRUN_ALERT, TICK_NS};

/// Die Frist des Watchdogs im Betrieb (12.3): zwei Perioden und ein
/// blockierender NVM-Vorgang (8.10). Ein Tick, der darueber hinaus
/// ueberzieht, ist kein Ueberlauf mehr (7.3), sondern ein Stillstand.
const WATCHDOG_NS: i64 = 2 * TICK_NS + NVM_BLOCKING_NS;

/// Die Frist fuer das geordnete Ende eines Laufs (12.7): Abschlusszeile
/// und Leitung warten je hoechstens `DRAIN_ROUNDS` Anlaeufe auf den Host,
/// zusammen weit unter dieser Frist.
const END_OF_RUN_NS: i64 = 8_000_000_000;

/// Alle wie viele Ticks die Ausgaenge im Betrieb ausgegeben werden: jeden,
/// wenn eine Zeile ein Zehntel der Periode fuellt, sonst jeden hundertsten.
const TRACE_EVERY: u64 = {
    const CHARS: u64 = 32;
    const NS_PER_LINE: u64 = CHARS * 10 * 1_000_000_000 / BAUD as u64;
    if TICK_NS as u64 >= NS_PER_LINE * 10 { 1 } else { 100 }
};

/// Konformitaetslauf: `TAKT_TICKS` beim Bau gesetzt heisst jeden Tick
/// ausgeben und nach so vielen Ticks `takt end`.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

/// Ein Konformitaetslauf zaehlt in logischer Zeit und verliert keine
/// Zeile, auch wenn die Leitung den Trace langsamer nimmt, als der Tick
/// dauert (FB-292). `TAKT_TIMED` beim Bau laesst die Uhr laufen, fuer den
/// Tick-Jitter von `takt bench`.
const LOGICAL: bool = TICKS.is_some() && option_env!("TAKT_TIMED").is_none();

/// `TAKT_INSTRUMENT=statements`: den Programmzaehler je Tick mitgeben (11.2).
const TRACE_PC: bool = matches!(option_env!("TAKT_INSTRUMENT"), Some(m) if matches!(m.as_bytes(), b"statements"));

static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Die Telemetrie, statisch: Der erzeugte Rahmen ruft `takt_board_trace`
/// als C-Symbol, und eine Funktion ohne Empfaenger kommt an nichts heran,
/// was in `main` liegt.
static mut UART: Option<Telemetry> = None;

/// Die LED, die der Treiber `takt_out_ui_led` schaltet; aus demselben Grund.
static mut LED: Option<Led> = None;

fn uart() -> Option<&'static mut Telemetry> {
    unsafe { (&raw mut UART).as_mut().and_then(Option::as_mut) }
}

/// Vom Rahmen gerufen: eine Zeile Trace, nullterminiert.
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Zeiger auf statischen
/// Text; die Schranke haelt einen Zeiger ohne Null auf (4.1, von Hand).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(uart) = uart() else { return };
    let mut p = text;
    for _ in 0..256 {
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
    let Some(uart) = uart() else { return };
    uart.write_i64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    let Some(uart) = uart() else { return };
    uart.write_u64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige Ziffernfolge (4.2).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    let Some(uart) = uart() else { return };
    let _ = write!(uart, "{value:?} ");
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    let Some(uart) = uart() else { return };
    uart.write_hex8(value);
}

/// Womit dieser Lauf begann (12.7), beim Start aus der Reset-Ursache gelesen.
static BOOT_REASON: AtomicI32 = AtomicI32::new(0);

/// Der Treiber fuer `input … @ hw("sys/boot_reason")` (12.7).
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_boot_reason(value: *mut i32, quality: *mut u8) -> bool {
    unsafe {
        *value = BOOT_REASON.load(Ordering::Relaxed);
        *quality = 0;
    }
    true
}

/// Die Starts in Folge ohne geordnetes Ende (12.7), beim Start gezaehlt.
static RESET_COUNT: AtomicU32 = AtomicU32::new(0);

/// Der Treiber fuer `input … @ hw("sys/reset_count")` (12.7).
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild; `int`
/// liegt dort als `long long`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_reset_count(value: *mut i64, quality: *mut u8) -> bool {
    unsafe {
        *value = i64::from(RESET_COUNT.load(Ordering::Relaxed));
        *quality = 0;
    }
    true
}

/// Der Treiber fuer `input … @ hw("sys/image_state")` (12.7): Ohne
/// Startstufe gibt es ein Image, und es ist bestaetigt.
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_image_state(value: *mut i32, quality: *mut u8) -> bool {
    unsafe {
        *value = image_state::CONFIRMED;
        *quality = 0;
    }
    true
}

/// Der Treiber fuer `output led : bool @ hw("ui/led")`.
///
/// Der Name ist die Adresse: Der Rahmen bildet `hw("ui/led")` auf
/// `takt_out_ui_led` ab und ruft es in Schritt 10 (12.1); wer es nicht
/// stellt, bekommt einen Linkfehler mit diesem Namen (8.10). Die LED der
/// Black Pill liegt an PC13 gegen 3V3 — was `true` elektrisch heisst,
/// weiss nur diese Zeile.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_ui_led(value: u8) {
    let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return };
    if value != 0 {
        led.on();
    } else {
        led.off();
    }
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

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[interrupt]
fn USART1() {
    takt_board_stm32f401::uart::on_interrupt();
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
fn conduct(clock: impl Clock) {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    let limit = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit == 0).then(|| Iwdg::arm(WATCHDOG_NS));
    let mut rt = Runtime::new(Generated::init(false), clock, watchdog, (), Profile::BAREMETAL, TICK_NS, policy);
    // Kein Journal: Das Board hat noch keinen `Nvm`-Treiber (5.9).
    let no_journal = None::<&mut Persist<'_, FakeNvm<0>>>;
    let stats = takt_rt_baremetal::run(&mut rt, no_journal, Cadence::of(limit, TRACE_EVERY, TRACE_PC), uart);
    if rt.watchdog.is_some() {
        Iwdg::arm(END_OF_RUN_NS);
    }
    if let Some(u) = uart() {
        let stack = Some(takt_board_stm32f401::stack::high_water());
        takt_rt_baremetal::report(u, rt.overrun(), &stats, &JournalStats::default(), stack);
    }
    if limit == 0 {
        platform(stats.command);
    }
}

/// Fuehrt ein Kommando an die Plattform aus (12.7).
///
/// Ein Konformitaetslauf endet wie der Wirtsrahmen mit dem Trace, und das
/// Board bleibt fuer das naechste Programm erreichbar; nur im Betrieb
/// fuehrt es das Kommando aus. Vorher geht die Leitung ganz hinaus: Reset
/// und Tiefschlaf naehmen mit, was noch in ihrem Puffer steht (FB-314).
fn platform(command: Option<PlatformCommand>) {
    let Some(command) = command else { return };
    if let Some(u) = uart() {
        u.finish(DRAIN_ROUNDS);
    }
    match command {
        PlatformCommand::Restart => platform::restart(),
        PlatformCommand::DeepSleep(duration) => platform::deep_sleep(duration),
        // TODO(M10 Schritt 17): Der Sprung braucht Slots, die erst das
        // Profil `boot` einrichtet; bis dahin haelt das Board mit
        // `safe`-Ausgaengen an und sagt es.
        PlatformCommand::Jump(_) => {
            if let Some(u) = uart() {
                u.write("takt: der Sprung in einen Slot braucht das Profil `boot`");
                u.newline();
                u.drain(DRAIN_ROUNDS);
            }
        }
    }
}

#[entry]
fn main() -> ! {
    // Ein Tiefschlaf mit Rest schlaeft weiter, bevor irgendetwas laeuft (12.7).
    platform::continue_deep_sleep();
    let boot_reason = platform::boot_reason();
    BOOT_REASON.store(boot_reason, Ordering::Relaxed);
    RESET_COUNT.store(platform::reset_count(boot_reason), Ordering::Relaxed);
    // Dann: Die Abschlusszeile meldet, wie tief der Stack unter Last reichte.
    takt_board_stm32f401::stack::paint();
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    let Ok(timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        // Ohne Takt keine Telemetrie: Die LED bleibt an.
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };
    let Ok(telemetry) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };
    let telemetry = if LOGICAL { telemetry.lossless() } else { telemetry };
    // Erst jetzt sichtbar machen: Ein Trace vor der Einrichtung schriebe
    // in ein nicht konfiguriertes Register.
    unsafe { UART = Some(telemetry) };

    let (mut dcb, mut dwt) = (cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    unsafe {
        cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::TIM2);
        cortex_m::peripheral::NVIC::unmask(stm32f4::stm32f401::Interrupt::USART1);
    }

    banner(timer.nominal_ns());
    unsafe { LED = Some(led) };

    if LOGICAL {
        // Zwischen den Ticks leert die Schleife die Leitung ganz; dann
        // steht die Uhr auf der Frist.
        conduct(LogicalClock::new(|| {
            if let Some(u) = uart() {
                u.drain(DRAIN_ROUNDS);
            }
        }));
    } else {
        // Zwischen den Ticks fuellt die Schleife die Leitung nach, sooft ein
        // Interrupt den Kern weckt: Sie nimmt nur ab, was in ihren FIFO passt.
        conduct(TimerClock::new(timer, TICK_NS).with_idle(|| {
            if let Some(u) = uart() {
                u.flush();
            }
        }));
    }
    // Nach dem Lauf bleibt die Leitung offen: Der Host holt das Board mit
    // `TAKT` zurueck, um das naechste Programm zu schreiben (FB-275).
    // Der Watchdog laeuft nach einem Lauf im Betrieb weiter; das Board
    // haelt an, statt neu zu starten.
    let mut sleep = WfiSleep;
    loop {
        sleep.sleep_until_event();
        if let Some(u) = uart() {
            u.flush();
        }
        Iwdg::feed();
    }
}

/// Was beim Start feststeht.
fn banner(nominal_ns: i64) {
    let Some(uart) = uart() else { return };
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
    uart.mark();
    uart.flush();
}
