//! Umgebung der Auswertung: was ein Ausdruck oder eine Anweisung ausserhalb
//! der eigenen Funktionsrahmen liest und schreibt (Maschinenvariablen,
//! Inputs, Outputs, Ψ, Zaehler, Beobachtungen). Die Konstantenauswertung
//! (11.3) hat keine Umgebung; jede Methode meldet dort einen internen Fehler,
//! weil das Sema Konstanten vorher auf Konstanz prueft.

use takt_diag::Span;
use takt_mir::expr::{Accessor, Builtin, StreamRef};
use takt_mir::machine::FaultKind;
use takt_mir::{ChannelId, CommandId, CounterId, MachineId, ParamId, SignalId, SiteId, TypeId, VarId};

use crate::image::Image;
use crate::loaded::Loaded;
use crate::machine::MachineState;
use crate::stream::Element;
use crate::value::{EvalResult, Sample, Value, bug};

/// Beobachtung (5.6, 13.5): nie ein Fault.
#[derive(Clone, Debug, PartialEq)]
pub enum Observation {
    /// `log "…"`
    Log(String),
    /// `alert cond, "…"`: Zustand der Meldung an dieser Stelle. Anders als
    /// `check` nennt die Bedingung eines Alerts das zu meldende *Ereignis*
    /// (5.6); der Alert ist aktiv, wenn sie zutrifft.
    Alert {
        /// Stelle des Alerts.
        span: Span,
        /// Werte der umgebenden Schleifenvariablen: eine Stelle in einer
        /// `for`-Schleife hat je Durchlauf eine eigene Flanke (5.6).
        index: Vec<i64>,
        /// Alert aktiv (nach Bestaetigungszeit).
        active: bool,
        /// Meldung.
        message: String,
        /// Ein Input der Bedingung war ungueltig: der Alert feuert mit
        /// Zusatz, statt die Steuerung zu beeinflussen (3.5).
        invalid: bool,
    },
    /// `measure name = e`; `None`, wenn der Wert ungueltig war.
    Measure {
        /// Name.
        name: String,
        /// Wert.
        value: Option<Value>,
        /// Typ des Werts (Einheit in der Ausgabe).
        ty: TypeId,
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
    /// Ein Fault hat sein Ziel erreicht (5.3); keine Anweisung, sondern
    /// Beobachtung des Laufs fuer Trace und Verdikt (13.5).
    Fault {
        /// Art.
        kind: FaultKind,
        /// Meldung.
        message: String,
        /// Name des Ziels.
        target: String,
    },
    /// `raise sig` (5.8).
    Signal {
        /// Name des Signals.
        name: String,
    },
    /// Coverage (13.2): ein Zustand betreten, eine Transition genommen, ein
    /// `check` ausgewertet, ein Handler gefeuert, ein irreversibler Output
    /// geschrieben (12.7). Kein Trace-Eintrag, sondern ein Zaehler.
    Cover {
        /// Art.
        kind: CoverKind,
        /// Schluessel: Zustandspfad, Transition, Stelle, Output.
        name: String,
    },
}

/// Art eines Coverage-Treffers (13.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CoverKind {
    /// Ein Zustand wurde betreten.
    State,
    /// Eine Transition wurde genommen.
    Transition,
    /// Ein `check` wurde ausgewertet.
    Check,
    /// Ein `check` war verletzt.
    CheckFailed,
    /// Ein Handler hat gefeuert.
    Handler,
    /// Ein irreversibler Output wurde geschrieben (12.7).
    Irreversible,
}

