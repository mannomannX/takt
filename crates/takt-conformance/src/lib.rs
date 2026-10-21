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
//! (`takt-llvm/src/abi.rs`, `image.rs`, `stream.rs`). Auf dem Wirt stellt
//! sie derselbe Produktrahmen wie auf jedem Board (`takt_frame::mcu`), mit
//! dem Kern des SDK; `testhost` erzeugt dazu den Wirt in C, dessen Treiber
//! den Stimulus liefern (M11 Schritt 9). C, weil C dieselbe ABI spricht wie
//! der erzeugte Code und clang alles in einem Zug uebersetzt — ein
//! Rust-Gegenstueck braeuchte `unsafe` und `extern "C"`, was der Workspace
//! verbietet.

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

#[cfg(feature = "board")]
pub mod bench;
#[cfg(feature = "board")]
pub mod board;
pub mod bringup;
pub mod cases;
pub mod couple;
mod deliveries;
pub mod esp_idf;
pub mod generated;
pub mod harness;
pub mod limits;
pub mod math;
pub mod natives;
pub mod paths;
#[cfg(feature = "board")]
pub mod port;
pub mod report;
pub mod run;
pub mod stimulus;
pub mod suites;
pub mod testhost;
pub mod wire;
pub mod zephyr;

pub use limits::LIMITS;
pub use run::{Difference, compare};
pub use stimulus::Stimulus;

/// Das Zielverzeichnis von `cargo`: `CARGO_TARGET_DIR`, sonst `target` in
/// der Wurzel des Repositorys.
pub fn target_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| repo_root().join("target"), Into::into)
}

/// Die Wurzel des Repositorys, aus dem dieser Lauf seine Quellen liest:
/// `TAKT_ROOT`, sonst der Ort beim Bau. Ein Test-Binary aus einem
/// nextest-Archiv laeuft woanders, als es gebaut wurde (M11 Schritt 34e:
/// gebaut auf einem Runner, gefahren am Board), und `env!` hielte den Pfad
/// des Runners fest; `tools/board-run.ps1` nennt darum seinen Worktree.
pub fn repo_root() -> std::path::PathBuf {
    std::env::var_os("TAKT_ROOT")
        .map_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."), Into::into)
}
