//! Die Testhilfe (12.11, 13.1): das Programm in logischer Zeit, sein Trace
//! gegen den Interpreter.
//!
//! ```ignore
//! let mut arena = valve::Arena::new();
//! let mut tank = Tank::default();
//! let trace = takt_embed::testing::run(valve::Program::init(&mut arena, &mut tank), valve::TICK_NS, 1000);
//! takt_embed::testing::same_as_interpreter("takt/valve.takt", 1000, &trace).unwrap();
//! ```
//!
//! **Die Leitung.** Der Rahmen schreibt seinen Trace ueber die C-Funktionen
//! `takt_board_trace*` (12.5); dieses Modul stellt sie und sammelt je Faden
//! in einen Puffer, den [`take`] leert. Ein Test, der mehrere Programme
//! faehrt, sieht ihre Zeilen darum in der Reihenfolge der Aufrufe.
//!
//! **Der Vergleich** ist der der Board-Suite (Satz 9.4.4): je Tick die
//! Ausgaenge, `f32` als sein Wert in `f64` (4.2, FB-356).

#![allow(unsafe_code, reason = "die Leitung des Rahmens als C-Funktionen (12.5); 9.5 fuehrt sie in der TCB")]

use core::cell::RefCell;
use core::ffi::{CStr, c_char};
use core::fmt::Write as _;
use std::collections::BTreeSet;
use std::path::Path;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, thread_local};

use crate::Jobs as _;
use takt_rt_core::{Clock, Outputs, Policy, Profile, Runtime, Sink, Tick, Tunables, Watchdog};

thread_local! {
    static LINE: RefCell<String> = const { RefCell::new(String::new()) };
}

fn push(text: &str) {
    LINE.with(|l| l.borrow_mut().push_str(text));
}

/// Vom Rahmen gerufen: eine Zeile Trace, nullterminiert.
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Text, der waehrend des Aufrufs gilt.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const c_char) {
    // SAFETY: siehe oben.
    let text = unsafe { CStr::from_ptr(text) };
    push(&text.to_string_lossy());
}

/// Vom Rahmen gerufen: eine Zahl im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    push(&format!("{value} "));
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    push(&format!("{value} "));
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige Ziffernfolge (4.2).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    push(&format!("{value:?} "));
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    push(&format!("0x{value:02x}"));
}

/// Der Trace dieses Fadens seit dem letzten Aufruf.
pub fn take() -> String {
    LINE.with(|l| core::mem::take(&mut *l.borrow_mut()))
}

/// Die logische Zeit (12.11, Form `logical`): Die Uhr steht, bis die
/// Testhilfe sie auf die naechste Frist stellt.
#[derive(Debug, Default)]
struct Logical(i64);

impl Clock for Logical {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

/// Kein Watchdog: Ein Test hat keinen.
struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Zeigt den Anfangszustand ganz und danach je Tick die Aenderungen, wie
/// ein Konformitaetslauf (9.3).
struct Conformance;

impl Sink for Conformance {
    fn outputs(&mut self, tick: Option<&Tick>) -> Outputs {
        if tick.is_some() { Outputs::Changed } else { Outputs::All }
    }

