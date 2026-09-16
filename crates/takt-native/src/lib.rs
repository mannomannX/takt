//! Die kuratierte Menge nativer Funktionen (4.5).
//!
//! **Eine native Funktion ist eine Signatur mit drei Zusagen** (4.5):
//! `stack` (maximaler Bedarf in Byte, geht in 12.3 ein), `cost`
//! (abstrakte Operationen je Klasse, geht in das Budget 9.4.3 ein) und
//! `total` — keine Panics, Terminierung, bitreproduzierbare Ergebnisse
//! ueber alle Targets.
//!
//! **Sie gehoert zur TCB** (9.5). Das hat zwei Folgen, die man im Code
//! sieht: Dieses Crate hat keine Abhaengigkeit — jede waere Teil der TCB
//! —, und jede Funktion ist rein: kein Zustand, keine Ein- oder Ausgabe,
//! feste Groessen.
//!
//! **Die Aufnahme ist dieselbe Huerde wie bei `libtaktm`** (13.8): „erst
//! danach wird eine Funktion in die kuratierte Menge aufgenommen". Eine
//! Funktion ohne gruene Vektoren ist hier nicht sichtbar, und der
//! Compiler lehnt sie ab — wie er heute ein v1.1-Konstrukt ablehnt.
//!
//! **Warum Pruefsummen zuerst.** Sie sind exakt spezifiziert (ihre
//! Vektoren stehen in den Normen), sie kommen in jedem Protokoll vor, und
//! ihre Kosten sind linear in der Laenge — also als Vertrag angebbar. Die
//! Transformationen (`fft256`) und die Hashes brauchen dieselbe Huerde,
//! aber mehr Code; sie folgen.

#![no_std]

pub mod bytes;
pub mod cost;
pub mod crc;

pub use cost::{Cost, cost_of};

/// Eine Funktion der kuratierten Menge (4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Native {
    /// `crc32(b)`: CRC-32 nach IEEE 802.3, wie in ZIP und PNG.
    Crc32,
    /// `crc32c(b)`: CRC-32 nach Castagnoli (iSCSI, SCTP).
    Crc32c,
    /// `crc16(b)`: CRC-16/IBM, wie in Modbus RTU.
    Crc16,
    /// `sum8(b)`: die Summe der Bytes, modulo 256.
    Sum8,
}

impl Native {
    /// Der Name, unter dem das Programm sie ruft.
    pub fn name(self) -> &'static str {
        match self {
            Native::Crc32 => "crc32",
            Native::Crc32c => "crc32c",
            Native::Crc16 => "crc16",
            Native::Sum8 => "sum8",
        }
    }

    /// Alle Funktionen der Menge.
    pub const ALL: [Native; 4] = [Native::Crc32, Native::Crc32c, Native::Crc16, Native::Sum8];

    /// Die Funktion zu einem Namen.
    pub fn by_name(name: &str) -> Option<Native> {
        Native::ALL.into_iter().find(|n| n.name() == name)
    }
}

/// Rechnet eine Funktion der Menge ueber einem Byteblock.
///
/// Das Ergebnis ist `u64`, weil die Breiten der Funktionen sich
/// unterscheiden (u8 bis u32) und der Aufrufer sie aus der Signatur
/// kennt. Eine Aufzaehlung je Breite waere vier Funktionen, die dasselbe
/// tun.
pub fn apply(f: Native, bytes: &[u8]) -> u64 {
    match f {
        Native::Crc32 => u64::from(crc::crc32(bytes)),
        Native::Crc32c => u64::from(crc::crc32c(bytes)),
        Native::Crc16 => u64::from(crc::crc16(bytes)),
        Native::Sum8 => u64::from(crc::sum8(bytes)),
    }
}
