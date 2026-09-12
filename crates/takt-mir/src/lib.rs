//! Mittlere IR von Takt (plan/mir.md): strukturiert, vollstaendig typisiert,
//! entzuckert, mit Knoten fuer die volle Referenz.
//!
//! - `program`, `types`, `fns`, `machine`, `stmt`, `expr`, `pattern`: die Knoten.
//! - `analysis`: das statische Gate aus M3 (Intervalle, Verengung, Kosten, Groesse).
//! - `desugar`: Sequenzen nach 6.2 in Zustaende (Oberflaeche → Kern-MIR).
//! - `format`: das Dateiformat `TAKT-MIR` (grammar/mir-format.md).
//! - `hash`: SHA-256 und Logik-Hash (11.3, 2.5).
//! - `dump`: Textform fuer Tests und Diagnosen.
//! - `sample`: ein Programm mit jeder Knotenart (Roundtrip-Test).
//!
//! Knoten werden gebaut und gelesen; grosse Varianten neben kleinen sind
//! gewollt (Zugriff ohne Indirektion).
#![allow(clippy::large_enum_variant)]

pub mod analysis;
pub mod desugar;
pub mod dfa;
pub mod dump;
pub mod expr;
pub mod fns;
pub mod format;
pub mod hash;
pub mod ids;
pub mod machine;
pub mod pattern;
pub mod program;
pub mod sample;
pub mod stmt;
pub mod types;

pub use desugar::desugar;
pub use format::{FormatError, decode_body, encode_body, read_program, write_program};
pub use hash::{Hash256, logic_hash, program_hash};
pub use ids::*;
pub use program::{Config, Program};
