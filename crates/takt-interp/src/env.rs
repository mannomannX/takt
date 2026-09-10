//! Umgebung der Auswertung: was ein Ausdruck oder eine Anweisung ausserhalb
//! der eigenen Funktionsrahmen liest und schreibt (Maschinenvariablen,
//! Inputs, Outputs, Ψ, Zaehler, Beobachtungen). Die Konstantenauswertung
//! (11.3) hat keine Umgebung; jede Methode meldet dort einen internen Fehler,
//! weil das Sema Konstanten vorher auf Konstanz prueft.

use takt_diag::Span;
use takt_mir::expr::Builtin;
use takt_mir::{ChannelId, CommandId, CounterId, MachineId, ParamId, SignalId, SiteId, TypeId, VarId};

use crate::value::{EvalResult, Sample, Value, bug};

/// Beobachtung (5.6, 13.5): nie ein Fault.
#[derive(Clone, Debug, PartialEq)]
pub enum Observation {
    /// `log "…"`
    Log(String),
    /// `alert cond, "…"`: Zustand der Verletzung an dieser Stelle.
    Alert {
        /// Stelle des Alerts (Schluessel fuer Flanken).
        span: Span,
        /// Bedingung verletzt (nach Bestaetigungszeit).
        violated: bool,
        /// Meldung.
        message: String,
    },
    /// `measure name = e`; `None`, wenn der Wert ungueltig war.
    Measure {
        /// Name.
        name: String,
        /// Wert.
        value: Option<Value>,
    },
    /// `verify cond, "…"`
    Verify {
        /// Stelle.
        span: Span,
        /// Bedingung erfuellt.
        ok: bool,
        /// Meldung.
        message: String,
        /// Anforderungsreferenz.
        req: Option<String>,
    },
    /// `verdict pass | fail`
    Verdict {
        /// `pass`
        pass: bool,
        /// Meldung.
        message: Option<String>,
    },
}

/// Umgebung einer Maschine (9.1) aus Sicht von `eval` und `exec`.
pub trait Outer {
    /// Maschinenvariable.
    fn var(&self, _v: VarId) -> EvalResult<&Value> {
        bug("Variable ausserhalb einer Maschine")
    }
    /// Maschinenvariable, schreibend.
    fn var_mut(&mut self, _v: VarId) -> EvalResult<&mut Value> {
        bug("Variable ausserhalb einer Maschine")
    }
    /// Typ einer Maschinenvariablen (Kapazitaeten von Sammlungen).
    fn var_type(&self, _v: VarId) -> EvalResult<TypeId> {
        bug("Variable ausserhalb einer Maschine")
    }
    /// Parameter des Laufs.
    fn param(&self, _p: ParamId) -> EvalResult<&Value> {
        bug("Parameter ausserhalb eines Laufs")
    }
    /// Command als Puls dieses Ticks (8.5).
    fn command(&self, _c: CommandId) -> EvalResult<bool> {
        bug("Command ausserhalb eines Laufs")
    }
    /// Abtastung eines Inputs (3.5).
    fn input(&self, _c: ChannelId) -> EvalResult<&Sample> {
        bug("Input ausserhalb eines Laufs")
    }
    /// Latch eines eigenen Outputs.
    fn output(&self, _c: ChannelId) -> EvalResult<&Value> {
        bug("Output ausserhalb eines Laufs")
    }
    /// Latch eines eigenen Outputs, schreibend.
    fn output_mut(&mut self, _c: ChannelId) -> EvalResult<&mut Value> {
        bug("Output ausserhalb eines Laufs")
    }
    /// `pub var` einer anderen Maschine aus Ψ (Unit-Delay).
    fn published(&self, _m: MachineId, _v: VarId) -> EvalResult<&Value> {
        bug("Veroeffentlichung ausserhalb eines Laufs")
    }
    /// `m.state` aus Ψ.
    fn state_of(&self, _m: MachineId) -> EvalResult<Value> {
        bug("Zustand ausserhalb eines Laufs")
    }
    /// Signal einer anderen Maschine aus Ψ.
    fn signal(&self, _m: MachineId, _s: SignalId) -> EvalResult<bool> {
        bug("Signal ausserhalb eines Laufs")
    }
    /// `now`, `tick`, `time_in_state`, `last_fault`, `event`.
    fn builtin(&self, _b: Builtin) -> EvalResult<Value> {
        bug("eingebaute Groesse ausserhalb eines Laufs")
    }
    /// Bestaetigungszaehler `viol[site]` in Nanosekunden (5.6).
    fn viol(&mut self, _site: SiteId) -> EvalResult<&mut i64> {
        bug("Bestaetigungszaehler ausserhalb einer Maschine")
    }
    /// `next`-Zaehler eines `every` in Nanosekunden (5.8).
    fn every(&mut self, _counter: CounterId) -> EvalResult<&mut i64> {
        bug("every ausserhalb einer Maschine")
    }
    /// Periode der Maschine in Nanosekunden (`P_m`).
    fn period(&self) -> EvalResult<i64> {
        bug("Periode ausserhalb einer Maschine")
    }
    /// Beobachtung melden.
    fn observe(&mut self, _o: Observation) -> EvalResult<()> {
        bug("Beobachtung ausserhalb einer Maschine")
    }
    /// `abort`: Vormerkung fuer alle anderen Maschinen (5.4).
    fn abort(&mut self) -> EvalResult<()> {
        bug("abort ausserhalb einer Maschine")
    }
    /// `raise sig` (5.8).
    fn raise(&mut self, _s: SignalId) -> EvalResult<()> {
        bug("raise ausserhalb einer Maschine")
    }
}

/// Umgebung der Konstantenauswertung: nur programmweite Konstanten sind
/// lesbar, allen voran `tick` (T₀, 3.3); alles andere gehoert zu einem Lauf.
pub struct ConstEnv {
    /// Basis-Tick in Nanosekunden.
    pub tick: i64,
}

impl ConstEnv {
    /// Umgebung mit dem Basis-Tick des Programms.
    pub fn new(tick: i64) -> Self {
        ConstEnv { tick }
    }
}

impl Outer for ConstEnv {
    fn builtin(&self, b: Builtin) -> EvalResult<Value> {
        match b {
            Builtin::Tick => Ok(Value::Duration(self.tick)),
            other => bug(format!("`{other:?}` ist nicht konstant")),
        }
    }
}
