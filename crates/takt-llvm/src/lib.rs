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
//! - `abi`: was der erzeugte Code von der Runtime ruft (9.3, 5.4).
//! - `block`: Blockinstanzen und ihre Methoden (5.7).
//! - `collection`: `push`, `append`, `clear` auf `bytes`/`vec` (3.9).
//! - `fns`: reine Funktionen als LLVM-Funktionen (4.4).
//! - `image`: der Aufbau des Prozessabbilds, die ABI zur Runtime (9.1).
//! - `machine`: Zustands-Struct und Tickschritt einer Maschine (11.2).
//! - `stmt`: Anweisungen, Checks und der Fault-Zweig (11.2).
//! - `scope`: was der Codegen deckt, gemessen an echten Programmen.
//! - `step`: die Schrittfunktion einer Maschine (11.2, 9.4).
//! - `stream`: die Naht zu den Ereignisstroemen (8.6, 9.6).
//! - `target`: die Zielklassen (12.8).
//! - `wire`: das Drahtformat der Records, `decode`/`encode` (3.7).
//! - `toolchain`: `clang` finden, um die IR zu pruefen und auszufuehren.

pub mod abi;
pub mod block;
pub mod collection;
pub mod emit;
pub mod expr;
pub mod fns;
pub mod image;
pub mod machine;
pub mod scope;
pub mod step;
pub mod stmt;
pub mod stream;
pub mod target;
pub mod toolchain;
pub mod ty;
pub mod wire;

pub use emit::{Module, Reg};
pub use expr::Lowered;
pub use machine::{StateStruct, state_struct};
pub use target::Target;
pub use toolchain::Clang;
pub use ty::LlvmType;
