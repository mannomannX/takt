//! Formatter (`takt fmt`): erzeugt aus einer syntaktisch gueltigen Datei die
//! kanonische Form nach `grammar/format.md`. Der Formatter bricht keine Zeile
//! um, behaelt die strukturellen Wahlen des Autors und normiert Leerraum,
//! Einrueckung, Spaltenausrichtung, Kommentarabstand und Leerzeilen.
//!
//! Stufen: Tokenizer und Parser (Fehler brechen ab), Drucken aus dem Baum mit
//! Beiwerk aus dem Tokenstrom (`emit.rs`, `print.rs`), Ausrichtung und
//! Endausgabe (`align.rs`).

mod align;
mod emit;
mod print;
mod verify;

pub use verify::verify;

use crate::Edition;
use crate::ast::{Item, SystemItem};
use crate::edition::declared_edition;

use takt_diag::Diagnostic;

use crate::parser::{parse_file_here, parse_snippet_here, with_deep_stack};
use crate::token::Tokens;
use crate::tokenize;

/// Formatiert eine Datei. Bei Tokenizer-, Parser- oder Formatterfehlern bleibt
/// die Datei unberuehrt und die Fehler werden zurueckgegeben, mit Stellen in `src`.
pub fn format(src: &str) -> Result<String, Vec<Diagnostic>> {
    let repaired = repair(src);
    with_deep_stack(|| {
        let toks = checked_tokens(&repaired.text)?;
        let (file, errors) = parse_file_here(&toks);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut e = emit::Emitter::new(&toks);
        e.fmt_file(&file);
        finish(e)
    })
    .map_err(|errors| repaired.relocate(errors))
}

/// Formatiert einen Schnipsel (Deklarationen, Zustandsinhalte und Anweisungen
/// gemischt, siehe `parse_snippet`).
pub fn format_snippet(src: &str) -> Result<String, Vec<Diagnostic>> {
    let repaired = repair(src);
    with_deep_stack(|| {
        let toks = checked_tokens(&repaired.text)?;
        let (items, errors) = parse_snippet_here(&toks);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut e = emit::Emitter::new(&toks);
        e.fmt_snippet(&items);
        finish(e)
    })
    .map_err(|errors| repaired.relocate(errors))
}

/// `takt fmt --edition`: traegt `language = N` ein, wenn es fehlt (2.5). Das
/// aendert den Tokenstrom und ist deshalb kein Teil von `format`. Liefert den
/// neuen, noch nicht formatierten Text, oder `None`, wenn nichts zu tun ist
/// oder die Datei nicht parst. Was `format` repariert (BOM, Tabulatoren), ist
/// im neuen Text schon repariert.
pub fn insert_edition(src: &str, edition: Edition) -> Option<String> {
    if declared_edition(src).is_some() {
        return None;
    }
    let src = repair(src).text;
    let src = src.as_str();
    let toks = tokenize(src);
    if !toks.errors.is_empty() {
        return None;
    }
    let (file, errors) = with_deep_stack(|| parse_file_here(&toks));
    if !errors.is_empty() {
        return None;
    }
    let entry = format!("language = {}", edition.number());
    let system = file.items.iter().find_map(|i| if let Item::System(s) = i { Some(s) } else { None });
    let (at, text) = match system {
        Some(s) if s.items.iter().any(|i| matches!(i, SystemItem::Language(_))) => return None,
        Some(s) => {
            // vor den ersten Eintrag des Blocks, auf dessen Zeilenanfang; der
            // Block beginnt mit dem Wort `system` an `s.span.start`
            let system_line = toks.tokens.iter().find(|t| t.start == s.span.start)?.line;
            let first =
                toks.tokens.iter().find(|t| t.start > s.span.start && t.line > system_line && !t.kind.is_layout())?;
            let line_start = src[..first.start as usize].rfind('\n').map_or(0, |p| p + 1);
            (line_start, format!("    {entry}\n"))
        }
        None => {
            // ein neuer Block vor der ersten Deklaration, hinter fuehrenden Kommentaren
            let first = toks.tokens.first()?;
            let line_start = src[..first.start as usize].rfind('\n').map_or(0, |p| p + 1);
            (line_start, format!("system:\n    {entry}\n\n"))
        }
    };
    let mut out = String::with_capacity(src.len() + text.len());
    out.push_str(&src[..at]);
    out.push_str(&text);
    out.push_str(&src[at..]);
    Some(out)
}

/// Ein reparierter Text und je Byte sein Versatz im Original.
struct Repaired {
    text: String,
    /// Versatz im Original je Byte von `text`, dazu einer fuer sein Ende;
    /// leer, wenn nichts zu reparieren war.
    origin: Vec<u32>,
}

impl Repaired {
    /// Verschiebt die Stellen der Fehler zurueck in das Original, damit sie auf
    /// die Datei zeigen, die der Nutzer sieht.
    fn relocate(&self, mut errors: Vec<Diagnostic>) -> Vec<Diagnostic> {
        let Some(&end) = self.origin.last() else { return errors };
        let at = |offset: u32| self.origin.get(offset as usize).copied().unwrap_or(end);
        for d in &mut errors {
            (d.span.start, d.span.end) = (at(d.span.start), at(d.span.end));
            for (span, _) in &mut d.notes {
                (span.start, span.end) = (at(span.start), at(span.end));
            }
        }
        errors
    }
}

/// Was `takt fmt` laut lexer.md L9 selbst behebt: die BOM am Dateianfang und
/// Tabulatoren ausserhalb von Strings und Kommentaren (in der Einrueckung je 4
/// Leerzeichen, sonst ein Leerzeichen). Ein Kommentar bleibt, wie er ist (L1.5, L7).
fn repair(src: &str) -> Repaired {
    let body = src.strip_prefix('\u{feff}').unwrap_or(src);
    if body.len() == src.len() && !src.contains('\t') {
        return Repaired { text: src.to_string(), origin: Vec::new() };
    }
    let mut text = String::with_capacity(src.len());
    let mut origin = Vec::with_capacity(src.len() + 1);
    let mut at = (src.len() - body.len()) as u32;
    for line in body.split_inclusive('\n') {
        let mut in_string = false;
        let mut in_comment = false;
        let mut escaped = false;
        let mut leading = true;
        for c in line.chars() {
            if c == '\t' && !in_string && !in_comment {
                text.push_str(if leading { "    " } else { " " });
                origin.resize(text.len(), at);
            } else {
                match c {
                    '"' if !escaped && !in_comment => in_string = !in_string,
                    '#' if !in_string => in_comment = true,
                    _ => {}
                }
                leading &= c == ' ';
                text.push(c);
                origin.extend((0..c.len_utf8() as u32).map(|i| at + i));
            }
            at += c.len_utf8() as u32;
            escaped = in_string && c == '\\' && !escaped;
        }
    }
    origin.push(at);
    Repaired { text, origin }
}

fn checked_tokens(src: &str) -> Result<Tokens<'_>, Vec<Diagnostic>> {
    let toks = tokenize(src);
    if toks.errors.is_empty() { Ok(toks) } else { Err(toks.errors.clone()) }
}

fn finish(e: emit::Emitter<'_, '_>) -> Result<String, Vec<Diagnostic>> {
    if e.errors.is_empty() { Ok(align::render(&e.lines)) } else { Err(e.errors) }
}
