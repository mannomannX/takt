//! `takt prove` (Referenz 13.3, plan/m6.md 2.8): die Schrittfunktion als
//! Transitionssystem, SMT-LIB2-Export und — mit einem Solver auf dem PATH —
//! k-Induktion. Der Interpreter ist die Spezifikation: Die Kodierung wird
//! konkret gegen ihn ausgefuehrt (`tests/against_interpreter.rs`), und ein
//! Gegenbeispiel muss dort die Eigenschaft tatsaechlich verletzen.

pub mod encode;
pub mod eval;
pub mod smt;
pub mod term;

pub use encode::{Goal, Model, StateVar, Unsupported, encode};
pub use eval::Val;
pub use smt::export;
