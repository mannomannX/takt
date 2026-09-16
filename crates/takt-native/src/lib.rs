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
}

/// Parameter und Ergebnis, wie das Programm sie deklarieren muss
/// (Pruefung 31).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Die Parameter in ihrer Reihenfolge.
    pub params: &'static [Kind],
    /// Das Ergebnis.
    pub ret: Kind,
}

/// Das Ergebnis eines Aufrufs ueber Byteblocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Eine Pruefsumme; die Breite steht in der Signatur.
    Scalar(u64),
    /// Ein Digest.
    Digest([u8; 32]),
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
        }
    }

    /// Die Implementierung liegt ausserhalb dieses Crates (`takt-crypto`):
    /// `call` liefert `None`, die Vektoren stehen im Block `takt-crypto`.
    pub fn external(self) -> bool {
        matches!(self, Native::EcdsaP256Verify)
    }

    /// Alle Funktionen der Menge.
    pub const ALL: [Native; 10] = [
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
    ];

    /// Die Funktion zu einem Namen.
    pub fn by_name(name: &str) -> Option<Native> {
        Native::ALL.into_iter().find(|n| n.name() == name)
    }

    /// Die Signatur, gegen die Pruefung 31 die Deklaration haelt.
    pub fn signature(self) -> Signature {
        match self {
            Native::Crc32 | Native::Crc32c => Signature { params: &[Kind::Bytes], ret: Kind::U32 },
            Native::Crc16 => Signature { params: &[Kind::Bytes], ret: Kind::U16 },
            Native::Sum8 => Signature { params: &[Kind::Bytes], ret: Kind::U8 },
            Native::Sha256 => Signature { params: &[Kind::Bytes], ret: Kind::Digest },
            Native::HmacSha256 => Signature { params: &[Kind::Bytes, Kind::Bytes], ret: Kind::Digest },
            Native::Sha256Init => Signature { params: &[], ret: Kind::Sha256Ctx },
            Native::Sha256Update => Signature { params: &[Kind::Sha256Ctx, Kind::Bytes], ret: Kind::Sha256Ctx },
            Native::Sha256Final => Signature { params: &[Kind::Sha256Ctx], ret: Kind::Digest },
            Native::EcdsaP256Verify => Signature { params: &[Kind::Bytes, Kind::Digest, Kind::Bytes], ret: Kind::Bool },
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
/// Pruefsummen und `sha256`, Schluessel und Nachricht fuer `hmac_sha256`,
/// die Chunks in ihrer Reihenfolge fuer `sha256_init/update/final`.
/// `None`, wenn die Zahl der Bloecke nicht zur Funktion passt.
pub fn call(f: Native, inputs: &[&[u8]]) -> Option<Output> {
    let one = || inputs.first().copied().filter(|_| inputs.len() == 1);
    Some(match f {
        Native::EcdsaP256Verify => return None,
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
