//! `takt prove` (Referenz 13.3, plan/m6.md 2.8): die Schrittfunktion als
//! Transitionssystem, SMT-LIB2-Export und — mit einem Solver auf dem PATH —
//! k-Induktion. Der Interpreter ist die Spezifikation: Die Kodierung wird
//! konkret gegen ihn ausgefuehrt (`tests/against_interpreter.rs`), und ein
//! Gegenbeispiel des Solvers muss dort die Eigenschaft tatsaechlich
//! verletzen (`solve.rs`).

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
    CheckReport, CheckVerdict, ContractReport, ContractVerdict, Report, Solver, TransitionReport, Verdict, classify,
    classify_compositional, classify_transitions, fault_at, find, named, prove, taken_at, verify_contracts,
};
