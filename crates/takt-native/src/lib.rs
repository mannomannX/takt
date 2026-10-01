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
//! **Was hier steht und was nicht.** Pruefsummen, Hashes und `fft256`
//! rechnet dieses Crate selbst. Die Kryptographie, die eine gepruefte
//! Abhaengigkeit braucht (`ecdsa_p256_verify`, `rsa3072_verify`,
//! `aes_gcm_decrypt`), steht in `takt-crypto`; hier stehen nur Namen und
//! Signaturen ([`Native::external`]).

#![no_std]

pub mod bytes;
pub mod cost;
pub mod crc;
pub mod fft;
pub mod fft_table;
pub mod map;
pub mod sha256;

pub use cost::{Cost, cost_of};

/// Eine Funktion der kuratierten Menge (4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Native {
    /// `crc32(b)`: CRC-32 nach IEEE 802.3, wie in ZIP und PNG.
    Crc32,
    /// `crc32c(b)`: CRC-32 nach Castagnoli (iSCSI, SCTP).
    Crc32c,
    /// `crc16(b)`: CRC-16/IBM, wie in Modbus RTU.
    Crc16,
    /// `sum8(b)`: die Summe der Bytes, modulo 256.
    Sum8,
    /// `sha256(b)`: SHA-256 nach FIPS 180-4.
    Sha256,
    /// `hmac_sha256(key, msg)`: HMAC ueber SHA-256 (RFC 2104).
    HmacSha256,
    /// `sha256_init() -> Sha256Ctx`: der leere Zustand.
    Sha256Init,
    /// `sha256_update(ctx, chunk) -> Sha256Ctx`: ein Chunk mehr.
    Sha256Update,
    /// `sha256_final(ctx) -> bytes<32>`: der Digest.
    Sha256Final,
    /// `ecdsa_p256_verify(key, digest, sig) -> bool`: ein Job (4.5); die
    /// Implementierung liegt in `takt-crypto`, dieses Crate kennt nur
    /// Namen und Signatur.
    EcdsaP256Verify,
    /// `fft256(x: [256] float) -> [256] float`: das Spektrum einer reellen
    /// Folge ([`fft`]).
    Fft256,
    /// `rsa3072_verify(key, digest, sig) -> bool`: RSASSA-PSS mit SHA-256
    /// und dem Exponenten 65537, ein Job in `takt-crypto`.
    Rsa3072Verify,
    /// `aes_gcm_decrypt(key, nonce, aad, data, tag) -> bytes<N>`: AES-GCM,
    /// ein Job in `takt-crypto`; ein falscher Tag beendet ihn mit
    /// `Err(FAILED)`.
    AesGcmDecrypt,
}

/// Die Art eines Arguments oder Ergebnisses an der Grenze (4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `bytes<N>` beliebiger Kapazitaet, als Zeiger und Laenge.
    Bytes,
    /// `u8`.
    U8,
    /// `u16`.
    U16,
    /// `u32`.
    U32,
    /// `bytes<32>`, ein Digest.
    Digest,
    /// Das Prelude-Record `Sha256Ctx` in kanonischer Byteform.
    Sha256Ctx,
    /// `bool`, ein Byte.
    Bool,
    /// `bytes<N>` mit genau dieser Kapazitaet (Nonce und Tag von AES-GCM).
    Fixed(u32),
    /// `[256] float` in der Breite des Programms (4.2), in kanonischer Form
    /// ohne Laenge: 1024 Byte fuer `f32`, 2048 fuer `f64`.
    Floats256,
}

/// Parameter und Ergebnis, wie das Programm sie deklarieren muss
/// (Pruefung 31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Die Parameter in ihrer Reihenfolge.
    pub params: &'static [Kind],
    /// Das Ergebnis.
    pub ret: Kind,
    /// Der Parameter, dessen Inhalt das Ergebnis fassen muss: Seine
    /// Kapazitaet ist hoechstens die des Ergebnisses (Pruefung 31).
    pub holds: Option<usize>,
}

/// Das Ergebnis eines Aufrufs ueber Byteblocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant, reason = "ohne Allokation (no_std, TCB) gibt es keine Box; der Wert lebt kurz")]
pub enum Output {
    /// Eine Pruefsumme; die Breite steht in der Signatur.
    Scalar(u64),
    /// Ein Digest.
    Digest([u8; 32]),
    /// Die 256 Werte von `fft256` in kanonischer Form: `len` Byte von
    /// `bytes` (1024 fuer `f32`, 2048 fuer `f64`).
    Floats {
        /// Der Puffer.
        bytes: [u8; fft::BYTES_F64],
        /// Wie viele Byte gelten.
        len: usize,
    },
}

impl Output {
    /// Das Ergebnis, wie die Vektoren es schreiben: eine Pruefsumme in
    /// `width` Hexziffern, ein Digest Byte fuer Byte, die Werte von
    /// `fft256` als SHA-256 ihrer kanonischen Form (grammar/takt-native.md).
    pub fn digest_or_scalar(&self) -> Result<u64, [u8; 32]> {
        match self {
            Output::Scalar(v) => Ok(*v),
            Output::Digest(d) => Err(*d),
            Output::Floats { bytes, len } => Err(sha256::sha256(&bytes[..*len])),
        }
    }
}

