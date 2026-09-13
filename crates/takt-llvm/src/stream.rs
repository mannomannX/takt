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

use crate::emit::Module;

/// Die Aufrufe, mit denen der erzeugte Code sein Fenster liest.
pub struct Streams;

impl Streams {
    /// Wie viele Elemente ab `cur` im Fenster stehen (9.6).
    pub const COUNT: &'static str = "takt_stream_count";

    /// Schreibt das `i`-te Element des Fensters an die uebergebene
    /// Stelle; liefert dessen `seq`.
    pub const AT: &'static str = "takt_stream_at";

    /// Meldet, bis zu welcher `seq` untersucht wurde (9.6: `cur[s, m] =
    /// examined + 1`).
    pub const EXAMINED: &'static str = "takt_stream_examined";

    /// Legt Bytes in den Sendepuffer eines Ausgabestroms (8.8).
    ///
    /// Der Puffer gehoert der Runtime, wie der Empfangsring: Der Treiber
    /// leert ihn mit `max_rate`, und `tx.free` wird zu Tick-Beginn
    /// gesampelt. Das Ergebnis sagt, ob die Bytes hineinpassten — ein
    /// `send` mit `len > tx.free` ist ein `StreamOverflow` (8.8).
    pub const SEND: &'static str = "takt_stream_send";

    /// Schreibt die Deklarationen in den Modulkopf.
    pub fn declare(m: &mut Module) {
        m.declare("\n; Stroeme (8.6, 8.8, 9.6); die Puffer gehoeren der Runtime");
        m.declare(&format!("declare i32 @{}(i32, i64)", Streams::COUNT));
        m.declare(&format!("declare i64 @{}(i32, i64, i32, ptr)", Streams::AT));
        m.declare(&format!("declare void @{}(i32, i64)", Streams::EXAMINED));
        m.declare(&format!("declare i1 @{}(i32, ptr, i32)", Streams::SEND));
    }
}
