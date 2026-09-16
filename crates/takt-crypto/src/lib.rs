//! Kryptographie mit gepruefter Abhaengigkeit (Referenz 4.5, 9.5;
//! plan/m6.md 2.4).
//!
//! `takt-native` hat keine Abhaengigkeit, weil jede zur TCB gehoerte.
//! ECDSA P-256 ist Konstantzeit-Bignum und Kurvenarithmetik — eine
//! Spezialitaet, die man nicht nachbaut. Sie kommt aus `p256`
//! (RustCrypto), hinter dem Feature `ecdsa`, und das TCB-Manifest des
//! Lauf-Headers nennt sie beim Namen (12.5).

#![no_std]

/// Die Abhaengigkeit fehlt: ohne das Feature `ecdsa` gebaut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unavailable;

/// Was das TCB-Manifest ueber dieses Crate sagt (12.5).
pub const MANIFEST: &str = if cfg!(feature = "ecdsa") {
    "takt-crypto ecdsa_p256_verify: p256 0.13 (RustCrypto)"
} else {
    "takt-crypto ohne Feature ecdsa"
};

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
