//! `takt prove` (Referenz 13.3, plan/m6.md 2.8): die Schrittfunktion als
//! Transitionssystem, SMT-LIB2-Export und — mit einem Solver auf dem PATH —
//! k-Induktion. Der Interpreter ist die Spezifikation: Die Kodierung wird
//! konkret gegen ihn ausgefuehrt (`tests/against_interpreter.rs`), und ein
//! Gegenbeispiel des Solvers muss dort die Eigenschaft tatsaechlich
//! verletzen (`solve.rs`).

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

pub mod encode;
pub mod eval;
pub mod run;
pub mod smt;
pub mod solve;
pub mod support;
pub mod term;

pub use encode::{
    CheckSite, ContractGoal, Goal, Model, Position, StateVar, Unsupported, encode, encode_machine, encode_scenario,
};
pub use eval::Val;
pub use run::{Stop, UNMODELLED, mismatches};
pub use smt::export;
pub use solve::{
    CheckReport, CheckVerdict, ContractReport, ContractVerdict, DEEPEST, Report, Solver, TransitionReport, Verdict,
    classify, classify_compositional, classify_passes, classify_sites, classify_transitions, fault_at, find, named,
    prove, taken_at, verify_contracts,
};
