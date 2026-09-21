//! Ereignisstroeme im erzeugten Code (8.6, 8.7, 9.6).
//!
//! **Die Puffer gehoeren der Runtime, nicht dem erzeugten Code.** 8.6
//! beschreibt `buf[s]` als beschraenkte FIFO, die der Treiber fuellt und
//! mehrere Konsumenten mit eigenen Cursorn lesen. Der Tickschritt sieht
//! davon nur sein Fenster — und selbst das nicht als Datenstruktur,
//! sondern durch drei Aufrufe:
//!
//! ```text
//! takt_stream_count(s, cur)     wie viele Elemente im Fenster stehen
//! takt_stream_at(s, cur, i, e)  das i-te Element nach `e` schreiben
//! takt_stream_examined(s, seq)  bis hierher untersucht (9.6)
//! ```
//!
//! Die Alternative waere gewesen, dem erzeugten Code den Ringpuffer
//! offenzulegen. Dagegen sprechen zwei Dinge: Der Puffer ist zwischen
//! Treiber-Thread und Tick-Thread geteilt (12.2, lock-freie
//! Doppelpuffer), und seine Darstellung ist eine Sache der Runtime, die
//! sich je Profil unterscheidet — ein Byte-Ring auf der MCU (8.6), etwas
//! anderes auf der Box. Drei Aufrufe sind die schmalste Naht, die beide
//! tragen.
//!
//! **Der Cursor bleibt im Zustand der Maschine** (`cur[s, m]`, 9.6): Er
//! gehoert zum Konsumenten, nicht zum Strom, und zwei Maschinen lesen
//! denselben Strom unabhaengig voneinander.

use takt_mir::TypeId;
use takt_mir::expr::StreamRef;
use takt_mir::program::Program;
use takt_mir::types::Type;

use crate::emit::{Module, Reg};
use crate::expr::NotYet;

/// Die Aufrufe, mit denen der erzeugte Code sein Fenster liest.
pub struct Streams;

impl Streams {
    /// Wie viele Elemente ab `cur` im Fenster stehen (9.6).
    pub const COUNT: &'static str = "takt_stream_count";

    /// Schreibt das `i`-te Element des Fensters an die uebergebene
    /// Stelle; liefert dessen `seq`. Die Stelle traegt `t` (i64,
    /// Nanosekunden), dann die Laenge (i32), dann die Bytes: Text in
    /// der Sammlungsform `{ i32 len, [N x i8] }`, ein Record in der
    /// kanonischen Byteform (plan/m6.md 2.2).
    pub const AT: &'static str = "takt_stream_at";

    /// Versatz der Laenge in dem, was `AT` schreibt.
    pub const LEN_AT: u32 = 8;
    /// Versatz der Bytes in dem, was `AT` schreibt.
    pub const BYTES_AT: u32 = 12;

    /// Meldet, bis zu welcher `seq` die Maschine `m` untersucht hat (9.6:
    /// `cur[s, m] = examined + 1`). Die Runtime bildet daraus das Minimum
    /// ueber alle Leser eines internen Stroms und gibt frei, was darunter
    /// liegt (8.6).
    pub const EXAMINED: &'static str = "takt_stream_examined";

    /// Legt Bytes in den Sendepuffer eines Ausgabestroms (8.8).
    ///
    /// Der Puffer gehoert der Runtime, wie der Empfangsring: Der Treiber
    /// leert ihn mit `max_rate`, und `tx.free` wird zu Tick-Beginn
    /// gesampelt. Das Ergebnis sagt, ob die Bytes hineinpassten — ein
    /// `send` mit `len > tx.free` ist ein `StreamOverflow` (8.8).
    pub const SEND: &'static str = "takt_stream_send";

    /// `o.sent` (8.8, FB-132): schreibt den beim letzten Commit abgeholten
    /// Ausschnitt als `{ i32 len, [CAP x i8] }` an die uebergebene Stelle
    /// und liefert die Laenge. Wie `takt_stream_send` gehoert er dem
    /// Treiber, der den Puffer leert.
    pub const SENT: &'static str = "takt_stream_sent";

