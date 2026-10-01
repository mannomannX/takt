//! `rsa3072_verify` (4.5): RSASSA-PSS nach RFC 8017 8.1.2 mit SHA-256,
//! MGF1 ueber SHA-256, Salt von 32 Byte und dem oeffentlichen Exponenten
//! 65537 — die Form, in der MCUboot Images mit RSA-3072 signiert.
//!
//! Die Arithmetik kommt aus `crypto-bigint` (RustCrypto): Montgomery-
//! Reduktion und breite Produkte fester Breite, ohne Allokation. Der Hash
//! ist der aus `takt-native`, der ohnehin zur TCB gehoert.
//!
//! **Warum nicht `DynResidue`.** Jeder Wert dort traegt seine Parameter mit
//! (Modulus, R, R², R³, je 384 Byte), und die Potenz haelt mehrere davon:
//! Auf den Boards waren das ueber 12 KiB Stack. Hier stehen dieselben
//! Bausteine — `montgomery_reduction`, `mul_wide`, `square_wide` — mit
//! einem Modulus und drei Zahlen.

use crypto_bigint::modular::montgomery_reduction;
use crypto_bigint::{Encoding, Limb, U3072, Uint, Word};
use takt_native::sha256;

/// Laenge von Modulus und Signatur in Byte.
const K: usize = 384;
/// Laenge des Digests und des Salts.
const H_LEN: usize = 32;
/// Laenge von `maskedDB`: `emLen - hLen - 1`.
const DB_LEN: usize = K - H_LEN - 1;

/// Prueft eine Signatur. `false` fuer jeden Schluessel, der kein Modulus
/// von genau 3072 Bit ist, fuer jede Signatur ab dem Modulus und fuer
/// jede, deren Kodierung nicht stimmt — nie ein Panic (13.8).
pub fn verify(key: &[u8], digest: &[u8], sig: &[u8]) -> bool {
    if key.len() != K || digest.len() != H_LEN || sig.len() != K {
        return false;
    }
    // 3072 Bit heisst: das oberste Bit gesetzt; ungerade, sonst gibt es
    // keine Montgomery-Form.
    if key[0] & 0x80 == 0 || key[K - 1] & 1 == 0 {
        return false;
    }
    let n = U3072::from_be_slice(key);
    let s = U3072::from_be_slice(sig);
    if s >= n {
        return false;
    }
    emsa_pss_verify(digest, &public_power(&s, &n).to_be_bytes())
}

/// `s^65537 mod n` fuer ein ungerades `n` mit gesetztem oberstem Bit:
/// sechzehn Quadrate und ein Produkt in Montgomery-Form. Die Parameter
/// wie `DynResidueParams::new`, `R mod n`, `R² mod n` und `-n⁻¹ mod 2^w`,
/// aber ohne Division: Mit `n < R = 2^3072 < 2n` ist `R mod n = R - n`,
/// und `R² mod n` entsteht aus ihm durch 3072 Verdopplungen modulo `n`.
/// Die Division haelt doppelt breite Kopien auf dem Stack.
fn public_power(s: &U3072, n: &U3072) -> U3072 {
    let mut r2 = U3072::ZERO.wrapping_sub(n);
    for _ in 0..3072 {
        r2 = r2.add_mod(&r2, n);
    }
    let low = Uint::<1>::from_words([n.as_words()[0]]);
    let inv = Limb(Word::MIN.wrapping_sub(low.inv_mod2k_vartime(Word::BITS as usize).as_words()[0]));
    let base = montgomery_reduction(&s.mul_wide(&r2), n, inv);
    let mut m = base;
    for _ in 0..16 {
        m = montgomery_reduction(&m.square_wide(), n, inv);
    }
    let m = montgomery_reduction(&m.mul_wide(&base), n, inv);
    montgomery_reduction(&(m, U3072::ZERO), n, inv)
}

/// EMSA-PSS-VERIFY (RFC 8017 9.1.2) fuer `emBits = 3071`.
fn emsa_pss_verify(m_hash: &[u8], em: &[u8; K]) -> bool {
    if em[K - 1] != 0xbc {
        return false;
    }
    let (masked_db, h) = (&em[..DB_LEN], &em[DB_LEN..K - 1]);
    // `8 * emLen - emBits` = 1: Das oberste Bit muss null sein.
    if masked_db[0] & 0x80 != 0 {
        return false;
    }
    let mut db = [0u8; DB_LEN];
    mgf1(h, &mut db);
    for (d, m) in db.iter_mut().zip(masked_db) {
        *d ^= m;
    }
    db[0] &= 0x7f;
    let pad = DB_LEN - H_LEN - 1;
    if db[..pad].iter().any(|b| *b != 0) || db[pad] != 0x01 {
        return false;
    }
    let mut m_prime = sha256::Ctx::new();
    m_prime.update(&[0u8; 8]);
    m_prime.update(m_hash);
    m_prime.update(&db[pad + 1..]);
    m_prime.finish() == *h
}

/// MGF1 ueber SHA-256 (RFC 8017 B.2.1), `out.len()` Byte.
fn mgf1(seed: &[u8], out: &mut [u8]) {
    for (counter, chunk) in (0u32..).zip(out.chunks_mut(H_LEN)) {
        let mut ctx = sha256::Ctx::new();
        ctx.update(seed);
        ctx.update(&counter.to_be_bytes());
        let block = ctx.finish();
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
}
