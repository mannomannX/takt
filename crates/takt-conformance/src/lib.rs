//! Differentielles Testen: Interpreter gegen erzeugten Code (13.8, 9.4.4).
//!
//! **Satz 9.4.4 sagt: Simulation und Hardware liefern dieselben Outputs.**
//! Der Interpreter ist die Spezifikation (Prinzip 2 des Plans), der
//! erzeugte Code die zweite Implementierung derselben Semantik. Weichen
//! sie ab, ist einer von beiden falsch — und weil der Interpreter das
//! Orakel ist, ist es der Codegen.
//!
//! **Verglichen werden Outputs, nicht Zustaende.** 9.4.4 spricht von
//! Outputs, und plan/m4.md 2.5 hat daraus eine Entscheidung gemacht: Der
//! Trace ist die Abnahme, der Zustands-Hash ein Diagnosewerkzeug. Ein
//! Vergleich der Zustaende waere schaerfer, als die Zusage lautet — und
//! eine Abweichung im Zustand, die keinen Output erreicht, waere kein
//! Fehler, sondern eine andere Darstellung derselben Semantik.
//!
//! **Wie der erzeugte Code laeuft.** Er braucht eine Umgebung: das
//! Prozessabbild, den Parametervektor, den Latch und die Runtime-Aufrufe
//! (`takt-llvm/src/abi.rs`, `image.rs`, `stream.rs`). `harness` erzeugt
//! sie als C, weil C dieselbe ABI spricht wie der erzeugte Code und weil
//! clang beides in einem Zug uebersetzt — ein Rust-Gegenstueck braeuchte
//! `unsafe` und `extern "C"`, was der Workspace verbietet.

#[cfg(feature = "board")]
pub mod bench;
#[cfg(feature = "board")]
pub mod board;
pub mod harness;
pub mod layout;
pub mod limits;
pub mod mcu;
pub mod natives;
pub mod report;
pub mod run;
pub mod stimulus;
pub mod streams;

pub use harness::Harness;
pub use limits::LIMITS;
pub use run::{Difference, compare};
pub use stimulus::Stimulus;
