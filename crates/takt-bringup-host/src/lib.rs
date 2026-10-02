//! Der MCU-Rahmen auf dem Wirt (13.8): dasselbe Programm hinter derselben
//! C-ABI und unter derselben Tickschleife wie auf den Boards, in logischer
//! Zeit, mit dem Trace auf der Standardausgabe.
//!
//! **Wozu.** Ein Treiber-Crate in Rust stellt Geraete: Typen, die die
//! Traits aus `takt-embed` erfuellen, und in `takt-drivers.toml` die
//! Adressen, die sie bedienen. `takt driver-test --crate` bindet es mit
//! diesem Crate zu einem Programm auf dem Wirt, dessen Pruefstand danach
//! verdrahtet ist: Der Treiber laeuft gegen sein Hardwaremodell, und es
//! urteilt derselbe Rand (12.6) wie auf dem Board — der Rahmen ist
//! derselbe, nicht nachgebaut.
//!
//! **Was fehlt, mit Absicht.** Kein Speicherschutz, kein Watchdog, keine
//! Echtzeit: Der Wirt prueft die Semantik und den Treibervertrag, nicht
//! das Zeitverhalten eines Boards. Jobs (4.5) rechnen zwischen zwei Ticks
//! zu Ende, wie auf den Boards in logischer Zeit.
//!
//! **Ein Lauf** ist `<binary> TICKS`: Das Programm laeuft so viele Ticks
//! oder bis es seinen Lauf beendet (`next_run`, 12.7); `0` heisst nur
//! Letzteres. Die Ausgabe hat die Form der Boards — Kopf, `takt trace`,
//! der Trace, die Bilanz, `takt end`.

#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt ihn in der TCB")]

use core::ffi::c_void;
use std::io::{BufWriter, Stdout, Write as _};
use std::process::ExitCode;

use takt_embed::{Input, Sample};
use takt_mcu_program::Generated;
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, NoWatchdog, Port, Telemetry};
use takt_rt_core::{FakeNvm, Persist, Policy, Profile, Runtime};

mod takt {
    #![allow(dead_code, missing_docs)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{OVERRUN_ALERT, TICK_NS};

/// Die Standardausgabe als Leitung: Sie nimmt jedes Byte.
struct Console(BufWriter<Stdout>);

impl Port for Console {
    fn try_write(&mut self, b: u8) -> bool {
        self.0.write_all(&[b]).is_ok()
    }

    fn flush(&mut self) {
        let _ = self.0.flush();
    }
}

/// Der Ring vor der Konsole; verlustfrei, denn auf dem Wirt zaehlt die Zeit nicht.
type Line = Telemetry<Console, 4096>;

/// Die Leitung, statisch: Der Rahmen ruft `takt_board_trace` als C-Symbol,
/// und eine Funktion ohne Empfaenger kommt an nichts heran, was in [`run`]
/// liegt.
static mut LINE: Option<Line> = None;

fn line() -> Option<&'static mut Line> {
    // SAFETY: ein Faden; der Rahmen ruft die Leitung nur aus der Schleife.
    unsafe { (&raw mut LINE).as_mut().and_then(Option::as_mut) }
}

/// Vom Rahmen gerufen: eine Zeile Trace, nullterminiert.
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Zeiger auf statischen Text;
/// die Schranke haelt einen Zeiger ohne Null auf (4.1, von Hand).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(line) = line() else { return };
    let mut p = text;
    for _ in 0..256 {
        let b = unsafe { *p };
        if b == 0 {
            return;
        }
        line.write_byte(b);
        p = unsafe { p.add(1) };
    }
}

/// Vom Rahmen gerufen: eine Zahl im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    let Some(line) = line() else { return };
    line.write_i64(value);
    line.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    let Some(line) = line() else { return };
    line.write_u64(value);
    line.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige Ziffernfolge (4.2).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    use core::fmt::Write as _;
    let Some(line) = line() else { return };
    let _ = write!(line, "{value:?} ");
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    let Some(line) = line() else { return };
    line.write_hex8(value);
}

/// Das Geraet hinter `input … @ hw("sys/previous_run")` (12.7): Ein Prozess
/// beginnt wie ein frisch geschriebenes Board ohne vorigen Lauf (`NONE`).
#[derive(Debug, Default)]
pub struct PreviousRun;

impl Input<i32> for PreviousRun {
    fn sample(&mut self, now: i64) -> Option<Sample<i32>> {
        Some(Sample::good(0, now))
    }
}

/// Kein Journal: Ein Lauf auf dem Wirt beginnt wie der Interpreter ohne
/// Speicher (5.9).
fn no_journal<'a>() -> Option<&'a mut Persist<'a, FakeNvm<0>>> {
    None
}

/// Fuehrt das Programm aus, so viele Ticks, wie das erste Argument nennt.
///
/// # Safety
///
/// `drivers` zeigt auf das Treiberobjekt, fuer das der Kleber des Programms
/// erzeugt ist (`takt_conformance::bringup::drivers`), und lebt bis zum
/// Ende des Laufs.
pub unsafe fn run(drivers: *mut c_void) -> ExitCode {
    let Some(ticks) = std::env::args().nth(1).and_then(|a| a.parse::<u64>().ok()) else {
        eprintln!("Aufruf: <binary> TICKS");
        return ExitCode::FAILURE;
    };
    // SAFETY: vor dem ersten Aufruf des Rahmens, in einem Faden.
    unsafe { LINE = Some(Telemetry::new(Console(BufWriter::new(std::io::stdout()))).lossless()) };
    let Some(head) = line() else { return ExitCode::FAILURE };
    head.write("takt auf dem wirt");
    head.newline();
    head.mark();

    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    // Zwischen den Ticks rechnet jeder Job zu Ende und die Leitung leert
    // sich; dann steht die Uhr auf der Frist (4.5, 13.8).
    let clock = LogicalClock::new(|| {
        while takt_mcu_program::jobs::dispatch() {
            takt_mcu_program::jobs::work();
        }
        if let Some(line) = line() {
            line.drain(DRAIN_ROUNDS);
        }
    });
    // SAFETY: siehe oben.
    let program = unsafe { Generated::init(false, drivers) };
    let mut rt = Runtime::new(program, clock, NoWatchdog, (), Profile::BAREMETAL, TICK_NS, policy);
    let stats = takt_rt_baremetal::run(&mut rt, no_journal(), Cadence::of(ticks, 1, false), line);
    if let Some(line) = line() {
        takt_rt_baremetal::report(line, rt.overrun(), &stats, &JournalStats::default(), None);
        line.drain(DRAIN_ROUNDS);
    }
    ExitCode::SUCCESS
}
