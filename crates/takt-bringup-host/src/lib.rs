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
#![allow(unsafe_code, reason = "die Leitung als Static fuer die Senke des C-Rahmens; 9.5 fuehrt ihn in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "die Lints fuer `unsafe` (plan/cert.md Abschnitt 6); Stufe 2 nicht, das Crate laeuft nur auf dem Wirt"
    )
)]

use std::io::{BufWriter, Stdout, Write as _};
use std::process::ExitCode;

use takt_embed::{Dispatch as _, Input, Jobs as _, Sample};
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, NoWatchdog, Port, Telemetry, Trace};
use takt_rt_core::{FakeNvm, Fits, Journal, Memory, Policy, Profile, Runtime, Store, Unloaded};

/// Die Standardausgabe als Leitung: Sie nimmt jedes Byte.
struct Stdio(BufWriter<Stdout>);

impl Port for Stdio {
    fn try_write(&mut self, b: u8) -> bool {
        self.0.write_all(&[b]).is_ok()
    }

    fn flush(&mut self) {
        let _ = self.0.flush();
    }
}

/// Der Ring vor der Konsole; verlustfrei, denn auf dem Wirt zaehlt die Zeit nicht.
type Line = Telemetry<Stdio>;

/// Die Leitung, statisch: Die Senke des Programms ([`Console`]) hat keinen
/// Empfaenger und kommt nur so an das, was [`run`] anlegt.
static mut LINE: Option<Line> = None;

fn line() -> Option<&'static mut Line> {
    let slot = &raw mut LINE;
    // SAFETY: ein Faden; der Rahmen schreibt nur aus der Schleife.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

/// Die Konsole als Senke des Programms (12.11): Was der Rahmen ueber die
/// Haken schreibt, geht ganz und in dieser Reihenfolge auf die Leitung. Die
/// Huelle bekommt sie als `&Console`.
#[derive(Debug)]
pub struct Console;

impl takt_embed::Line for Console {
    fn write(&self, bytes: &[u8]) {
        let Some(line) = line() else { return };
        for &b in bytes {
            line.write_byte(b);
        }
    }
}

takt_embed::hooks!(Console);

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

/// Das Geraet des Journals auf dem Wirt: Speicher, der wie das Journal eines
/// Boards im Konformitaetslauf leer beginnt (5.9, 13.8).
pub type Device = FakeNvm<4096>;

/// Das Journal von `program` (5.9), vor `init` geladen; fuer ein Programm
/// ohne `persist` keines. `logic_hash` und `min_interval_ns` sind
/// `LOGIC_HASH` und `PERSIST_MIN_INTERVAL_NS` seines Moduls, die Puffer so
/// gross wie `PERSIST_BOUND`.
pub fn journal<'a, P: takt_rt_core::Program>(
    program: &mut P,
    current: &'a mut [u8],
    stored: &'a mut [u8],
    logic_hash: u64,
    min_interval_ns: i64,
) -> <P::Memory as Memory>::Store<'a, Device> {
    <P::Memory as Memory>::store(|| {
        let journal = Journal::new(Device::new(), logic_hash, min_interval_ns);
        Unloaded::new(journal, current, stored).load(program).0
    })
}

/// Fuehrt `program` mit seinem Journal ([`journal`]) aus, so viele Ticks,
/// wie das erste Argument nennt; `tick_ns` und `alert` sind `TICK_NS` und
/// `OVERRUN_ALERT` seines Moduls.
pub fn run<P, J>(mut program: P, journal: J, tick_ns: i64, alert: bool) -> ExitCode
where
    P: takt_embed::Program,
    J: Store + Fits<P::Memory>,
{
    let Some(ticks) = std::env::args().nth(1).and_then(|a| a.parse::<u64>().ok()) else {
        eprintln!("Aufruf: <binary> TICKS");
        return ExitCode::FAILURE;
    };
    let ring = vec![0; 4096].leak();
    // SAFETY: vor dem ersten Aufruf des Rahmens, in einem Faden.
    unsafe { LINE = Some(Telemetry::new(Stdio(BufWriter::new(std::io::stdout())), ring).lossless()) };
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
    let mut rt = Runtime::journaled(program, journal, clock, NoWatchdog, trace, Profile::BAREMETAL, tick_ns, policy);
    let stats = takt_rt_baremetal::run(&mut rt, None);
    let counted = rt.journal().persist().map_or_else(JournalStats::default, |p| JournalStats {
        writes: p.journal().writes(),
        failures: p.journal().failures(),
        ..JournalStats::default()
    });
    if let Some(line) = line() {
        takt_rt_baremetal::report(line, rt.overrun(), &stats, &counted, &Default::default());
        line.drain(DRAIN_ROUNDS);
    }
    ExitCode::SUCCESS
}
