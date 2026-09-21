//! `natives.review`: wer ein Projekt-Native geprueft hat (4.5, v1.2).
//!
//! **Warum eine Datei und keine Signatur.** Ein Projekt-Native erweitert
//! die TCB; 4.5 verlangt dafuer einen Prozess, nicht eine Technik. Die
//! Datei liegt im Repository, und ihre Historie ist das Audit: Wer eine
//! Zeile hinzufuegt, tut es in einem Commit mit Namen und Datum. Ein
//! Schluesselpaar brauchte eine Verwaltung, die niemand betreibt, und
//! bewiese nichts, was `git log` nicht auch zeigt.
//!
//! Der Schluessel ist der Hash des Objekts, nicht sein Name: Eine
//! geaenderte Implementierung ist ein anderes Native, auch wenn sie
//! gleich heisst. Genau das soll die Pruefung bemerken.
//!
//! ```text
//! # natives.review — je Zeile: Native, SHA-256 der Quelle, Pruefer, Datum
//! crc_custom  7b3a…f1  anna.beispiel  2026-09-22
//! ```

use std::collections::BTreeMap;

use crate::hash::{Hash256, sha256};

/// Eine gepruefte Implementierung.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Name des Natives, wie das Programm ihn nennt.
    pub native: String,
    /// SHA-256 der Quelldatei, hexadezimal.
    pub hash: String,
    /// Wer geprueft hat.
    pub by: String,
    /// Wann, als `JJJJ-MM-TT`.
    pub date: String,
}

/// Die Datei, nach Native und Hash.
#[derive(Clone, Debug, Default)]
pub struct Review {
    entries: BTreeMap<(String, String), Entry>,
}

/// Was beim Lesen schiefgehen kann.
#[derive(Debug)]
pub struct ParseError {
    /// Zeilennummer, ab eins.
    pub line: u32,
    /// Was fehlt.
    pub message: String,
}

impl Review {
    /// Die Zeile zu einem Native mit diesem Hash.
    pub fn entry(&self, native: &str, hash: &str) -> Option<&Entry> {
        self.entries.get(&(native.to_string(), hash.to_string()))
    }

    /// Alle Zeilen zu einem Native, gleich welchen Hashes — fuer die
    /// Meldung „geprueft, aber eine andere Fassung".
    pub fn any_for(&self, native: &str) -> Vec<&Entry> {
        self.entries.values().filter(|e| e.native == native).collect()
    }

    /// Alle Zeilen.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Nimmt eine Zeile auf; eine gleiche ersetzt sie.
    pub fn insert(&mut self, e: Entry) {
        self.entries.insert((e.native.clone(), e.hash.clone()), e);
    }

    /// Die kanonische Textform, sortiert.
    pub fn render(&self) -> String {
        let mut s = String::from("# natives.review — je Zeile: Native, SHA-256 der Quelle, Pruefer, Datum\n");
        for e in self.entries.values() {
            s.push_str(&format!("{} {} {} {}\n", e.native, e.hash, e.by, e.date));
        }
        s
    }
}

/// Liest die Datei; leere Zeilen und `#`-Kommentare werden uebergangen.
pub fn parse(text: &str) -> Result<Review, ParseError> {
    let mut out = Review::default();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [native, hash, by, date] = fields[..] else {
            return Err(ParseError {
                line: i as u32 + 1,
                message: format!("vier Felder erwartet (Native, Hash, Pruefer, Datum), {} gefunden", fields.len()),
            });
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseError { line: i as u32 + 1, message: format!("`{hash}` ist kein SHA-256 in Hex") });
        }
        out.insert(Entry {
            native: native.to_string(),
            hash: hash.to_ascii_lowercase(),
            by: by.to_string(),
            date: date.to_string(),
        });
    }
    Ok(out)
}

/// Der Hash einer Quelldatei, wie ihn die Datei fuehrt.
pub fn hash_of(source: &[u8]) -> String {
    let Hash256(bytes) = sha256(source);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
