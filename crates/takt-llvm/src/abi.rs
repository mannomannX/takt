//! Was der erzeugte Code von der Runtime ruft (9.3, 5.4).
//!
//! Der Tickschritt rechnet, aber er *meldet* auch: `alert`, `log`,
//! `measure` und `verify` sind Beobachtungen (9.3) — sie aendern den
//! Zustand nicht, sie tragen etwas nach draussen. Und `abort` beendet den
//! Lauf aller Maschinen (5.4).
//!
//! Beides kann der erzeugte Code nicht selbst: Ein Trace-Eintrag braucht
//! einen Puffer, ein `abort` die anderen Maschinen. Er ruft darum die
//! Runtime, und diese Aufrufe sind eine ABI wie das Prozessabbild.
//!
//! **Die Signaturen tragen keine Zeichenketten, sondern Nummern.** Ein
//! `alert` traegt seinen Text im Programm, nicht im Aufruf: Die Meldung
//! steht in einer Tabelle, die der Compiler erzeugt, und der Aufruf nennt
//! ihren Index. Das spart auf einer MCU den Text im RAM (12.3), macht die
//! Signatur unabhaengig von der Zeichenkettendarstellung — und der
//! Trace bleibt kanonisch, weil die Runtime den Text aus derselben
//! Tabelle nimmt wie der Report.

use crate::emit::Module;
use crate::ty::LlvmType;

/// Die Funktionen, die die Runtime bereitstellt.
///
/// Sie werden deklariert, nicht definiert: Der erzeugte Code ruft sie,
/// `takt-rt-core` liefert sie.
pub struct Abi;

impl Abi {
    /// `alert cond, "text"` (9.3): Eine Flanke wird gemeldet.
    ///
    /// `machine` und `site` identifizieren die Stelle, `active` ist der
    /// Wert der Bedingung in diesem Tick. Die Runtime bildet daraus die
    /// Flanke — sie kennt den vorigen Wert, der erzeugte Code muesste ihn
    /// sonst im Zustand fuehren.
    pub const ALERT: &'static str = "takt_alert";

    /// `log "text"` (9.3).
    pub const LOG: &'static str = "takt_log";

    /// `measure name = e` (13.2): ein Messwert fuer den Report.
    pub const MEASURE: &'static str = "takt_measure";

    /// `verify cond, "text"` (13.2): eine Pruefung mit Verdikt.
    pub const VERIFY: &'static str = "takt_verify";

    /// `abort "text"` (5.4): Fault fuer *alle* Maschinen im selben Tick.
    pub const ABORT: &'static str = "takt_abort";

    /// Schreibt die Deklarationen in den Modulkopf.
    ///
    /// Alle nehmen `(machine: i32, site: i32, ...)`: Die Stelle ist das,
    /// was der Trace braucht, und sie ist beim Uebersetzen bekannt.
    pub fn declare(m: &mut Module) {
        m.declare("\n; Runtime-Schnittstelle (9.3, 5.4); `takt-rt-core` liefert sie");
        m.declare(&format!("declare void @{}(i32, i32, i1)", Abi::ALERT));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::LOG));
        m.declare(&format!("declare void @{}(i32, i32, double)", Abi::MEASURE));
        m.declare(&format!("declare void @{}(i32, i32, i1)", Abi::VERIFY));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::ABORT));
    }
}

/// Der Typ eines Meldungsindex.
pub const SITE: LlvmType = LlvmType::Int(32);
