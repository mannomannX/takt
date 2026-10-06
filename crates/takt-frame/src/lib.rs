//! Der C-Rahmen um den erzeugten Code (12.1, 12.11).
//!
//! Der erzeugte Code rechnet den Schritt einer Maschine; alles darum herum
//! — Prozessabbild, Latch und Parameter, Ringe der Stroeme, der Treiberrand,
//! geplante Ausgaben, Jobs, die Reihenfolge eines Ticks — ist der Rahmen.
//! Dieses Crate erzeugt ihn als C-Quelltext fuer ein Programm.
//!
//! **Warum C.** Der erzeugte Code spricht die C-ABI, und clang uebersetzt
//! beides in einem Zug. Ein Rahmen in Rust braeuchte `extern "C"` und
//! `unsafe` fuer jeden Zeiger, und das verbietet der Workspace (13.4). Der
//! Quelltext bleibt zudem lesbar fuer die Pruefung der TCB.
//!
//! **Was hier steht.** `mcu` setzt den Rahmen zusammen, den die Boards und
//! `takt-bringup-host` binden; `parts` sind seine Bausteine, die auch der
//! Wirtsrahmen des Differentials in `takt-conformance` nutzt; `layout`,
//! `streams` und `edge` beschreiben Abbild, Stroeme und Rand; `text` fuegt
//! den Rahmen um die Arena zusammen (12.11). `drivers` nennt die Treiber eines
//! Programms in jeder Form (12.6), `embed` schreibt das Rust-Modul der
//! Lieferform (`P.rs`, 12.11).

pub mod drivers;
pub mod edge;
pub mod embed;
pub mod layout;
pub mod linker;
pub mod mcu;
pub mod parts;
pub mod streams;
pub mod text;