    fn record(&mut self, _tick: &Tick) {}
}

/// Faehrt `program` ueber `ticks` Ticks von `tick_ns` in logischer Zeit
/// und liefert seinen Trace; beendet das Programm den Lauf vorher (12.7),
/// endet auch der Trace.
///
/// Der Weg ist der eines Wirts: [`Runtime::service`] an jeder Frist, und
/// zwischen den Ticks rechnet jeder Job zu Ende (4.5, 13.8).
///
/// # Panics
///
/// Wenn der Griff des Job-Kontexts schon vergeben ist (`Program::jobs`).
pub fn run<P: crate::Program>(program: P, tick_ns: i64, ticks: u64) -> String {
    take();
    let mut stepper = Stepper::new(program, tick_ns, ticks);
    let mut trace = String::new();
    while let Some(part) = stepper.step() {
        trace.push_str(&part);
    }
    trace
}

/// Wie [`run`], mit einer Quelle fuer Tunables (8.4): Ihre Saetze gehen ueber
/// den Weg der Schleife in das Programm (`Runtime::service_with`,
/// `Program::tune`), vor dem Schritt ihrer Grenze. Grenze `k` der Schleife
/// ist Tick `k + 1` des Traces.
///
/// # Panics
///
/// Wie [`run`].
pub fn run_tuned<P: crate::Program>(program: P, tick_ns: i64, ticks: u64, tunables: &mut dyn Tunables) -> String {
    take();
    let mut stepper = Stepper::new(program, tick_ns, ticks);
    let mut trace = String::new();
    while let Some(part) = stepper.step_with(Some(&mut *tunables)) {
        trace.push_str(&part);
    }
    trace
}

/// Ein Programm in logischer Zeit, Frist fuer Frist (12.11, Form
/// `logical`): Wer mehrere Programme im selben Faden abwechselnd faehrt,
/// bekommt je Schritt den Trace genau dieses Programms. [`run`] ist dieser
/// Schritt bis zum Ende.
pub struct Stepper<P: crate::Program> {
    rt: Runtime<P, Logical, Quiet, Conformance>,
    jobs: P::Jobs,
    /// Die Frist des letzten Ticks ist `(ticks - 1) * tick_ns`; ein Schlaf
    /// (9.9) ueber sie hinaus endet den Lauf, statt Ticks hinter dem
    /// Interpreter zu rechnen.
    end: i64,
    done: bool,
}

impl<P: crate::Program> Stepper<P> {
    /// Faehrt `program` ueber `ticks` Ticks von `tick_ns`.
    ///
    /// # Panics
    ///
    /// Wenn der Griff des Job-Kontexts schon vergeben ist (`Program::jobs`).
    pub fn new(mut program: P, tick_ns: i64, ticks: u64) -> Stepper<P> {
        let jobs = program.jobs().expect("der Job-Kontext gehoert der Testhilfe");
        let rt =
            Runtime::new(program, Logical::default(), Quiet, Conformance, Profile::BAREMETAL, tick_ns, Policy::Fault);
        let end = i64::try_from(ticks).unwrap_or(i64::MAX).saturating_mul(tick_ns);
        Stepper { rt, jobs, end, done: false }
    }

    /// Rechnet bis zur naechsten Frist und liefert, was das Programm dabei
    /// in den Trace schrieb; jeder Job rechnet zu Ende (4.5). `None`, wenn
    /// der Lauf vorbei ist.
    pub fn step(&mut self) -> Option<String> {
        self.step_with(None)
    }

    /// Wie [`Stepper::step`], mit einer Quelle fuer Tunables (8.4).
    pub fn step_with(&mut self, mut tunables: Option<&mut dyn Tunables>) -> Option<String> {
        if self.done {
            return None;
        }
        let before = take();
        debug_assert!(before.is_empty(), "fremder Trace in diesem Faden: {before}");
        loop {
            let next = self
                .rt
                .service_with(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>, again(&mut tunables));
            if next.jobs {
                self.jobs.work();
                continue;
            }
            if next.ended.is_some() || next.deadline >= self.end {
                // Ein Schlaf ueber das Ende hinaus gibt die Zeile des Ticks
                // davor erst hier frei (9.9).
                self.rt.finish(None::<&mut takt_rt_core::Persist<'_, takt_rt_core::FakeNvm<0>>>);
                self.done = true;
            } else {
                self.rt.clock.wait_until(next.deadline);
            }
            return Some(take());
        }
    }
}

/// Leiht die Tunables fuer einen Aufruf erneut.
fn again<'a>(tunables: &'a mut Option<&mut dyn Tunables>) -> Option<&'a mut dyn Tunables> {
    match tunables {
        Some(t) => Some(&mut **t),
        None => None,
    }
}

/// Vergleicht `trace` mit dem Lauf des Interpreters ueber `ticks` Ticks fuer
/// das Programm in `source` (13.1, Satz 9.4.4). Die Konfigurationen aus
/// `import channels` liegen neben ihm (8.2).
///
/// Der Interpreter laeuft ohne Stimulus: Liefern die Treiber des Tests
/// Eingaben, sieht er sie nicht; dafuer [`same_as_interpreter_with`].
///
/// # Errors
///
/// Wenn das Programm nicht uebersetzt, der Interpreter abbricht oder ein
/// Ausgang abweicht; die Meldung nennt die ersten Abweichungen.
pub fn same_as_interpreter(source: impl AsRef<Path>, ticks: u64, trace: &str) -> Result<(), String> {
    same_as_interpreter_with(source, ticks, "", trace)
}

