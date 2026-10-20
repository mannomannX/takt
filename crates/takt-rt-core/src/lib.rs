//! Die Tickschleife (Referenz 12.1), `no_std` und ohne Allokation.
//!
//! 12.1 gibt dreizehn Schritte vor. Neun davon sind Semantik und stehen
//! dort, wo die Semantik steht — im Interpreter oder im erzeugten Code.
//! Die uebrigen vier sind *Zeit*, und nur sie stehen hier:
//!
//! | 12.1 | wer |
//! |---|---|
//! | `wait_for_tick_boundary()` | der Port; in der Form „eigener Kern“ hier, ueber [`Clock`] |
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

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![no_std]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        reason = "Stufe 2 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]

pub mod journal;
pub mod loopcore;
pub mod overrun;
pub mod port_test;
pub mod profile;
pub mod protection;
pub mod stream;

pub use journal::{
    FakeNvm, Fits, Journal, Loaded, Memory, NoDevice, NoJournal, Persist, Persisting, Store, Unloaded, Volatile,
};
pub use loopcore::{
    Clock, JobState, Jobs, Next, NextRun, Nvm, NvmState, Outputs, Program, Runtime, Sink, Tick, Tolerance, Tunables,
    Watchdog, tick_end,
};
pub use overrun::{Overrun, Policy, Seen};
pub use profile::Profile;
