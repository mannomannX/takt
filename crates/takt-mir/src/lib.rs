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

/// Ein Enum ohne Nutzlast mit der Liste aller Varianten (`ALL`, in der
/// Ordnung der Deklaration). Die Liste entsteht aus der Deklaration: Eine
/// neue Variante steht darin, ohne dass jemand sie nachfuehrt (Schritt 25;
/// `Accessor::ALL` von Hand fehlten `peek` und `sent`).
macro_rules! with_all {
    ($(#[$m:meta])* $vis:vis enum $name:ident { $($(#[$vm:meta])* $v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $($(#[$vm])* $v),*
        }

        impl $name {
            /// Alle Varianten, in der Ordnung der Deklaration.
            pub const ALL: [$name; [$(stringify!($v)),*].len()] = [$($name::$v),*];
        }
    };
}

pub mod analysis;
pub mod bytes;
pub mod capability;
pub mod census;
pub mod desugar;
pub mod dfa;
pub mod dump;
pub mod expr;
pub mod fns;
pub mod format;
pub mod hardware;
pub mod hash;
pub mod ids;
pub mod machine;
pub mod pattern;
pub mod persist;
pub mod program;
pub mod requirements;
pub mod review;
pub mod sample;
pub mod scan;
pub mod stmt;
pub mod sys;
pub mod types;
pub mod visit;

pub use desugar::desugar;
pub use format::{FormatError, decode_body, encode_body, read_program, write_program};
pub use hash::{Hash256, logic_hash, program_hash};
pub use ids::*;
pub use program::{Config, Program, SourceLines};
