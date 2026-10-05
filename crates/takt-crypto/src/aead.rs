//! `aes_gcm_decrypt` (4.5): AES-GCM nach NIST SP 800-38D mit 96-Bit-Nonce
//! und 128-Bit-Tag; AES-128 oder AES-256 nach der Laenge des Schluessels.
//!
//! Die Implementierung kommt aus `aes-gcm` (RustCrypto): die Bitslice-AES
//! aus `aes`, in konstanter Zeit, ohne Allokation.

use aes_gcm::aead::AeadInPlace;
use aes_gcm::aead::consts::U12;
use aes_gcm::{Aes128Gcm, Aes256Gcm, KeyInit, Nonce, Tag};

/// Entschluesselt `data` nach `out` und liefert die Laenge des
/// Klartexts. `None`, wenn der Tag nicht stimmt, wenn der Schluessel weder
/// 16 noch 32 Byte hat, die Nonce nicht 12 und der Tag nicht 16 Byte oder
/// `out` den Klartext nicht fasst — nie ein Panic (13.8). Vor dem
/// Vergleich des Tags steht in `out` nichts, was der Aufrufer sehen soll:
/// Ein falscher Tag hinterlaesst Nullen.
pub fn decrypt(key: &[u8], nonce: &[u8], aad: &[u8], data: &[u8], tag: &[u8], out: &mut [u8]) -> Option<usize> {
    let nonce = Nonce::<U12>::from(<[u8; 12]>::try_from(nonce).ok()?);
    let tag = Tag::from(<[u8; 16]>::try_from(tag).ok()?);
    // Erst der Schluessel, dann die Kopie: Ein Abbruch danach liesse das
    // Chiffrat in `out` stehen (INT-032).
    if key.len() != 16 && key.len() != 32 {
        return None;
    }
    let plain = out.get_mut(..data.len())?;
    plain.copy_from_slice(data);
    let opened = if key.len() == 16 {
        Aes128Gcm::new_from_slice(key).map(|c| c.decrypt_in_place_detached(&nonce, aad, plain, &tag))
    } else {
        Aes256Gcm::new_from_slice(key).map(|c| c.decrypt_in_place_detached(&nonce, aad, plain, &tag))
    };
    let opened = opened.unwrap_or(Err(aes_gcm::Error));
    if opened.is_err() {
        plain.fill(0);
        return None;
    }
    Some(data.len())
}
