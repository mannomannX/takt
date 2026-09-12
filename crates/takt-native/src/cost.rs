//! Der Kostenvertrag der nativen Funktionen (4.5, 9.4.3).
//!
//! 4.5 verlangt `cost` als *Vertrag*: eine Konstante, die in das Budget
//! (9.4.3) eingeht. Sie ist keine Messung — sie ist eine Zusage, die die
//! Konformitaetssuite (13.8) prueft.
//!
//! **Die Kosten sind linear in der Laenge**, und das ist der Grund, warum
//! die Pruefsummen zuerst kamen: Ihre Schranke laesst sich angeben, ohne
//! etwas zu messen. `cost` nennt darum die Kosten *je Byte*; die Laenge
//! steht im Typ (`bytes<N>`), und das Budget multipliziert.

use crate::Native;

/// Die Kosten einer nativen Funktion (9.4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Cost {
    /// Abstrakte Operationen je Byte der Eingabe.
    pub per_byte: u64,
    /// Feste Kosten des Aufrufs.
    pub call: u64,
    /// Maximaler Stack-Bedarf in Byte (4.5, geht in 12.3 ein).
    pub stack: u32,
}

/// Der Kostenvertrag einer Funktion.
///
/// Die Zahlen sind die abgezaehlten Operationen der Implementierung, nicht
/// gemessene Zeit: 9.4.3 rechnet in abstrakten Operationen, und die
/// Umrechnung in Zeit ist die Kalibrierung aus M5.
pub fn cost_of(f: Native) -> Cost {
    match f {
        // Acht Runden je Byte, je Runde ein Test, ein Schieben und ein
        // Xor; dazu das Xor des Bytes.
        Native::Crc32 | Native::Crc32c => Cost { per_byte: 25, call: 2, stack: 16 },
        Native::Crc16 => Cost { per_byte: 25, call: 2, stack: 16 },
        // Eine Addition je Byte.
        Native::Sum8 => Cost { per_byte: 1, call: 1, stack: 8 },
    }
}
