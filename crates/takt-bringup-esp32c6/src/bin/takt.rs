//! Ein Takt-Programm auf dem ESP32-C6 (plan/esp32c6.md Schritte 4 und 6).
//!
//! Die Schleife aus `takt-rt-baremetal` (12.1) ueber dem SYSTIMER-Alarm,
//! das erzeugte Programm hinter `Generated`, das `persist`-Journal in
//! zwei Flash-Sektoren, Schlaf in `idle`-Zustaenden als virtuelle Ticks
//! (9.9), der Trace ueber USB-Serial-JTAG. Welches Programm laeuft, sagt
//! `takt.toml`. Hier steht nur, was das Board ist: Peripherie, Treiber
//! und die Telemetriefunktionen des Rahmens.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{Button, FlashNvm, Generated, Telemetry, Ws2812, route_uart0};
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, NoWatchdog, Sleep, TimerClock};
use takt_rt_core::{Clock, Journal, Loaded, Persist, Policy, Profile, Runtime};

esp_bootloader_esp_idf::esp_app_desc!();

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{LOGIC_HASH, NVM_BLOCKING_NS, OVERRUN_ALERT, PERSIST_BOUND, PERSIST_MIN_INTERVAL_NS, TICK_NS};

/// Alle wie viele Ticks die Ausgaenge im Betrieb ausgegeben werden.
const TRACE_EVERY: u64 = 100;

/// Konformitaetslauf (plan/esp32c6.md 5): `TAKT_TICKS` beim Bau gesetzt
/// heisst jeden Tick ausgeben, nach so vielen Ticks das Journal schreiben,
/// `takt end` und Halt.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

/// Ein Konformitaetslauf zaehlt in logischer Zeit und verliert keine
/// Zeile, auch wenn die Leitung den Trace langsamer nimmt, als der Tick
/// dauert (FB-292, FB-271). `TAKT_TIMED` beim Bau laesst die Uhr laufen,
/// fuer den Tick-Jitter von `takt bench`.
const LOGICAL: bool = TICKS.is_some() && option_env!("TAKT_TIMED").is_none();

/// `TAKT_FRESH_JOURNAL` beim Bau gesetzt: das Journal vor dem Lauf
/// loeschen, damit der Lauf wie der Interpreter ohne Speicher beginnt.
const FRESH_JOURNAL: bool = option_env!("TAKT_FRESH_JOURNAL").is_some();

/// `TAKT_INSTRUMENT=statements`: den Programmzaehler je Tick mitgeben (11.2).
const TRACE_PC: bool = matches!(option_env!("TAKT_INSTRUMENT"), Some(m) if matches!(m.as_bytes(), b"statements"));

/// Das Journal: die `nvs`-Partition des ESP-IDF-Schemas, das `probe-rs`
/// flasht (0x9000, 24 KiB, sonst leer); zwei Sektoren davon.
const JOURNAL_AT: u32 = 0x9000;

static mut UART: Option<Telemetry> = None;
static mut LED: Option<Ws2812> = None;
static mut BTN: Option<Button> = None;

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

/// Vom Rahmen gerufen: eine Fliesskommazahl im Trace, als kuerzeste
/// Ziffernfolge, die den Wert eindeutig zurueckgibt (Bitgleichheit, 4.2).
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

/// Der Output `ui_led` des Programms auf der RGB-LED.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_ui_led(value: u8) {
    let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return };
    if value != 0 {
        led.on();
    } else {
        led.off();
    }
}

