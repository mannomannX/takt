//! Stufe 1: Funktionen, deren Ergebnis IEEE-754 vorschreibt.
//!
//! Sie brauchen keine eigene Approximation und sind damit auf jedem
//! konformen Target dasselbe — das ist der Grund, mit ihnen anzufangen
//! (plan/m4.md 2.3b). `sqrt` und `fma` sind IEEE-754-Operationen,
//! `round`/`floor`/`ceil`/`trunc` waehlen nur einen Rundungsmodus, und
//! `abs`/`copysign` setzen Vorzeichenbits.
//!
//! Ohne das Feature `std` gibt es sie nicht: Die vier ersten sind
//! LLVM-Intrinsics, die `core` auf stabilem Rust nicht anbietet
//! (plan/m4.md 2.3c). Sie dann in Software zu schreiben, hiesse den
//! Rueckfall vor dem Normalfall zu bauen — 4.2 verlangt Hardware, „wo
//! vorhanden".
//!
//! Keine Funktion hier prueft auf Endlichkeit. Ob ein Ergebnis endlich
//! sein muss, entscheidet der Aufrufer (4.1); die Bibliothek rechnet.

/// Quadratwurzel, korrekt gerundet (IEEE-754).
#[cfg(feature = "std")]
pub fn sqrt_f64(x: f64) -> f64 {
    x.sqrt()
}

/// Quadratwurzel, korrekt gerundet (IEEE-754).
#[cfg(feature = "std")]
pub fn sqrt_f32(x: f32) -> f32 {
    x.sqrt()
}

/// `a * b + c` mit einer einzigen Rundung (IEEE-754-2008, 4.2).
#[cfg(feature = "std")]
pub fn fma_f64(a: f64, b: f64, c: f64) -> f64 {
    a.mul_add(b, c)
}

/// `a * b + c` mit einer einzigen Rundung (IEEE-754-2008, 4.2).
#[cfg(feature = "std")]
pub fn fma_f32(a: f32, b: f32, c: f32) -> f32 {
    a.mul_add(b, c)
}

/// Zur naechsten ganzen Zahl, halbe Werte vom Nullpunkt weg.
#[cfg(feature = "std")]
pub fn round_f64(x: f64) -> f64 {
    x.round()
}

/// Zur naechsten ganzen Zahl, halbe Werte vom Nullpunkt weg.
#[cfg(feature = "std")]
pub fn round_f32(x: f32) -> f32 {
    x.round()
}

/// Groesste ganze Zahl kleiner oder gleich `x`.
#[cfg(feature = "std")]
pub fn floor_f64(x: f64) -> f64 {
    x.floor()
}

/// Groesste ganze Zahl kleiner oder gleich `x`.
#[cfg(feature = "std")]
pub fn floor_f32(x: f32) -> f32 {
    x.floor()
}

/// Kleinste ganze Zahl groesser oder gleich `x`.
#[cfg(feature = "std")]
pub fn ceil_f64(x: f64) -> f64 {
    x.ceil()
}

/// Kleinste ganze Zahl groesser oder gleich `x`.
#[cfg(feature = "std")]
pub fn ceil_f32(x: f32) -> f32 {
    x.ceil()
}

/// Nachkommastellen abschneiden (zum Nullpunkt hin).
#[cfg(feature = "std")]
pub fn trunc_f64(x: f64) -> f64 {
    x.trunc()
}

/// Nachkommastellen abschneiden (zum Nullpunkt hin).
#[cfg(feature = "std")]
pub fn trunc_f32(x: f32) -> f32 {
    x.trunc()
}

// --------------------------------------------------------------- Bitweise
//
// Diese vier brauchen kein `std`: Sie setzen Vorzeichenbits und sind in
// `core` schreibbar. Sie stehen darum ohne `cfg` da — ein `no_std`-Bau
// hat wenigstens sie.

/// Betrag: das Vorzeichenbit loeschen. Auch fuer NaN definiert.
pub fn fabs_f64(x: f64) -> f64 {
    f64::from_bits(x.to_bits() & !SIGN_F64)
}

/// Betrag: das Vorzeichenbit loeschen. Auch fuer NaN definiert.
pub fn fabs_f32(x: f32) -> f32 {
    f32::from_bits(x.to_bits() & !SIGN_F32)
}

/// Betrag von `x` mit dem Vorzeichen von `y` (IEEE-754 `copySign`).
pub fn copysign_f64(x: f64, y: f64) -> f64 {
    f64::from_bits((x.to_bits() & !SIGN_F64) | (y.to_bits() & SIGN_F64))
}

/// Betrag von `x` mit dem Vorzeichen von `y` (IEEE-754 `copySign`).
pub fn copysign_f32(x: f32, y: f32) -> f32 {
    f32::from_bits((x.to_bits() & !SIGN_F32) | (y.to_bits() & SIGN_F32))
}

const SIGN_F64: u64 = 1 << 63;
const SIGN_F32: u32 = 1 << 31;