impl Native {
    /// Der Name, unter dem das Programm sie ruft.
    pub fn name(self) -> &'static str {
        match self {
            Native::Crc32 => "crc32",
            Native::Crc32c => "crc32c",
            Native::Crc16 => "crc16",
            Native::Sum8 => "sum8",
            Native::Sha256 => "sha256",
            Native::HmacSha256 => "hmac_sha256",
            Native::Sha256Init => "sha256_init",
            Native::Sha256Update => "sha256_update",
            Native::Sha256Final => "sha256_final",
            Native::EcdsaP256Verify => "ecdsa_p256_verify",
            Native::Fft256 => "fft256",
            Native::Rsa3072Verify => "rsa3072_verify",
            Native::AesGcmDecrypt => "aes_gcm_decrypt",
        }
    }

    /// Die Implementierung liegt ausserhalb dieses Crates (`takt-crypto`):
    /// `call` liefert `None`, die Vektoren stehen im Block `takt-crypto`.
    pub fn external(self) -> bool {
        matches!(self, Native::EcdsaP256Verify | Native::Rsa3072Verify | Native::AesGcmDecrypt)
    }

    /// Nur als `native job` (4.5): Die Rechnung passt in keinen Tick, und
    /// ein `fn` fuehrte sie im Schritt aus — mit einem Kostenvertrag, der
    /// nur den Start zaehlt.
    pub fn job_only(self) -> bool {
        self.external()
    }

    /// Alle Funktionen der Menge.
    pub const ALL: [Native; 13] = [
        Native::Crc32,
        Native::Crc32c,
        Native::Crc16,
        Native::Sum8,
        Native::Sha256,
        Native::HmacSha256,
        Native::Sha256Init,
        Native::Sha256Update,
        Native::Sha256Final,
        Native::EcdsaP256Verify,
        Native::Fft256,
        Native::Rsa3072Verify,
        Native::AesGcmDecrypt,
    ];

    /// Die Funktion zu einem Namen.
    pub fn by_name(name: &str) -> Option<Native> {
        Native::ALL.into_iter().find(|n| n.name() == name)
    }

    /// Die Signatur, gegen die Pruefung 31 die Deklaration haelt.
    pub fn signature(self) -> Signature {
        let plain = |params, ret| Signature { params, ret, holds: None };
        match self {
            Native::Crc32 | Native::Crc32c => plain(&[Kind::Bytes], Kind::U32),
            Native::Crc16 => plain(&[Kind::Bytes], Kind::U16),
            Native::Sum8 => plain(&[Kind::Bytes], Kind::U8),
            Native::Sha256 => plain(&[Kind::Bytes], Kind::Digest),
            Native::HmacSha256 => plain(&[Kind::Bytes, Kind::Bytes], Kind::Digest),
            Native::Sha256Init => plain(&[], Kind::Sha256Ctx),
            Native::Sha256Update => plain(&[Kind::Sha256Ctx, Kind::Bytes], Kind::Sha256Ctx),
            Native::Sha256Final => plain(&[Kind::Sha256Ctx], Kind::Digest),
            Native::EcdsaP256Verify | Native::Rsa3072Verify => {
                plain(&[Kind::Bytes, Kind::Digest, Kind::Bytes], Kind::Bool)
            }
            Native::Fft256 => plain(&[Kind::Floats256], Kind::Floats256),
            Native::AesGcmDecrypt => Signature {
                params: &[Kind::Bytes, Kind::Fixed(12), Kind::Bytes, Kind::Bytes, Kind::Fixed(16)],
                ret: Kind::Bytes,
                holds: Some(3),
            },
        }
    }

    /// Unter welchem Namen die Vektoren in `grammar/takt-native.md`
    /// stehen: Die Chunk-Natives teilen sich die Zeilen `sha256_update`,
    /// deren Eingaben die Chunks sind.
    pub fn vector_name(self) -> &'static str {
        match self {
            Native::Sha256Init | Native::Sha256Final => "sha256_update",
            other => other.name(),
        }
    }
}

/// Rechnet eine Funktion der Menge ueber Byteblocks: ein Block fuer die
/// Pruefsummen, `sha256` und `fft256`, Schluessel und Nachricht fuer
/// `hmac_sha256`, die Chunks in ihrer Reihenfolge fuer
/// `sha256_init/update/final`. `None`, wenn die Bloecke nicht zur Funktion
/// passen, und fuer die Funktionen aus `takt-crypto`.
pub fn call(f: Native, inputs: &[&[u8]]) -> Option<Output> {
    let one = || inputs.first().copied().filter(|_| inputs.len() == 1);
    Some(match f {
        Native::EcdsaP256Verify | Native::Rsa3072Verify | Native::AesGcmDecrypt => return None,
        Native::Fft256 => {
            let mut bytes = [0u8; fft::BYTES_F64];
            let len = fft::fft256(one()?, &mut bytes)?;
            Output::Floats { bytes, len }
        }
        Native::Crc32 => Output::Scalar(u64::from(crc::crc32(one()?))),
        Native::Crc32c => Output::Scalar(u64::from(crc::crc32c(one()?))),
        Native::Crc16 => Output::Scalar(u64::from(crc::crc16(one()?))),
        Native::Sum8 => Output::Scalar(u64::from(crc::sum8(one()?))),
        Native::Sha256 => Output::Digest(sha256::sha256(one()?)),
        Native::HmacSha256 => match inputs {
            [key, msg] => Output::Digest(sha256::hmac_sha256(key, msg)),
            _ => return None,
        },
        Native::Sha256Init | Native::Sha256Update | Native::Sha256Final => {
            let mut ctx = sha256::Ctx::new();
            for chunk in inputs {
                ctx.update(chunk);
            }
            Output::Digest(ctx.finish())
        }
    })
}