/// Wie [`same_as_interpreter`], und der Interpreter sieht `stimulus` in
/// der Form eines Traces (`grammar/trace.md`: `in`, `cmd`, `runtime`, …):
/// dieselben Eingaben, die die Treiber des Tests zu denselben Ticks liefern.
/// Beide Seiten haelt der Test von Hand gleich. TODO(M11 Schritt 9): der
/// Stimulus als Testtreiber, eine Quelle fuer beide Seiten.
///
/// # Errors
///
/// Wie [`same_as_interpreter`], dazu ein Stimulus, der kein Trace ist.
pub fn same_as_interpreter_with(
    source: impl AsRef<Path>,
    ticks: u64,
    stimulus: &str,
    trace: &str,
) -> Result<(), String> {
    let path = source.as_ref();
    let stimulus = takt_interp::trace::Trace::parse(stimulus).map_err(|e| format!("Stimulus: {e}"))?;
    let widened = interpreted(path, ticks, &stimulus)?;
    // Verglichen wird nur, was beide Seiten melden; ein Ausgang, den der Lauf
    // nie schreibt, fiele sonst durch (FB-305).
    let have = output_names(trace);
    let missing: Vec<&str> = output_names(&widened).into_iter().filter(|n| !have.contains(n)).collect();
    let diffs = takt_conformance::run::compare(&widened, trace);
    if missing.is_empty() && diffs.is_empty() {
        return Ok(());
    }
    let mut s = format!("{}: {} Abweichungen vom Interpreter\n", path.display(), diffs.len());
    if !missing.is_empty() {
        let _ = writeln!(s, "  im Lauf fehlen die Ausgaenge {}", missing.join(", "));
    }
    for d in diffs.iter().take(8) {
        let _ = writeln!(s, "  {d}");
    }
    Err(s)
}

/// Der Trace des Interpreters ueber `ticks` Ticks, `f32` als sein Wert in
/// `f64` wie im erzeugten Code (4.2, FB-356).
fn interpreted(path: &Path, ticks: u64, stimulus: &takt_interp::trace::Trace) -> Result<String, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let channel_imports = takt_sema::channel_imports(&text)
        .into_iter()
        .map(|file| std::fs::read_to_string(dir.join(&file)).map(|t| (file, t)))
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        channel_imports,
        core: None,
    };
    let checked = takt_sema::compile(&text, &options);
    let Some(p) = checked.program else {
        let errors: Vec<String> =
            checked.diagnostics.iter().filter(|d| d.is_error()).map(ToString::to_string).collect();
        return Err(format!("{}:\n{}", path.display(), errors.join("\n")));
    };
    let run = takt_interp::RunOptions { ticks, ..Default::default() };
    let interpreted = takt_interp::run(&p, stimulus, &run).map_err(|e| format!("Interpreter: {e:?}"))?.trace.render();
    Ok(takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p)))
}

/// Die Ausgaenge, die ein Trace nennt (`t=<k> out <name> …`).
fn output_names(trace: &str) -> BTreeSet<&str> {
    trace
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next()?.starts_with("t=") && w.next()? == "out").then_some(())?;
            w.next()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALVE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/rust-host/takt/valve.takt");

    /// **Der Vergleich besteht nicht leer**: Der Trace des Interpreters
    /// selbst besteht; ein Lauf ohne Zeilen, ein falscher Wert und ein
    /// fehlender Ausgang scheitern und nennen, was fehlt.
    #[test]
    fn the_comparison_rejects_what_differs() {
        let good = interpreted(Path::new(VALVE), 1000, &Default::default()).expect("Interpreter");
        assert!(good.contains("t=300 out valve true"), "{good}");
        assert_eq!(same_as_interpreter(VALVE, 1000, &good), Ok(()));

        let empty = same_as_interpreter(VALVE, 1000, "").expect_err("ein leerer Lauf");
        assert!(empty.contains("fehlen die Ausgaenge fills, valve"), "{empty}");

        let wrong = good.replace("t=300 out valve true", "t=300 out valve false");
        let wrong = same_as_interpreter(VALVE, 1000, &wrong).expect_err("ein falscher Wert");
        assert!(wrong.contains("t=300 valve"), "{wrong}");

        let without: String =
            good.lines().filter(|l| !l.contains(" out fills ")).map(|l| l.to_string() + "\n").collect();
        let without = same_as_interpreter(VALVE, 1000, &without).expect_err("ein fehlender Ausgang");
        assert!(without.contains("fehlen die Ausgaenge fills"), "{without}");
    }
}
