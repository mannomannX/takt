//! Pruefsummen (4.5, 11.4).
//!
//! **Bitweise gerechnet, nicht ueber eine Tabelle.** Eine Tabelle waere
//! schneller und kostete 1 KiB Flash je Polynom — auf einer MCU ist das
//! viel (12.3, `takt size` weist es aus). Vor allem aber ist die bitweise
//! Form die, die *offensichtlich* der Norm entspricht: Sie ist die
//! Definition, eine Tabelle ist ihre Optimierung. Fuer die TCB (9.5)
//! zaehlt das mehr als der Takt.
//!
//! LLVM faltet die innere Schleife ohnehin, und wo es auf Geschwindigkeit
//! ankommt, deklariert ein Projekt seine eigene Tabelle als
//! Projekt-Native (4.5, v1.1).

/// CRC-32 nach IEEE 802.3 (ZIP, PNG, Ethernet).
///
/// Polynom 0xEDB88320 (reflektiert), Startwert 0xFFFFFFFF, Ergebnis
/// invertiert.
pub fn crc32(bytes: &[u8]) -> u32 {
    reflected(bytes, 0xEDB8_8320, 0xFFFF_FFFF) ^ 0xFFFF_FFFF
}

/// CRC-32C nach Castagnoli (iSCSI, SCTP, ext4).
///
/// Dasselbe Verfahren mit dem Polynom 0x82F63B78; es hat bessere
/// Fehlererkennung bei kurzen Nachrichten.
pub fn crc32c(bytes: &[u8]) -> u32 {
    reflected(bytes, 0x82F6_3B78, 0xFFFF_FFFF) ^ 0xFFFF_FFFF
}

/// CRC-32 ueber mehrere Abschnitte.
///
/// Wer eine Nachricht aus Teilen zusammensetzt — etwa einen Kopf und seine
/// Nutzlast —, rechnet nicht zwei CRCs, sondern fuehrt den Zustand fort.
/// `crc32_start()` liefert den Anfang, `crc32_final` das Ergebnis; dazwischen
/// beliebig viele `crc32_update`.
pub fn crc32_start() -> u32 {
    0xFFFF_FFFF
}

/// Schreibt Bytes in einen laufenden CRC-32 fort.
pub fn crc32_update(state: u32, bytes: &[u8]) -> u32 {
    step(state, bytes, 0xEDB8_8320)
}

/// Schliesst einen laufenden CRC-32 ab.
pub fn crc32_final(state: u32) -> u32 {
    state ^ 0xFFFF_FFFF
}

/// Der gemeinsame Kern der beiden CRC-32.
fn reflected(bytes: &[u8], poly: u32, init: u32) -> u32 {
    step(init, bytes, poly)
}

/// Eine Runde ueber `bytes` ab `crc`.
fn step(mut crc: u32, bytes: &[u8], poly: u32) -> u32 {
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            // Ohne Verzweigung waere es eine Maske; mit ist es die Form
            // der Norm. Der Compiler macht daraus dasselbe.
            crc = if crc & 1 != 0 { (crc >> 1) ^ poly } else { crc >> 1 };
        }
    }
    crc
}

/// CRC-16/IBM, auch CRC-16/ARC (Modbus RTU).
///
/// Polynom 0xA001 (reflektiert), Startwert 0; ohne Invertierung.
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for b in bytes {
        crc ^= u16::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// Die Summe der Bytes, modulo 256.
///
/// Die einfachste Pruefsumme, und in vielen Feldprotokollen die einzige.
/// `wrapping_add` ist hier kein Ueberlauf, sondern die Definition.
pub fn sum8(bytes: &[u8]) -> u8 {
    let mut s: u8 = 0;
    for b in bytes {
        s = s.wrapping_add(*b);
    }
    s
}
