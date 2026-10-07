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
//!
//! **Das Programm bringt der Wirt mit** (12.11, M11 Schritt 10): Er bindet
//! es als Lieferform (`takt_embed::build`), mit dem Pruefstand `Rig` als
//! Treiberobjekt, und reicht die Huelle an [`run`].

#![allow(unsafe_code, reason = "Telemetrie fuer den C-Rahmen; 9.5 fuehrt ihn in der TCB")]

use std::io::{BufWriter, Stdout, Write as _};
use std::process::ExitCode;

use takt_embed::{Dispatch as _, Input, Jobs as _, Sample};
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, NoWatchdog, Port, Telemetry, Trace};
use takt_rt_core::{FakeNvm, Persist, Policy, Profile, Runtime};

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
/// Ganz, ohne Schranke: Eine still nach 256 Byte gekuerzte Zeile waere ein
/// Trace, der vom Interpreter abweicht, ohne es zu sagen (RT-038).
///
/// # Safety
///
/// `text` ist null oder ein nullterminierter Text; der Rahmen uebergibt nur
/// Literale und Namen aus seinen Tabellen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(line) = line() else { return };
    if text.is_null() {
        return;
    }
    // SAFETY: siehe oben.
    let text = unsafe { core::ffi::CStr::from_ptr(text.cast()) };
    trace_text(text, &mut |b| line.write_byte(b));
}

/// Jedes Byte eines Texts an `put`.
fn trace_text(text: &core::ffi::CStr, put: &mut dyn FnMut(u8)) {
    for &b in text.to_bytes() {
        put(b);
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

impl Input<u32> for PreviousRun {
    fn sample(&mut self, now: i64) -> Option<Sample<u32>> {
        Some(Sample::good(0, now))
    }
}

/// Das Geraet hinter `input … @ hw("sys/clock")` (7.4): die Wanduhr des
/// Wirts in Nanosekunden seit der Unix-Epoche, ausserhalb der Semantik —
/// dieselbe Auskunft wie `takt_rt_linux::wall_clock_ns` ausserhalb von
/// Linux. Vor der Epoche weiss der Wirt keine Zeit und liefert nichts.
#[derive(Debug, Default)]
pub struct WallClock;

impl Input<i64> for WallClock {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(Sample::good(i64::try_from(since.as_nanos()).ok()?, now))
    }
}

/// Kein Journal: Ein Lauf auf dem Wirt beginnt wie der Interpreter ohne
/// Speicher (5.9).
fn no_journal<'a>() -> Option<&'a mut Persist<'a, FakeNvm<0>>> {
    None
}

/// Fuehrt `program` aus, so viele Ticks, wie das erste Argument nennt;
/// `tick_ns` und `alert` sind `TICK_NS` und `OVERRUN_ALERT` seines Moduls.
pub fn run<P: takt_embed::Program>(mut program: P, tick_ns: i64, alert: bool) -> ExitCode {
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

    let policy = if alert { Policy::Alert } else { Policy::Fault };
    let (mut dispatch, mut jobs) = (program.dispatch(), program.jobs());
    // Zwischen den Ticks rechnet jeder Job zu Ende und die Leitung leert
    // sich; dann steht die Uhr auf der Frist (4.5, 13.8).
    let clock = LogicalClock::new(|| {
        if let (Some(dispatch), Some(jobs)) = (dispatch.as_mut(), jobs.as_mut()) {
            while dispatch.next() {
                jobs.work();
            }
        }
        if let Some(line) = line() {
            line.drain(DRAIN_ROUNDS);
        }
    });
    let trace = Trace::new(Cadence::of(ticks, 1), tick_ns, line);
    let mut rt = Runtime::new(program, clock, NoWatchdog, trace, Profile::BAREMETAL, tick_ns, policy);
    let stats = takt_rt_baremetal::run(&mut rt, no_journal(), None);
    if let Some(line) = line() {
        takt_rt_baremetal::report(line, rt.overrun(), &stats, &JournalStats::default(), &Default::default());
        line.drain(DRAIN_ROUNDS);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    /// Eine Zeile ueber 256 Byte kommt ganz an (RT-038).
    #[test]
    fn a_long_trace_line_is_not_cut() {
        let long: Vec<u8> = (0..300u32).map(|i| b'a' + (i % 26) as u8).chain([0]).collect();
        let text = core::ffi::CStr::from_bytes_with_nul(&long).expect("nullterminiert");
        let mut got = Vec::new();
        super::trace_text(text, &mut |b| got.push(b));
        assert_eq!(got, long[..300]);
    }
}
