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
    /// Maximaler Stack-Bedarf in Byte (4.5, geht in 12.3 ein): der am
    /// Einstieg aus `takt-native-abi` gemessene groesste Bedarf beider Boards
    /// (13.8, FB-293) plus ein Viertel, auf 32 Byte aufgerundet. Die Sema
    /// lehnt eine Deklaration darunter ab.
    pub stack: u32,
}

/// Der Kostenvertrag einer Funktion.
///
/// Die Operationen sind abgezaehlt, nicht gemessene Zeit: 9.4.3 rechnet in
/// abstrakten Operationen, und die Umrechnung in Zeit ist die Kalibrierung
/// aus M5. Der Stack ist gemessen; im Kommentar das Maximum (F401, C6 am
/// 2026-09-28).
pub fn cost_of(f: Native) -> Cost {
    match f {
        // Acht Runden je Byte, je Runde ein Test, ein Schieben und ein
        // Xor; dazu das Xor des Bytes. Stack 16.
        Native::Crc32 | Native::Crc32c => Cost { per_byte: 25, call: 2, stack: 32 },
        Native::Crc16 => Cost { per_byte: 25, call: 2, stack: 32 },
        // Eine Addition je Byte. Stack 8.
        Native::Sum8 => Cost { per_byte: 1, call: 1, stack: 32 },
        // Je 64-Byte-Block 64 Runden zu rund 20 Operationen und 48
        // Schedule-Schritte zu rund 10, dazu die Byteschleife: 32 je Byte.
        // Das Finale fuellt bis zu zwei Bloecke. Stack 504: der Zustand
        // (112), der Ring des Plans (64), die Runden.
        Native::Sha256 => Cost { per_byte: 32, call: 3600, stack: 640 },
        // Zwei Hashes: innen 64 Byte Pad plus Nachricht, aussen 64 plus 32.
        // Stack 792.
        Native::HmacSha256 => Cost { per_byte: 32, call: 7200, stack: 992 },
        // Stack 236: der Zustand und seine kanonische Form.
        Native::Sha256Init => Cost { per_byte: 0, call: 8, stack: 320 },
        // Dazu das Lesen und Schreiben der kanonischen Form (108 Byte).
        // Stack 412.
        Native::Sha256Update => Cost { per_byte: 32, call: 240, stack: 544 },
        // Stack 488.
        Native::Sha256Final => Cost { per_byte: 0, call: 3600, stack: 640 },
        // Der Start eines Jobs (4.5): Argumente kopieren; die Pruefung
        // selbst laeuft ausserhalb der Schrittphase in `takt-crypto`. Der
        // Stack ist der des Job-Kontexts. Stack 4480 (`p256`).
        Native::EcdsaP256Verify => Cost { per_byte: 0, call: 300, stack: 5600 },
        // Stack 8348: Montgomery-Reduktion ueber 3072 Bit mit doppelt
        // breiten Produkten (`crypto-bigint`), dazu MGF1 und SHA-256.
        Native::Rsa3072Verify => Cost { per_byte: 0, call: 900, stack: 10_464 },
        // Stack 2188.
        Native::AesGcmDecrypt => Cost { per_byte: 1, call: 300, stack: 2752 },
        // 128-Punkt-FFT: 7 Stufen zu 64 Schmetterlingen mit 4 Produkten und
        // 6 Summen, die Trennung je Bin 14 Operationen, dazu Laden und
        // Speichern. Stack 4344: Real- und Imaginaerteil in `f64` (2 KiB)
        // und der Puffer der kanonischen Form am Einstieg.
        Native::Fft256 => Cost { per_byte: 0, call: 8_400, stack: 5440 },
    }
}
