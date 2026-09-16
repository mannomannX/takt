//! Korrekt gerundete Mathematik beider Breiten (Referenz 4.2).
//!
//! Satz 9.4.4 verlangt bitidentische Ergebnisse auf jedem Target. Die
//! Standardbibliothek der Plattform kann das nicht zusagen: `f64::sin`
//! ruft die libm des Systems, und glibc, musl und die ARM-Bibliotheken
//! unterscheiden sich im letzten Bit. Darum rechnet Takt seine Mathematik
//! selbst — auf dem Host im Interpreter und auf dem Target im erzeugten
//! Code aus derselben Quelle.
//!
//! # Die kuratierte Menge
//!
//! 13.8 legt die Regel fest: Eine Funktion wird aufgenommen, *nachdem*
//! ihre Bit-Gleichheit belegt ist — nicht davor. [`Curated`] sagt je
//! Funktion, ob sie es ist; wer eine nicht kuratierte aufruft, bekommt
//! [`Unsupported`] und keinen Zahlenwert. Das ist der Unterschied zu
//! heute: Eine Plattformfunktion liefert stillschweigend *ein* Ergebnis
//! und behauptet damit eine Zusage, die sie nicht halten kann.
//!
//! Stufe 1 sind die Funktionen, deren Ergebnis ohne eigene Approximation
//! feststeht — IEEE-754 schreibt es vor:
//!
//! | Funktion | Grundlage |
//! |---|---|
//! | `sqrt` | IEEE-754-Operation, korrekt gerundet |
//! | `fma` | IEEE-754-2008 fusedMultiplyAdd |
//! | `round`, `floor`, `ceil`, `trunc` | exakt, nur Rundungsmodus |
//! | `abs`, `copysign` | exakt, Vorzeichenbits |
//!
//! Die transzendenten Funktionen (`sin`, `cos`, `tan`, `asin`, `acos`,
//! `atan`, `atan2`, `exp`, `log`, `pow`) folgen nach dem Muster von
//! CORE-MATH; bis dahin melden sie [`Unsupported`].
//!
//! # Keine Panics
//!
//! 13.8 verlangt Panic-Freiheit unter Fuzzing. Keine Funktion dieses
//! Crates indiziert, allokiert oder rechnet mit `unwrap`; jede nimmt
//! jedes Bitmuster entgegen, einschliesslich NaN und Unendlich. Die
//! Frage, ob ein Ergebnis endlich sein *muss*, beantwortet der Aufrufer
//! (4.1: `ArithmeticFault(NonFinite)`), nicht die Bibliothek.

// Ohne das Feature `std` ist das Crate `no_std` — dann fehlen `sqrt`,
// `fma` und die Rundungen, weil `core` sie auf stabilem Rust nicht hat
// (plan/m4.md 2.3c). Mit dem Feature (Standard) nutzt es die
// Hardwarebefehle, die 4.2 verlangt.
#![cfg_attr(not(feature = "std"), no_std)]

mod cost;
mod exact;
#[cfg(feature = "std")]
pub mod mat;

pub use cost::{Cost, cost_of};
#[cfg(feature = "std")]
pub use exact::{
    ceil_f32, ceil_f64, floor_f32, floor_f64, fma_f32, fma_f64, round_f32, round_f64, sqrt_f32, sqrt_f64, trunc_f32,
    trunc_f64,
};
pub use exact::{copysign_f32, copysign_f64, fabs_f32, fabs_f64};

/// Eine Funktion der Bibliothek, unabhaengig von der Breite.
///
/// Die Reihenfolge entspricht `takt_mir::expr::Intrinsic`, damit der
/// Aufrufer ohne Tabelle uebersetzen kann; `takt-mir` haengt nicht von
/// diesem Crate ab und dieses nicht von jenem — beide zu koppeln hiesse,
/// `no_std` und Fliesskomma-Freiheit gegeneinander zu tauschen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs)]
pub enum Fun {
    Sqrt,
    Fma,
    Round,
    Floor,
    Ceil,
    Trunc,
    Abs,
    CopySign,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Exp,
    Log,
    Pow,
}

impl Fun {
    /// Der Name, wie ihn die Referenz schreibt (4.2).
    pub fn name(self) -> &'static str {
        match self {
            Fun::Sqrt => "sqrt",
            Fun::Fma => "fma",
            Fun::Round => "round",
            Fun::Floor => "floor",
            Fun::Ceil => "ceil",
            Fun::Trunc => "trunc",
            Fun::Abs => "abs",
            Fun::CopySign => "copysign",
            Fun::Sin => "sin",
            Fun::Cos => "cos",
            Fun::Tan => "tan",
            Fun::Asin => "asin",
            Fun::Acos => "acos",
            Fun::Atan => "atan",
            Fun::Atan2 => "atan2",
            Fun::Exp => "exp",
            Fun::Log => "log",
            Fun::Pow => "pow",
        }
    }

    /// Alle Funktionen, fuer Tests und Berichte.
    pub const ALL: [Fun; 18] = [
        Fun::Sqrt,
        Fun::Fma,
        Fun::Round,
        Fun::Floor,
        Fun::Ceil,
        Fun::Trunc,
        Fun::Abs,
        Fun::CopySign,
        Fun::Sin,
        Fun::Cos,
        Fun::Tan,
        Fun::Asin,
        Fun::Acos,
        Fun::Atan,
        Fun::Atan2,
        Fun::Exp,
        Fun::Log,
        Fun::Pow,
    ];
}

/// Stand einer Funktion in der kuratierten Menge (13.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Curated {
    /// Bit-Gleichheit ist belegt; die Funktion darf benutzt werden.
    Yes,
    /// Noch keine korrekt gerundete Implementierung. Der Aufrufer meldet
    /// das als Stufe, statt ein Ergebnis zu erfinden.
    NotYet,
}

/// Ob eine Funktion kuratiert ist.
///
/// Stufe 1 sind die exakten Operationen: Ihr Ergebnis schreibt IEEE-754
/// vor, sie brauchen keine eigene Approximation und sind damit auf jedem
/// konformen Target dasselbe.
///
/// `sqrt`, `fma` und die Rundungen haengen am Feature `std` — ohne es
/// gibt es sie nicht (plan/m4.md 2.3c), und dann sind sie auch nicht
/// kuratiert. Die Antwort dieser Funktion und das, was das Crate
/// anbietet, duerfen nicht auseinanderlaufen: Sonst meldete der Compiler
/// eine Funktion als verfuegbar, die beim Aufruf fehlt.
pub fn curated(f: Fun) -> Curated {
    match f {
        Fun::Abs | Fun::CopySign => Curated::Yes,
        Fun::Sqrt | Fun::Fma | Fun::Round | Fun::Floor | Fun::Ceil | Fun::Trunc => {
            if cfg!(feature = "std") {
                Curated::Yes
            } else {
                Curated::NotYet
            }
        }
        Fun::Sin
        | Fun::Cos
        | Fun::Tan
        | Fun::Asin
        | Fun::Acos
        | Fun::Atan
        | Fun::Atan2
        | Fun::Exp
        | Fun::Log
        | Fun::Pow => Curated::NotYet,
    }
}

/// Fehler eines Aufrufs ueber die einheitliche Schnittstelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported {
    /// Die nicht kuratierte Funktion.
    pub fun: Fun,
}
