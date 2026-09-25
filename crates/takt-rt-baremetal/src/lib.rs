//! Profilaufsatz `baremetal` (Referenz 12.3, 12.8), `no_std`.
//!
//! **Was hier steht und was nicht.** Der Kern (`takt-rt-core`) kennt die
//! Schleife und sonst nichts — kein Board, keine MIR, keine
//! Abhaengigkeiten. Dieses Crate legt darauf, was 12.3 fuer eine MCU
//! verlangt: den Tick aus dem Timer-Interrupt, die gemessene Periode, den
//! Schutzbereich unter dem Stack, den Schlaf. Was ein Register anfasst,
//! steht auch hier nicht — es steht hinter den Traits in [`board`] und
//! kommt aus dem Board-Crate.
//!
//! Der Schnitt folgt `takt-hal`: Dort ist der Simulationstreiber eine
//! Trait-Implementierung und kein zweiter Programmpfad (8.3). Hier ist es
//! dieselbe Konstruktion eine Ebene tiefer, mit demselben Zweck — **eine
//! Tickschleife, nicht eine je Board.** Der Test dafuer ist mechanisch und
//! laeuft in diesem Crate: Die Attrappen in den Modultests belegen, dass
//! es ohne jedes Board-Crate baut und laeuft.
//!
//! **Der Unterschied zu `linux_rt` in einem Satz:** Dort bestimmt die
//! Runtime den Takt und schlaeft bis zur Deadline (12.2); hier bestimmt
//! ihn der Timer und die Schleife wartet auf ihn (12.3). Dieselbe Frage,
//! entgegengesetzte Richtung — siehe [`clock`].
//!
//! **Eine Schleife, eine Telemetrie.** [`run`] fuehrt den Lauf ueber der
//! Runtime, [`Telemetry`] den Ring vor der Leitung — beides ohne Board,
//! das nur Uhr, Leitung ([`Port`]) und Journal stellt.
//!
//! **Die Zeitgarantie ist eine andere.** 12.8 fuehrt `linux_rt` mit
//! „empirisch (Konformitaetsmessung)" und `baremetal` mit „statisch
//! (Budget × kalibrierte Tabelle) plus Messung". Das Statische entsteht
//! im Compiler (9.4.3, 7.2), nicht hier; dieses Crate liefert die Messung
//! daneben, die es bestaetigt.

#![no_std]

pub mod bench;
pub mod board;
pub mod clock;
pub mod run;
pub mod telemetry;
pub mod tolerance;

pub use board::{HardwareWatchdog, Sleep, StackGuard, TickSource};
pub use clock::{LogicalClock, TimerClock};
pub use run::{Cadence, JournalStats, NoWatchdog, Stats, Traced, report, run};
pub use telemetry::{DRAIN_ROUNDS, Port, Telemetry};
pub use tolerance::Period;