    /// Schreibt die Deklarationen in den Modulkopf.
    pub fn declare(m: &mut Module) {
        m.declare("\n; Stroeme (8.6, 8.8, 9.6); die Puffer gehoeren der Runtime");
        m.declare(&format!("declare i32 @{}(i32, i64)", Streams::COUNT));
        m.declare(&format!("declare i64 @{}(i32, i64, i32, ptr)", Streams::AT));
        m.declare(&format!("declare void @{}(i32, i32, i64)", Streams::EXAMINED));
        m.declare(&format!("declare i1 @{}(i32, ptr, i32)", Streams::SEND));
        m.declare(&format!("declare i32 @{}(i32, ptr)", Streams::SENT));
    }
}

/// Die Nummer eines Stroms fuer die Runtime.
///
/// Channel und interner Strom haben je eigene Nummern; die Runtime
/// unterscheidet sie am Vorzeichen, damit ein Aufruf genuegt. `t.fired`
/// ist v1.2, ein Strom in einer Variablen v1.1; beide haben zur
/// Uebersetzungszeit keine feste Nummer.
pub fn number(stream: StreamRef) -> Option<i64> {
    match stream {
        StreamRef::Channel(c) => Some(i64::from(c.0)),
        StreamRef::Internal(s) => Some(-1 - i64::from(s.0)),
        _ => None,
    }
}

/// Der Elementtyp eines Stroms.
pub fn element(p: &Program, stream: StreamRef) -> Option<TypeId> {
    match stream {
        StreamRef::Channel(c) => match p.types.list.get(p.channels.get(c.index())?.ty.index())? {
            Type::Stream(e) => Some(*e),
            _ => None,
        },
        StreamRef::Internal(s) => Some(p.streams.get(s.index())?.elem),
        _ => None,
    }
}

/// Die Kapazitaet der Bytes eines Elements: `N` bei Text, sonst die
/// Hoechstzahl Nutzbytes eines Elements (8.6).
/// kanonische Byteform — dasselbe, was `takt_stream_cap` im Rahmen sagt.
pub fn payload_cap(p: &Program, elem: TypeId) -> Result<u32, NotYet> {
    match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => Ok(*cap),
        _ => takt_mir::bytes::max_size(p, elem).map_err(|_| NotYet { what: "Elementform ohne feste Groesse" }),
    }
}

/// Ein Platz fuer ein Element, wie `takt_stream_at` es schreibt.
pub fn scratch(p: &Program, elem: TypeId, m: &mut Module) -> Result<Reg, NotYet> {
    let cap = payload_cap(p, elem)?;
    Ok(m.inst(&format!("alloca [{} x i8], align 8", cap + Streams::BYTES_AT)))
}

/// Der Inhalt eines Elements aus dem Scratch nach `dst`: Text in seiner
/// Sammlungsform — ein `line` mit `truncated = false`, weil der Rand
/// schon begrenzt hat (3.9) —, ein Record aus der kanonischen Byteform.
pub fn copy_payload(buf: Reg, dst: Reg, elem: TypeId, p: &Program, m: &mut Module) -> Result<(), NotYet> {
    match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => {
            let pair = format!("{{ i32, [{cap} x i8] }}");
            let src = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", Streams::LEN_AT));
            let v = m.inst(&format!("load {pair}, ptr {src}"));
            m.void_inst(&format!("store {pair} {v}, ptr {dst}"));
            if matches!(p.types.list.get(elem.index()), Some(Type::Line { .. })) {
                let flag = m.inst(&format!("getelementptr inbounds i8, ptr {dst}, i64 {}", 4 + cap));
                m.void_inst(&format!("store i1 false, ptr {flag}"));
            }
            Ok(())
        }
        _ => {
            let src = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", Streams::BYTES_AT));
            crate::persist::decode_canonical(p, elem, src, dst, m)
        }
    }
}

/// 9.6: `examined` ist das Maximum ueber die Aktivierung. Das Feld traegt
/// `examined + 1`, damit null „nichts untersucht" heisst; am Ende des
/// Schritts wird daraus der Cursor (`advance_cursors`).
pub fn note_examined(examined_ptr: Reg, seq: Reg, m: &mut Module) {
    let have = m.inst(&format!("load i64, ptr {examined_ptr}"));
    let past = m.inst(&format!("add i64 {seq}, 1"));
    let ahead = m.inst(&format!("icmp sgt i64 {past}, {have}"));
    let new = m.inst(&format!("select i1 {ahead}, i64 {past}, i64 {have}"));
    m.void_inst(&format!("store i64 {new}, ptr {examined_ptr}"));
}
