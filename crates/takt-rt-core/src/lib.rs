//! Die Tickschleife (Referenz 12.1), `no_std` und ohne Allokation.
//!
//! 12.1 gibt dreizehn Schritte vor. Neun davon sind Semantik und stehen
//! dort, wo die Semantik steht — im Interpreter oder im erzeugten Code.
//! Die uebrigen vier sind *Zeit*, und nur sie stehen hier:
//!
//! | 12.1 | wer |
//! |---|---|
//! | `wait_for_tick_boundary()` | hier, ueber [`Clock`] |
//! | `sample_inputs()`, `validate_and_bound()` | `takt-hal` |
//! | `deliver_streams()` bis `commit_outputs()` | die Semantik |
//! | `record_and_telemeter()` | hier, ueber [`Sink`] |
//! | `kick_watchdog()` | hier, ueber [`Watchdog`] |
//! | `maybe_sleep()` | hier, nach 9.9 |
//!
//! **Warum der Kern die Semantik nicht kennt.** Der Interpreter fuehrt
//! seinen Tick ueber `Value`, der erzeugte Code ueber getypte Strukturen
//! (11.2). Ein Kern, der beides koennte, waere generisch ueber die
//! Semantik — oder er kennte eine von beiden und passte nicht zur anderen.
//! Er ruft darum [`Program::tick`] und weiss nicht, was dahinter liegt.
//!
//! **Warum `no_std`.** M5 baut `baremetal` auf denselben Kern (plan/m4.md
//! 2.6). Ein Kern, der `std` braucht, waere dort neu zu schreiben, und
//! zwei Tickschleifen driften auseinander — der Fehler, den 5.2 fuer
//! Treiber ausschliesst, auf der Ebene darueber.

#![no_std]

pub mod loopcore;
pub mod overrun;
pub mod profile;
pub mod stream;

pub use loopcore::{Clock, Program, Runtime, Sink, Tick, Watchdog};
pub use overrun::{Overrun, Policy, Seen};
pub use profile::Profile;
