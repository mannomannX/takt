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

use takt_mir::program::Program;
use takt_mir::{ChannelId, MachineId, NativeId};

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

/// Der Versatz hinter den Typen `before`, ausgerichtet auf `align`.
///
/// Natuerliche Ausrichtung, wie LLVM sie annimmt: Ein `load double` an
/// einem ungeraden Versatz zerlegt das Backend in Byte-Zugriffe (auf
/// dem C6 die Haelfte des Programms, FB-223), und ein Struct-Zugriff
/// und ein Byte-Versatz meinen so dieselbe Stelle.
fn after(before: impl Iterator<Item = Option<LlvmType>>, align: u64) -> u64 {
    let mut at = 0u64;
    for t in before.flatten() {
        at = at.div_ceil(t.align()) * t.align();
        at += t.aligned_size();
    }
    at.div_ceil(align.max(1)) * align.max(1)
}

/// Das Prozessabbild ist ein Array von Eintraegen, eines je Channel; die
/// Reihenfolge ist die der `ChannelId`, wie im Interpreter.
///
/// Ein Eintrag hat je Channel eine andere Groesse (der Wert traegt seinen
/// Typ), darum wird nicht indiziert, sondern der Versatz aufsummiert. Das
/// ist der Preis dafuer, dass die Werte nicht alle gleich gross sind — die
/// Alternative waere ein Union in der Groesse des groessten Werts, und die
/// waere fuer ein Abbild mit einem `bytes<256>` verschwenderisch.
///
/// Ein Strom hat keinen Eintrag im Abbild: Seine Elemente stehen im
/// Puffer der Runtime (8.6). Sein Platz bleibt leer, damit der Index
/// eines Channels seine `ChannelId` bleibt.
pub fn offset_of(channel: ChannelId, p: &Program) -> Option<u64> {
    let align = entry_type(channel, p).map_or(1, |t| t.align());
    Some(after((0..channel.index()).map(|i| entry_type(ChannelId(i as u32), p)), align))
}

/// Das Ende der Channel-Eintraege.
pub fn entries_end(p: &Program) -> u64 {
    after((0..p.channels.len()).map(|i| entry_type(ChannelId(i as u32), p)), 1)
}

/// Der Versatz eines Outputs im Latch (9.2, 11.2).
///
/// Der Latch traegt je Output nur den Wert — die Qualitaet gehoert zur
/// Eingabe (3.5). Gezaehlt wird ueber *alle* Channels, nicht nur die
/// Outputs: Der Index eines Channels ist seine `ChannelId`, und eine
/// zweite Nummerierung waere eine zweite Gelegenheit, sie verschieden zu
/// vergeben. Ein Strom hat keinen Latch: Er wird gesendet, nicht gestellt
/// (8.8).
pub fn latch_offset(channel: ChannelId, p: &Program) -> Option<u64> {
    let align = crate::ty::lower(p.channels.get(channel.index())?.ty, p).map_or(1, |t| t.align());
    let before = (0..channel.index()).map(|i| p.channels.get(i).and_then(|c| crate::ty::lower(c.ty, p)));
    Some(after(before, align))
}

/// Der Versatz eines Parameters im Parametervektor (8.4).
pub fn param_offset(id: takt_mir::ParamId, p: &Program) -> Option<u64> {
    let align = crate::ty::lower(p.params.get(id.index())?.ty, p)?.align();
    let mut before = Vec::with_capacity(id.index());
    for i in 0..id.index() {
        before.push(Some(crate::ty::lower(p.params.get(i)?.ty, p)?));
    }
    Some(after(before.into_iter(), align))
}

/// Der Versatz eines Commands im Prozessabbild (8.5).
///
/// Commands stehen hinter den Channels; ihr Versatz beginnt damit hinter
/// dem letzten Channel-Eintrag. Ein Command ist ein Puls und traegt nur
/// ein Byte — es hat keine Qualitaet, weil es keine Lieferung ist.
pub fn command_offset(id: takt_mir::CommandId, p: &Program) -> Option<u64> {
    Some(entries_end(p) + id.index() as u64)
}

/// Das Ende der Commands: hier beginnen die Ψ-Baenke.
pub fn commands_end(p: &Program) -> u64 {
    entries_end(p) + p.commands.len() as u64
}

/// Die Groesse eines Job-Slots im Abbild (4.5): `done: i8`, `ok: i8`, zwei
/// Byte frei, `err: i32` (die Diskriminante von `JobErr`), dann der Wert in
/// kanonischer Byteform (`bytes::max_size`); jeder Slot beginnt
/// 8-Byte-ausgerichtet. Die Runtime schreibt ihn, der erzeugte Code liest.
pub fn job_entry_size(native: NativeId, p: &Program) -> Option<u64> {
    let n = p.natives.get(native.index())?;
    let value = u64::from(takt_mir::bytes::max_size(p, n.ret).ok()?);
    Some((8 + value).div_ceil(8) * 8)
}

/// Der Versatz eines Job-Slots: Die Slots liegen hinter den Ψ-Baenken, in
/// der Reihenfolge der Maschinen und ihrer `Layout::job_slots`.
pub fn job_offset(machine: MachineId, slot: usize, p: &Program) -> Option<u64> {
    let mut off = crate::psi::image_size(p);
    for (i, m) in p.machines.iter().enumerate() {
        for (j, s) in m.layout.job_slots.iter().enumerate() {
            if i == machine.index() && j == slot {
                return Some(off);
            }
            off += job_entry_size(s.native, p)?;
        }
    }
    None
}

/// Das Ende des Abbilds mit allen Job-Slots.
pub fn jobs_end(p: &Program) -> u64 {
    let slots = p.machines.iter().flat_map(|m| m.layout.job_slots.iter());
    slots.fold(crate::psi::image_size(p), |off, s| off + job_entry_size(s.native, p).unwrap_or(0))
}
