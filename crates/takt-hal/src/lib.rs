//! Treiberschnittstelle und defensiver Treiberrand (Referenz 12.6).
//!
//! Leitsatz aus 12.6: **Inputs degradieren, Outputs faulten.** Eine
//! Lieferung, der nicht zu trauen ist, wird `Bad`; das Programm entscheidet
//! ueber `.valid` und `.or()`, was das heisst, und die Erholung ist
//! automatisch. Nur wenn das *Stellen* scheitert, entsteht ein Fault.
//!
//! Der Kern steht ohne `std` und ohne Allokation, damit jede Runtime ihn
//! benutzt — der Interpreter, der Wirtsrahmen der Konformitaet und die
//! MCU-Rahmen ueber ihre C-Einstiege (FB-285):
//!
//! - `contract`: der Treibervertrag (Zeilen 1, 2), die Zaehlregel der
//!   Periode (Zeile 7) und das Urteil der Ausgabeseite (Zeile 6).
//! - `quality`: die Qualitaetsmaschine je Input — Range, `max_slew`,
//!   `debounce` (3.5; Zeilen 3, 4).
//!
//! Mit dem Feature `mir` (Standard) kommt hinzu, was ein Programm braucht:
//!
//! - `driver`: was ein Treiber koennen muss (Prinzip 4: Treiber sind Traits).
//! - `edge`: der Rand ueber einem Programm — Treiber je Kanal, Grenzen,
//!   `MAXPT` — fuer die Lieferungen eines Ticks.
//! - `sim`: der Simulationstreiber.
//!
//! **Warum der Rand hier steht und nicht im Treiber.** Er gilt fuer jeden
//! Treiber gleich; in jedem einzeln stuende er dreimal verschieden da, und
//! die Zusage aus 3.4 — dass deklarierte Ranges gueltige Annahmen der
//! Intervallanalyse sind — haengt daran, dass *kein* Treiber sie umgehen
//! kann. Ein Treiber liefert Rohdaten und wird geprueft, er prueft nicht
//! selbst.
//!
//! **Warum der Rand nicht mit `Value` rechnet.** Der Interpreter hat
//! `Value`, der erzeugte Code hat getypte Strukturen (11.2) — beide sollen
//! denselben Rand benutzen, sonst gilt Satz 9.4.4 fuer die Randfaelle
//! nicht. Die Pruefungen 3 und 4 brauchen vom Wert nur Ordnung und
//! Differenz; `Scalar` ist genau das und nicht mehr.

#![no_std]

/// Eine `#[repr(C)]`-Struktur, die der erzeugte C-Rahmen mit dem Kern teilt,
/// und ihre C-Seite aus derselben Feldliste (FB-397): `C_DECL` ist die
/// Deklaration, `C_LAYOUT` Groesse und Versaetze, die der Rahmen mit
/// `_Static_assert` gegen seinen Compiler haelt. Eine zweite Fassung in C
/// von Hand koennte still abweichen; der Linker sieht keine Typen.
macro_rules! shared_with_c {
    (
        $(#[$m:meta])*
        $vis:vis struct $name:ident as $c:literal {
            $($(#[$fm:meta])* $fvis:vis $field:ident : $ty:tt),* $(,)?
        }
    ) => {
        $(#[$m])*
        #[repr(C)]
        $vis struct $name {
            $($(#[$fm])* $fvis $field: $ty),*
        }

        impl $name {
            /// Die C-Deklaration, aus der Feldliste.
            pub const C_DECL: &'static str =
                concat!("struct ", $c, " { ", $(c_type!($ty), " ", stringify!($field), "; ",)* "};");
            /// Der C-Name und Groesse und Versaetze der Felder, wie Rust sie anlegt.
            pub const C_LAYOUT: $crate::CLayout = $crate::CLayout {
                name: $c,
                size: core::mem::size_of::<$name>(),
                fields: &[$((stringify!($field), core::mem::offset_of!($name, $field))),*],
            };
        }
    };
}

/// Der C-Typ eines Felds einer [`shared_with_c`]-Struktur. Nur Typen, deren
/// Breite auf jedem Ziel des Rahmens feststeht.
macro_rules! c_type {
    (bool) => {
        "_Bool"
    };
    (u8) => {
        "unsigned char"
    };
    (u32) => {
        "unsigned"
    };
    (i64) => {
        "long long"
    };
    (f64) => {
        "double"
    };
}

/// Groesse und Versaetze einer Struktur, die der C-Rahmen teilt.
#[derive(Clone, Copy, Debug)]
pub struct CLayout {
    /// Der C-Name (`takt_track`).
    pub name: &'static str,
    /// `size_of` in Rust.
    pub size: usize,
    /// Je Feld Name und `offset_of` in Rust.
    pub fields: &'static [(&'static str, usize)],
}

impl CLayout {
    /// Die Pruefungen fuer den C-Compiler: Weicht sein Layout ab, bricht
    /// der Bau, statt dass der Rand still falsche Felder liest.
    pub fn static_asserts(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        let n = self.name;
        writeln!(out, "_Static_assert(sizeof(struct {n}) == {}, \"{n}: Groesse wie in takt-hal\");", self.size)?;
        for (field, offset) in self.fields {
            writeln!(
                out,
                "_Static_assert(offsetof(struct {n}, {field}) == {offset}, \"{n}.{field}: Versatz wie in takt-hal\");"
            )?;
        }
        Ok(())
    }
}

#[cfg(feature = "mir")]
extern crate std;

pub mod contract;
#[cfg(feature = "mir")]
pub mod driver;
#[cfg(feature = "mir")]
pub mod edge;
pub mod quality;
#[cfg(feature = "mir")]
pub mod sim;

pub use contract::{Contract, Placement, Track, Window};
#[cfg(feature = "mir")]
pub use driver::{Delivery, Driver, Element, Reading, Writing};
#[cfg(feature = "mir")]
pub use edge::{Alert, Edge, Settled};
pub use quality::{Quality, Reason, Scalar};