impl CoverKind {
    /// Name in Bericht und Datei.
    pub fn name(self) -> &'static str {
        match self {
            CoverKind::State => "state",
            CoverKind::Transition => "transition",
            CoverKind::Check => "check",
            CoverKind::CheckFailed => "check_failed",
            CoverKind::Handler => "handler",
            CoverKind::Irreversible => "irreversible",
        }
    }
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
    /// Bestaetigungszaehler `viol[site, index]` in Nanosekunden (5.6);
    /// `index` sind die Indizes der umgebenden `for`-Schleifen.
    fn viol(&mut self, _site: SiteId, _index: &[i64]) -> EvalResult<&mut i64> {
        bug("Bestaetigungszaehler ausserhalb einer Maschine")
    }
    /// `next`-Zaehler eines `every` in Nanosekunden (5.8), je Stelle und
    /// Schleifenindex; beim ersten Zugriff nach einem Zustandseintritt
    /// beginnt er bei der Periode `start`.
    fn every(&mut self, _counter: CounterId, _index: &[i64], _start: i64) -> EvalResult<&mut i64> {
        bug("every ausserhalb einer Maschine")
    }
    /// Uhr, gegen die ein `every` misst (5.8): `time_in_state` fuer eine
    /// Stelle in einem Zustand, `now` fuer eine auf Maschinenebene. Ein
    /// maschinenweiter Block gehoert keinem Zustand, dessen Eintritt ihn neu
    /// startete; mit `time_in_state` verstummte er nach dem ersten Wechsel.
    fn every_clock(&self, _counter: CounterId) -> EvalResult<Value> {
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
    /// Coverage-Treffer (13.2); ausserhalb einer Maschine zaehlt nichts.
    fn cover(&mut self, _kind: CoverKind, _name: String) {}
    /// `abort`: Vormerkung fuer alle anderen Maschinen (5.4).
    fn abort(&mut self) -> EvalResult<()> {
        bug("abort ausserhalb einer Maschine")
    }
    /// `raise sig` (5.8).
    fn raise(&mut self, _s: SignalId) -> EvalResult<()> {
        bug("raise ausserhalb einer Maschine")
    }
    /// `send s, wert` (8.6, 8.8): in einen Ausgabestrom oder einen internen
    /// Stream. Ueberlauf trifft den Schreiber.
    fn send(&mut self, _s: StreamRef, _v: Value, _len_max: u32, _span: Span) -> EvalResult<()> {
        bug("send ausserhalb einer Maschine")
    }
    /// `at T: o = v` (9.8): plant einen Schreibvorgang ein.
    fn schedule(&mut self, _o: ChannelId, _t: i64, _v: Value, _span: Span) -> EvalResult<()> {
        bug("at ausserhalb einer Maschine")
    }
    /// `cancel o` (7.5): verwirft die ausstehenden Schreibvorgaenge.
    fn cancel(&mut self, _o: ChannelId) -> EvalResult<()> {
        bug("cancel ausserhalb einer Maschine")
    }
    /// Zaehler eines Stroms (`s.count`, `.dropped`, `.overflowed`,
    /// `.malformed`, `.free`, 8.6/8.8); `None`, wenn der Zugriff kein
    /// Stream-Zaehler ist.
    fn stream_stat(&self, _s: StreamRef, _acc: Accessor) -> EvalResult<Option<Value>> {
        Ok(None)
    }
    /// Fenster W eines Stroms fuer diese Aktivierung (9.6); es ist pro
    /// Aktivierung fest und durch CAP beschraenkt.
    fn stream_window(&self, _s: StreamRef) -> EvalResult<Vec<Element>> {
        bug("Stream-Fenster ausserhalb einer Maschine")
    }
    /// Meldet ein Element als untersucht (9.6, „untersucht heisst
    /// konsumiert").
    fn stream_examined(&mut self, _s: StreamRef, _seq: i64) -> EvalResult<()> {
        bug("Stream-Fenster ausserhalb einer Maschine")
    }
    /// Bindungsrecord eines Elements: Captures, dann `.t`, `.seq` und
    /// `.text`/`.data` beziehungsweise die Felder des Elements (8.7).
    fn element_value(&mut self, _v: VarId, _e: &Element, _caps: Vec<Value>) -> EvalResult<Value> {
        bug("Stream-Bindung ausserhalb einer Maschine")
    }
}

/// Umgebung einer Maschine ueber dem Prozessabbild (9.1): was `eval` und
/// `exec` ausserhalb ihrer Rahmen lesen und schreiben.
pub struct MachineEnv<'a, 'p> {
    /// Geladenes Programm.
    pub loaded: &'a Loaded<'p>,
    /// Die Maschine.
    pub id: MachineId,
    /// Ihr Zustand.
    pub state: &'a mut MachineState,
    /// Prozessabbild und Ψ.
    pub image: &'a mut Image,
    /// Beobachtungen dieses Ticks.
    pub out: &'a mut Vec<Observation>,
    /// Basis-Tick in Nanosekunden.
    pub tick_ns: i64,
    /// Aktueller Tick.
    pub tick: u64,
    /// `abort` wurde in diesem Schritt ausgefuehrt (5.4).
    pub aborted: bool,
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