/// Der Input `ui_button` aus dem BOOT-Taster an IO9 (12.1 Schritt 2).
///
/// Ein wackelnder Kontakt meldet `Suspect` statt `Good`: Der Wert ist da,
/// aber noch nicht stabil (12.6). Ohne Treiber bliebe der Eintrag `Bad`,
/// und das waere hier falsch — der Taster ist verdrahtet.
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_ui_button(value: *mut u8, quality: *mut u8) -> bool {
    let Some(btn) = (unsafe { (&raw mut BTN).as_mut().and_then(Option::as_mut) }) else { return false };
    let (level, stable) = btn.poll();
    unsafe {
        *value = u8::from(level);
        *quality = if stable { 0 } else { 1 };
    }
    true
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
fn conduct(program: Generated, clock: impl Clock, persist: &mut Option<Persist<'_, FlashNvm>>) {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    let mut rt = Runtime::new(program, clock, NoWatchdog, (), Profile::BAREMETAL, TICK_NS, policy);
    let limit = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    let stats = takt_rt_baremetal::run(&mut rt, persist.as_mut(), Cadence::of(limit, TRACE_EVERY, TRACE_PC), uart);
    let journal = persist.as_ref().map_or(JournalStats::default(), |p| {
        let (erase_ns, program_ns) = p.journal().device().measured_ns();
        JournalStats { writes: p.journal().writes(), failures: p.journal().failures(), erase_ns, program_ns }
    });
    if let Some(u) = uart() {
        let stack = Some(takt_board_esp32c6::stack::high_water());
        takt_rt_baremetal::report(u, rt.overrun(), &stats, &journal, stack);
    }
}

#[main]
fn main() -> ! {
    // Zuerst: Die Abschlusszeile meldet, wie tief der Stack unter Last reichte.
    takt_board_esp32c6::stack::paint();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    let mut telemetry = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    let Ok(timer) = takt_board_esp32c6::init(peripherals.SYSTIMER, TICK_NS) else {
        telemetry.write("takt: Periode nicht einrichtbar");
        telemetry.newline();
        telemetry.drain(DRAIN_ROUNDS);
        loop {
            core::hint::spin_loop();
        }
    };
    telemetry.write("takt esp32c6: tick ");
    telemetry.write_i64(timer.nominal_ns());
    telemetry.write(" ns");
    telemetry.newline();
    telemetry.mark();
    let telemetry = if LOGICAL { telemetry.lossless() } else { telemetry };
    unsafe { UART = Some(telemetry) };
    if let Ok(led) = Ws2812::new(peripherals.RMT, peripherals.GPIO8) {
        unsafe { LED = Some(led) };
    }
    unsafe { BTN = Some(Button::new(peripherals.GPIO9)) };
    // 12.10: Ein `port @ mmio(...)` schreibt Register; die Verbindung zum
    // Pad macht die GPIO-Matrix, nicht der Treiber.
    route_uart0(peripherals.GPIO7, peripherals.GPIO17);

    // 5.9: s0 kommt aus dem Journal, darum laden vor dem ersten Eintritt.
    // Ohne `persist` im Programm gibt es kein Journal und keinen Flash-Zugriff.
    let (mut current, mut stored) = ([0u8; PERSIST_BOUND], [0u8; PERSIST_BOUND]);
    let mut persist = None;
    if PERSIST_BOUND != 0 {
        let mut nvm = FlashNvm::new(peripherals.FLASH, JOURNAL_AT).with_blocking_ns(NVM_BLOCKING_NS);
        if FRESH_JOURNAL && !nvm.wipe() {
            report("journal: loeschen scheiterte");
        }
        persist = Some(Persist::new(Journal::new(nvm, LOGIC_HASH, PERSIST_MIN_INTERVAL_NS), &mut current, &mut stored));
    }
    let mut program = Generated::new(false);
    let loaded = persist.as_mut().map(|p| p.load(&mut program));
    program.ensure_init();
    if let Some(u) = uart() {
        match loaded {
            Some((Loaded::Found { length, sequence }, applied)) => {
                let _ = write!(u, "journal: Eintrag {sequence}, {length} Byte, {applied} Werte geladen\r\n");
            }
            Some((Loaded::Empty, _)) => u.write("journal: leer\r\n"),
            None => u.write("journal: keins\r\n"),
        }
        u.flush();
    }

    if LOGICAL {
        // Zwischen den Ticks leert die Schleife die Leitung ganz; dann
        // steht die Uhr auf der Frist.
        let clock = LogicalClock::new(|| {
            if let Some(u) = uart() {
                u.drain(DRAIN_ROUNDS);
            }
        });
        conduct(program, clock, &mut persist);
    } else {
        let clock = TimerClock::new(timer, TICK_NS).with_idle(|| {
            if let Some(u) = uart() {
                u.flush();
            }
        });
        conduct(program, clock, &mut persist);
    }
    let mut sleep = takt_board_esp32c6::WfiSleep;
    loop {
        sleep.sleep_until_event();
    }
}

/// Eine Zeile ausserhalb des Traces.
fn report(text: &str) {
    if let Some(u) = uart() {
        u.write(text);
        u.newline();
        u.flush();
    }
}
