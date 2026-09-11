//! Der Aufbau des Prozessabbilds im erzeugten Code (9.1, 3.5).
//!
//! 11.2 gibt der Schrittfunktion den Zeiger `i` auf das Prozessabbild,
//! sagt aber nicht, wie es innen aussieht. Das ist hier zu entscheiden,
//! und die Entscheidung ist eine ABI: Runtime und erzeugter Code muessen
//! sich einig sein, sonst liest die eine Seite, was die andere nicht
//! geschrieben hat.
//!
//! **Ein Eintrag je Channel, Wert und Qualitaet nebeneinander.**
//!
//! ```text
//! { wert: T, qualitaet: i8, grund: i8, alter: i64 }
//! ```
//!
//! Die Alternative waere gewesen, Werte und Qualitaeten in getrennte
//! Arrays zu legen („struct of arrays"). Dagegen spricht 3.5: Fast jeder
//! Zugriff auf einen Input liest *beide* — der implizite Validitaets-Check
//! steht vor jedem Lesen, und `x.or(d)` braucht Wert und Qualitaet in
//! derselben Zeile. Getrennte Arrays kosteten dafuer zwei Cache-Zeilen
//! statt einer.
//!
//! **Die Qualitaet ist ein `i8`, kein Bitfeld.** Vier Werte passten in
//! zwei Bit, aber die Runtime schreibt sie aus einem anderen Crate
//! (`takt-hal`), und ein Bitfeld waere eine Verabredung, die keiner der
//! beiden Seiten ansieht. Ein Byte ist die Form, die beide ohne Absprache
//! richtig treffen.

use takt_mir::ChannelId;
use takt_mir::program::Program;

use crate::ty::{self, LlvmType};

/// Die Qualitaet als Zahl (3.5). Dieselbe Reihenfolge wie
/// `takt_hal::Quality` — die Runtime schreibt sie, der erzeugte Code
/// liest sie.
pub mod quality {
    /// Gueltig und frisch.
    pub const GOOD: i64 = 0;
    /// Entprellt (`debounce`), letzter guter Wert wird gehalten.
    pub const SUSPECT: i64 = 1;
    /// Alter ueber `max_age`.
    pub const STALE: i64 = 2;
    /// Ungueltig.
    pub const BAD: i64 = 3;
}

/// Der Eintrag eines Channels im Prozessabbild.
pub fn entry_type(channel: ChannelId, p: &Program) -> Option<LlvmType> {
    let c = p.channels.get(channel.index())?;
    let value = ty::lower(c.ty, p)?;
    Some(LlvmType::Struct(vec![value, LlvmType::Int(8), LlvmType::Int(8), LlvmType::Int(64)]))
}

/// Die Felder eines Eintrags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Slot {
    Value = 0,
    Quality = 1,
    Reason = 2,
    Age = 3,
}

/// Das Prozessabbild ist ein Array von Eintraegen, eines je Channel; die
/// Reihenfolge ist die der `ChannelId`, wie im Interpreter.
///
/// Ein Eintrag hat je Channel eine andere Groesse (der Wert traegt seinen
/// Typ), darum wird nicht indiziert, sondern der Versatz aufsummiert. Das
/// ist der Preis dafuer, dass die Werte nicht alle gleich gross sind — die
/// Alternative waere ein Union in der Groesse des groessten Werts, und die
/// waere fuer ein Abbild mit einem `bytes<256>` verschwenderisch.
pub fn offset_of(channel: ChannelId, p: &Program) -> Option<u64> {
    let mut sum = 0;
    for i in 0..channel.index() {
        sum += entry_type(ChannelId(i as u32), p)?.size();
    }
    Some(sum)
}
