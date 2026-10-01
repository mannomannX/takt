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
//! Stufe 2 sind die transzendenten Funktionen `exp`, `log`, `sin`, `cos`,
//! `tan`, `asin`, `acos`, `atan`, `atan2` und `pow`, korrekt gerundet nach
//! dem Ansatz von CORE-MATH: eine Naeherung, deren Fehler beweisbar unter
//! dem Abstand jedes Ergebnisses zur naechsten Rundungsgrenze liegt. Sie
//! rechnen nur mit Ganzzahlen (`big.rs`, `elem.rs`) und brauchen darum
//! weder `std` noch eine FPU. Die Sonderwerte folgen IEEE 754-2019 9.2;
//! NaN ist immer dasselbe ruhige NaN.
//!
//! # Keine Panics
//!
//! 13.8 verlangt Panic-Freiheit unter Fuzzing. Keine Funktion dieses
//! Crates allokiert oder rechnet mit `unwrap`; indiziert wird nur in Felder
//! fester Laenge mit Grenzen aus der Konstruktion, und Tabellenindizes sind
//! auf ihren Bereich geklemmt. Jede Funktion nimmt jedes Bitmuster entgegen,
//! einschliesslich NaN und Unendlich. Die
//! Frage, ob ein Ergebnis endlich sein *muss*, beantwortet der Aufrufer
//! (4.1: `ArithmeticFault(NonFinite)`), nicht die Bibliothek.

// Ohne das Feature `std` ist das Crate `no_std` — dann fehlen `sqrt`,
// `fma` und die Rundungen, weil `core` sie auf stabilem Rust nicht hat
// (plan/m4.md 2.3c). Mit dem Feature (Standard) nutzt es die
// Hardwarebefehle, die 4.2 verlangt.
#![cfg_attr(not(feature = "std"), no_std)]

mod big;
mod elem;
mod exact;
#[cfg(feature = "std")]
pub mod mat;
mod table;

use big::{F32, F64};
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
        Fun::Abs
        | Fun::CopySign
        | Fun::Sin
        | Fun::Cos
        | Fun::Tan
        | Fun::Asin
        | Fun::Acos
        | Fun::Atan
        | Fun::Atan2
        | Fun::Exp
        | Fun::Log
        | Fun::Pow => Curated::Yes,
        Fun::Sqrt | Fun::Fma | Fun::Round | Fun::Floor | Fun::Ceil | Fun::Trunc => {
            if cfg!(feature = "std") {
                Curated::Yes
            } else {
                Curated::NotYet
            }
        }
    }
}

/// Fehler eines Aufrufs ueber die einheitliche Schnittstelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported {
    /// Die nicht kuratierte Funktion.
    pub fun: Fun,
}

// ------------------------------------------------- Stufe 2: transzendent
//
// Je Funktion ein Einstieg je Breite; `f32` rechnet mit zwei Woertern
// Mantisse, `f64` mit vier (`elem.rs`). Das Bitmuster geht hinein und
// heraus — kein Schritt dazwischen benutzt Gleitkommaarithmetik.

macro_rules! unary {
    ($(#[$doc:meta])* $f64:ident, $f32:ident, $imp:ident) => {
        $(#[$doc])*
        pub fn $f64(x: f64) -> f64 {
            f64::from_bits(elem::$imp::<4>(x.to_bits(), F64))
        }

        $(#[$doc])*
        pub fn $f32(x: f32) -> f32 {
            f32::from_bits(elem::$imp::<2>(u64::from(x.to_bits()), F32) as u32)
        }
    };
}

unary!(
    /// e^x, korrekt gerundet.
    exp_f64, exp_f32, exp
);
unary!(
    /// Natuerlicher Logarithmus, korrekt gerundet; ln(±0) = −∞, NaN unter null.
    log_f64, log_f32, log
);
unary!(
    /// Sinus, korrekt gerundet fuer jedes endliche Argument.
    sin_f64, sin_f32, sin
);
unary!(
    /// Kosinus, korrekt gerundet fuer jedes endliche Argument.
    cos_f64, cos_f32, cos
);
unary!(
    /// Tangens, korrekt gerundet fuer jedes endliche Argument.
    tan_f64, tan_f32, tan
);
unary!(
    /// Arkussinus, korrekt gerundet; NaN ausserhalb [−1, 1].
    asin_f64, asin_f32, asin
);
unary!(
    /// Arkuskosinus, korrekt gerundet; NaN ausserhalb [−1, 1].
    acos_f64, acos_f32, acos
);
unary!(
    /// Arkustangens, korrekt gerundet.
    atan_f64, atan_f32, atan
);

/// Der Winkel von (x, y), korrekt gerundet; Nullen und Unendlichkeiten nach
/// IEEE 754-2019 9.2.1.
pub fn atan2_f64(y: f64, x: f64) -> f64 {
    f64::from_bits(elem::atan2::<4>(y.to_bits(), x.to_bits(), F64))
}

/// Der Winkel von (x, y), korrekt gerundet; Nullen und Unendlichkeiten nach
/// IEEE 754-2019 9.2.1.
pub fn atan2_f32(y: f32, x: f32) -> f32 {
    f32::from_bits(elem::atan2::<2>(u64::from(y.to_bits()), u64::from(x.to_bits()), F32) as u32)
}

/// x^y, korrekt gerundet, auch dort, wo das Ergebnis genau zwischen zwei
/// Gleitkommazahlen liegt; Sonderfaelle nach IEEE 754-2019 9.2.1.
pub fn pow_f64(x: f64, y: f64) -> f64 {
    f64::from_bits(elem::pow::<4>(x.to_bits(), y.to_bits(), F64))
}

/// x^y, korrekt gerundet, auch dort, wo das Ergebnis genau zwischen zwei
/// Gleitkommazahlen liegt; Sonderfaelle nach IEEE 754-2019 9.2.1.
pub fn pow_f32(x: f32, y: f32) -> f32 {
    f32::from_bits(elem::pow::<2>(u64::from(x.to_bits()), u64::from(y.to_bits()), F32) as u32)
}
