//! Treiberschnittstelle und defensiver Treiberrand (Referenz 12.6).
//!
//! Leitsatz aus 12.6: **Inputs degradieren, Outputs faulten.** Eine
//! Lieferung, der nicht zu trauen ist, wird `Bad`; das Programm entscheidet
//! ueber `.valid` und `.or()`, was das heisst, und die Erholung ist
//! automatisch. Nur wenn das *Stellen* scheitert, entsteht ein Fault.
//!
//! Der Kern steht ohne `std` und ohne Allokation, damit jede Runtime ihn
//! benutzt — der Interpreter, der Wirtsrahmen der Konformitaet und die
//! MCU-Rahmen ueber ihre C-Einstiege (FB-285):
//!
//! - `contract`: der Treibervertrag (Zeilen 1, 2), die Zaehlregel der
//!   Periode (Zeile 7) und das Urteil der Ausgabeseite (Zeile 6).
//! - `quality`: die Qualitaetsmaschine je Input — Range, `max_slew`,
//!   `debounce` (3.5; Zeilen 3, 4).
//!
//! Mit dem Feature `mir` (Standard) kommt hinzu, was ein Programm braucht:
//!
//! - `driver`: was ein Treiber koennen muss (Prinzip 4: Treiber sind Traits).
//! - `edge`: der Rand ueber einem Programm — Treiber je Kanal, Grenzen,
//!   `MAXPT` — fuer die Lieferungen eines Ticks.
//! - `sim`: der Simulationstreiber.
//!
//! **Warum der Rand hier steht und nicht im Treiber.** Er gilt fuer jeden
//! Treiber gleich; in jedem einzeln stuende er dreimal verschieden da, und
//! die Zusage aus 3.4 — dass deklarierte Ranges gueltige Annahmen der
//! Intervallanalyse sind — haengt daran, dass *kein* Treiber sie umgehen
//! kann. Ein Treiber liefert Rohdaten und wird geprueft, er prueft nicht
//! selbst.
//!
//! **Warum der Rand nicht mit `Value` rechnet.** Der Interpreter hat
//! `Value`, der erzeugte Code hat getypte Strukturen (11.2) — beide sollen
//! denselben Rand benutzen, sonst gilt Satz 9.4.4 fuer die Randfaelle
//! nicht. Die Pruefungen 3 und 4 brauchen vom Wert nur Ordnung und
//! Differenz; `Scalar` ist genau das und nicht mehr.

#![no_std]

#[cfg(feature = "mir")]
extern crate std;

pub mod contract;
#[cfg(feature = "mir")]
pub mod driver;
#[cfg(feature = "mir")]
pub mod edge;
pub mod quality;
#[cfg(feature = "mir")]
pub mod sim;

pub use contract::{Contract, Placement, Track, Window};
#[cfg(feature = "mir")]
pub use driver::{Delivery, Driver, Element, Reading, Writing};
#[cfg(feature = "mir")]
pub use edge::{Alert, Edge, Settled};
pub use quality::{Quality, Reason, Scalar};
