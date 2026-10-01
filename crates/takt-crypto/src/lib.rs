//! Kryptographie mit gepruefter Abhaengigkeit (Referenz 4.5, 9.5;
//! plan/m6.md 2.4).
//!
//! `takt-native` hat keine Abhaengigkeit, weil jede zur TCB gehoerte.
//! Kurvenarithmetik, Bignum in konstanter Zeit und eine Blockchiffre sind
//! Spezialitaeten, die man nicht nachbaut: Sie kommen aus RustCrypto, jede
//! hinter einem eigenen Feature (`ecdsa`, `rsa`, `aes-gcm`), und das
//! TCB-Manifest des Lauf-Headers nennt sie beim Namen (12.5).

#![no_std]

#[cfg(feature = "aes-gcm")]
mod aead;
#[cfg(feature = "rsa")]
mod rsa;

/// Die Abhaengigkeit fehlt: ohne ihr Feature gebaut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unavailable;

/// Was das TCB-Manifest ueber eine Funktion dieses Crates sagt (12.5):
/// ihre Abhaengigkeit, oder `None`, wenn das Feature fehlt oder die
/// Funktion nicht hierher gehoert.
pub fn manifest(native: &str) -> Option<&'static str> {
    match native {
        "ecdsa_p256_verify" if cfg!(feature = "ecdsa") => Some("takt-crypto ecdsa_p256_verify: p256 0.13 (RustCrypto)"),
        "rsa3072_verify" if cfg!(feature = "rsa") => {
            Some("takt-crypto rsa3072_verify: crypto-bigint 0.5 (RustCrypto), SHA-256 aus takt-native")
        }
        "aes_gcm_decrypt" if cfg!(feature = "aes-gcm") => {
            Some("takt-crypto aes_gcm_decrypt: aes-gcm 0.10 (RustCrypto)")
        }
        _ => None,
    }
}

/// `ecdsa_p256_verify(key, digest, sig)` (4.5): der Schluessel als X ‖ Y
/// (64 Byte, unkomprimiert ohne Praefix), der Digest 32 Byte, die
/// Signatur als r ‖ s (64 Byte). `false` fuer jeden ungueltigen
/// Schluessel und jede ungueltige Signatur — nie ein Panic (13.8).
pub fn ecdsa_p256_verify(key: &[u8; 64], digest: &[u8; 32], sig: &[u8; 64]) -> Result<bool, Unavailable> {
    #[cfg(feature = "ecdsa")]
    {
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        use p256::ecdsa::{Signature, VerifyingKey};
        let mut encoded = [0u8; 65];
        encoded[0] = 4;
        encoded[1..].copy_from_slice(key);
        let Ok(point) = p256::EncodedPoint::from_bytes(encoded) else { return Ok(false) };
        let Ok(verifying) = VerifyingKey::from_encoded_point(&point) else { return Ok(false) };
        let Ok(signature) = Signature::from_slice(sig) else { return Ok(false) };
        Ok(verifying.verify_prehash(digest, &signature).is_ok())
    }
    #[cfg(not(feature = "ecdsa"))]
    {
        let _ = (key, digest, sig);
        Err(Unavailable)
    }
}

/// `rsa3072_verify(key, digest, sig)` (4.5): RSASSA-PSS mit SHA-256, MGF1,
/// Salt 32 Byte und dem Exponenten 65537; der Schluessel ist der Modulus
/// (384 Byte, big endian). `false` fuer jeden ungueltigen Schluessel und
/// jede ungueltige Signatur — nie ein Panic (13.8).
pub fn rsa3072_verify(key: &[u8], digest: &[u8], sig: &[u8]) -> Result<bool, Unavailable> {
    #[cfg(feature = "rsa")]
    {
        Ok(rsa::verify(key, digest, sig))
    }
    #[cfg(not(feature = "rsa"))]
    {
        let _ = (key, digest, sig);
        Err(Unavailable)
    }
}

/// `aes_gcm_decrypt(key, nonce, aad, data, tag)` (4.5): der Klartext nach
/// `out`, seine Laenge, oder `None`, wenn der Tag nicht stimmt oder eine
/// Laenge nicht passt — nie ein Panic (13.8).
pub fn aes_gcm_decrypt(
    key: &[u8],
    nonce: &[u8],
    aad: &[u8],
    data: &[u8],
    tag: &[u8],
    out: &mut [u8],
) -> Result<Option<usize>, Unavailable> {
    #[cfg(feature = "aes-gcm")]
    {
        Ok(aead::decrypt(key, nonce, aad, data, tag, out))
    }
    #[cfg(not(feature = "aes-gcm"))]
    {
        let _ = (key, nonce, aad, data, tag, out);
        Err(Unavailable)
    }
}
