//! Referenzinterpreter ueber der MIR: die ausfuehrbare Fassung von Referenz 9
//! (plan/m1.md, Abschnitt 4).
//!
//! - `value`, `arith`: Werte und totale Arithmetik (4.1, 3.3, 3.10).
//! - `eval`, `exec`, `call`: `eval`/`exec` aus 9.2, Aufrufe, Primitive.
//! - `env`: Umgebung (Maschine oder Konstantenauswertung), Beobachtungen.
//! - `loaded`, `validate`: Verifier beim Laden und abgeleitete Tabellen.
//! - `format`: Formatstrings in feste Puffer (3.9).
//! - `machine`, `system`, `image`: Schritt einer Maschine (9.3), System-Tick
//!   (9.4) und das Prozessabbild mit Ψ (9.1).
//! - `trace`: Stimulus, Golden-Trace und lesbare Aufzeichnung (grammar/trace.md).
//!
//! Der Interpreter panickt nie: Faults sind Werte (`Trap::Fault`), Fehler
//! des Interpreters selbst `Trap::Bug`, und der Verifier faengt unerlaubte
//! MIR beim Laden ab.

pub mod arith;
pub mod call;
pub mod env;
pub mod eval;
pub mod exec;
pub mod format;
pub mod image;
pub mod loaded;
pub mod machine;
pub mod pattern;
pub mod run;
pub mod system;
pub mod trace;
pub mod validate;
pub mod value;

pub use env::{ConstEnv, MachineEnv, Observation, Outer};
pub use eval::Ctx;
pub use exec::{Mode, Out};
pub use image::Image;
pub use loaded::Loaded;
pub use machine::MachineState;
pub use run::{RunOptions, RunResult, Verdict, run};
pub use system::Sim;
pub use trace::{Trace, TraceLine};
pub use value::{Fault, Quality, Reason, Sample, Trap, Value};

use takt_mir::expr::Expr;
use takt_mir::program::Program;

/// Wertet einen konstanten Ausdruck aus (11.3): ohne Maschine, Inputs oder
/// Parameter; reine Funktionen mit beschraenkten Schleifen sind erlaubt.
pub fn eval_const(program: &Program, expr: &Expr) -> Result<Value, Trap> {
    let loaded = Loaded::borrow(program);
    let mut env = ConstEnv::new(program.config.tick);
    let mut ctx = Ctx::new(&loaded, &mut env, 0);
    ctx.eval(expr)
}
