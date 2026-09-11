//! Codegen nach LLVM-IR (Referenz 11.2), als Text.
//!
//! **Warum Text und nicht `inkwell`** (plan/m4.md 4.1, revidiert): Die
//! Zertifizierung (13.4) verlangt ein Artefakt, das sich pruefen und
//! archivieren laesst; reproduzierbare Builds (11.3) werden ueber einen
//! Diff der IR belegbar statt ueber zwei Binaries; und der Compiler bleibt
//! ohne LLVM-Installation baubar und testbar. LLVM wird zum Werkzeug der
//! Abnahme (Schritt 8), nicht zur Bibliothek des Compilers.
//!
//! **Die strikte FP-Semantik ist hier eine Abwesenheit.** 4.2 verlangt
//! keine Fast-Math-Flags, `contract=off`, keine Reassoziation. In der
//! Textform heisst das: Eine `fadd`-Zeile traegt *kein* Flag. Das ist
//! vollstaendig pruefbar — `tests/strict_fp.rs` durchsucht die erzeugte
//! IR nach den sieben Flags und schlaegt bei jedem Vorkommen fehl.
//!
//! - `ty`: Typen der MIR nach LLVM-Typen (3.4, 11.2).
//! - `emit`: der Textpuffer, der die IR aufbaut.
//! - `expr`: Ausdruecke.

pub mod emit;
pub mod expr;
pub mod ty;

pub use emit::{Module, Reg};
pub use expr::Lowered;
pub use ty::LlvmType;
