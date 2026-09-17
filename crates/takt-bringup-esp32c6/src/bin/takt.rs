//! Ein Takt-Programm auf dem ESP32-C6 (plan/esp32c6.md Schritte 4 und 6).
//!
//! Die Schleife aus `takt-rt-core` (12.1) ueber dem SYSTIMER-Alarm, das
//! erzeugte Programm hinter `Generated`, das `persist`-Journal in zwei
//! Flash-Sektoren, Schlaf in `idle`-Zustaenden als virtuelle Ticks (9.9),
//! der Trace ueber USB-Serial-JTAG. Welches Programm laeuft, sagt
//! `takt.toml`.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{FlashNvm, Generated, Telemetry, Ws2812};
use takt_rt_baremetal::Sleep;
use takt_rt_core::{Journal, Loaded, Persist, Policy, Profile, Runtime, Sink, Tick, Watchdog};

esp_bootloader_esp_idf::esp_app_desc!();

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{LOGIC_HASH, NVM_BLOCKING_NS, OVERRUN_ALERT, PERSIST_BOUND, PERSIST_MIN_INTERVAL_NS, TICK_NS};

/// Alle wie viele Ticks der Zustand ausgegeben wird. USB-Serial-JTAG ist
/// schnell, aber das FIFO blockiert, wenn der Host nicht liest; ein
/// Abzug je 100 Ticks haelt den Tick frei.
const TRACE_EVERY: u64 = 100;

/// Konformitaetslauf (plan/esp32c6.md 5): `TAKT_TICKS` beim Bau gesetzt
/// heisst jeden Tick ausgeben, nach so vielen Ticks das Journal schreiben,
/// `takt end` und Halt.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

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

/// Der Hardware-Watchdog ist noch nicht angebunden (plan/esp32c6.md 4);
/// `esp-hal` haelt RWDT und MWDT beim Start an.
struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn kick(&mut self) {}
}

/// Die Schleife meldet je Tick; im Konformitaetslauf geht jede Zeit als
/// Metazeile mit (grammar/trace.md, `time`).
#[derive(Default)]
struct Summary {
    slept: u64,
    overruns: u64,
    trace: bool,
}

impl Sink for Summary {
    fn record(&mut self, tick: &Tick) {
        self.slept += tick.slept;
        self.overruns += u64::from(tick.overrun);
        if let (true, Some(u)) = (self.trace, uart()) {
            let _ = tick.write_time(u);
            u.write("\r\n");
        }
    }
}

fn uart() -> Option<&'static mut Telemetry> {
    unsafe { (*&raw mut UART).as_mut() }
}

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let mut telemetry = Telemetry::new(peripherals.USB_DEVICE);
    let Ok(timer) = takt_board_esp32c6::init(peripherals.SYSTIMER, TICK_NS) else {
        telemetry.write("takt: Periode nicht einrichtbar");
        telemetry.newline();
        loop {
            core::hint::spin_loop();
        }
    };
    telemetry.write("takt esp32c6: tick ");
    telemetry.write_i64(timer.nominal_ns());
    telemetry.write(" ns");
    telemetry.newline();
    unsafe { UART = Some(telemetry) };
    if let Ok(led) = Ws2812::new(peripherals.RMT, peripherals.GPIO8) {
        unsafe { LED = Some(led) };
    }

    let limit: u64 = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    let trace_every = if limit > 0 { 1 } else { TRACE_EVERY };

    // 5.9: s0 kommt aus dem Journal, darum laden vor dem ersten Eintritt.
    let mut nvm = FlashNvm::new(peripherals.FLASH, JOURNAL_AT).with_blocking_ns(NVM_BLOCKING_NS);
    if FRESH_JOURNAL && !nvm.wipe() {
        report("journal: loeschen scheiterte");
    }
    let (mut current, mut stored) = ([0u8; PERSIST_BOUND], [0u8; PERSIST_BOUND]);
    let mut persist = Persist::new(Journal::new(nvm, LOGIC_HASH, PERSIST_MIN_INTERVAL_NS), &mut current, &mut stored);
    let mut program = Generated::new(false);
    let (loaded, applied) = persist.load(&mut program);
    program.ensure_init();
    if let Some(u) = uart() {
        match loaded {
            Loaded::Found { length, sequence } => {
                let _ = write!(u, "journal: Eintrag {sequence}, {length} Byte, {applied} Werte geladen\r\n");
            }
            Loaded::Empty => u.write("journal: leer\r\n"),
        }
    }

    let clock = takt_rt_baremetal::TimerClock::new(timer, TICK_NS);
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    let sink = Summary { trace: limit > 0, ..Summary::default() };
    let mut rt = Runtime::new(program, clock, NoWatchdog, sink, Profile::BAREMETAL, TICK_NS, policy);
    if limit > 0 {
        rt.program.dump();
    }
    let mut next_trace = trace_every;
    loop {
        rt.step_persisting(&mut persist);
        rt.program.commit();
        let k = rt.tick_number();
        if k >= next_trace {
            next_trace = k + trace_every;
            rt.program.dump();
            if TRACE_PC {
                rt.program.pc();
            }
        }
        if limit > 0 && k >= limit {
            break;
        }
    }
    let flushed = persist.flush(&mut rt.program);
    if let Some(u) = uart() {
        let journal = persist.journal();
        let (erase_ns, program_ns) = journal.device().measured_ns();
        let o = rt.overrun();
        let _ = write!(
            u,
            "takt schlief {} ueberlaeufe {} verspaetet {} verloren {} rueckstand {} ns \
             journal geschrieben {} fehlgeschlagen {} flush {} \
             nvm loeschen {erase_ns} ns programmieren {program_ns} ns\r\n",
            rt.sink.slept,
            rt.sink.overruns,
            o.late,
            o.lost,
            o.worst_drift,
            journal.writes(),
            journal.failures(),
            u8::from(flushed)
        );
        u.write("takt end\r\n");
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
    }
}

