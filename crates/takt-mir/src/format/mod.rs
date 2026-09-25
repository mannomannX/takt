//! Dateiformat `TAKT-MIR` (Referenz 11.3; Spezifikation grammar/mir-format.md).
//!
//! Kopf `{magic, format_version, edition, compiler_version}`, Stringtabelle,
//! Rumpf als ein Knoten `Program`. Knoten sind Folgen nummerierter Felder
//! (Nummer, Drahtart, Wert); Leser ueberspringen unbekannte Nummern, Schreiber
//! schreiben die neueste Version. Kein `unsafe`, keine Abhaengigkeit.

pub mod codec;
pub mod schema;
pub mod wire;

pub use wire::{FormatError, Raw, Reader, Wire, Writer};

use codec::{Field, read_one};
use wire::{Node, get_varint, put_varint, slice};

use crate::program::Program;

/// Kennung am Dateianfang.
pub const MAGIC: &[u8; 8] = b"TAKT-MIR";

/// Formatversion dieses Schreibers; Leser akzeptieren alle Versionen bis hier.
pub const FORMAT_VERSION: u16 = 9;

/// Kopf einer Datei.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Formatversion der Datei.
    pub format_version: u16,
    /// Edition des Programms (2.5).
    pub edition: u32,
    /// Compiler-Version des Schreibers.
    pub compiler_version: String,
}

/// Serialisiert den Rumpf: Stringtabelle und Bytes des Knotens `Program`
/// (Feld 1 der Wurzel).
pub fn encode_body(p: &Program, logic_only: bool) -> (Vec<String>, Vec<u8>) {
    let mut w = Writer::new(logic_only);
    p.write(&mut w, 1);
    w.finish()
}

/// Schreibt eine Datei.
pub fn write_program(p: &Program, compiler_version: &str) -> Vec<u8> {
    let (strings, body) = encode_body(p, false);
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&p.config.edition.to_le_bytes());
    put_str(&mut out, compiler_version);
    put_varint(&mut out, strings.len() as u64);
    for s in &strings {
        put_str(&mut out, s);
    }
    put_varint(&mut out, body.len() as u64);
    out.extend_from_slice(&body);
    out
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    put_varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn get_str(buf: &[u8], pos: &mut usize) -> Result<String, FormatError> {
    let len = get_varint(buf, pos)?;
    let bytes = slice(buf, *pos, len)?;
    *pos += bytes.len();
    String::from_utf8(bytes.to_vec()).map_err(|_| FormatError::BadUtf8)
}

/// Liest nur den Kopf.
pub fn read_header(buf: &[u8]) -> Result<(Header, usize), FormatError> {
    if buf.len() < 14 || &buf[..8] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let format_version = u16::from_le_bytes([buf[8], buf[9]]);
    if format_version > FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion(u32::from(format_version)));
    }
    let edition = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]);
    let mut pos = 14;
    let compiler_version = get_str(buf, &mut pos)?;
    Ok((Header { format_version, edition, compiler_version }, pos))
}

/// Liest einen Rumpf mit seiner Stringtabelle (Umkehrung von `encode_body`).
pub fn decode_body(strings: Vec<String>, body: &[u8]) -> Result<Program, FormatError> {
    let r = Reader::new(strings);
    let root = Node::parse("Wurzel", body)?;
    read_one::<Program>(&root, &r, 1)
}

/// Liest eine Datei.
pub fn read_program(buf: &[u8]) -> Result<(Header, Program), FormatError> {
    let (header, mut pos) = read_header(buf)?;
    let count = get_varint(buf, &mut pos)?;
    let mut strings = Vec::new();
    for _ in 0..count {
        strings.push(get_str(buf, &mut pos)?);
    }
    let len = get_varint(buf, &mut pos)?;
    let body = slice(buf, pos, len)?;
    Ok((header, decode_body(strings, body)?))
}
