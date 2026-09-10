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
/// die Datei unberuehrt und die Fehler werden zurueckgegeben.
pub fn format(src: &str) -> Result<String, Vec<Diagnostic>> {
    let src = repair(src);
    with_deep_stack(|| {
        let toks = checked_tokens(&src)?;
        let (file, errors) = parse_file_here(&toks);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut e = emit::Emitter::new(&toks);
        e.fmt_file(&file);
        finish(e)
    })
}

/// Formatiert einen Schnipsel (Deklarationen, Zustandsinhalte und Anweisungen
/// gemischt, siehe `parse_snippet`).
pub fn format_snippet(src: &str) -> Result<String, Vec<Diagnostic>> {
    let src = repair(src);
    with_deep_stack(|| {
        let toks = checked_tokens(&src)?;
        let (items, errors) = parse_snippet_here(&toks);
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut e = emit::Emitter::new(&toks);
        e.fmt_snippet(&items);
        finish(e)
    })
}

/// `takt fmt --edition`: traegt `language = N` ein, wenn es fehlt (2.5). Das
/// aendert den Tokenstrom und ist deshalb kein Teil von `format`. Liefert den
/// neuen, noch nicht formatierten Text, oder `None`, wenn nichts zu tun ist
/// oder die Datei nicht parst.
pub fn insert_edition(src: &str, edition: Edition) -> Option<String> {
    if declared_edition(src).is_some() {
        return None;
    }
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
            // vor den ersten Eintrag des Blocks, auf dessen Zeilenanfang
            let first = toks
                .tokens
                .iter()
                .find(|t| t.start > s.span.start && t.line > toks.tokens[0].line && !t.kind.is_layout())?;
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

/// Was `takt fmt` laut lexer.md L9 selbst behebt: die BOM am Dateianfang und
/// Tabulatoren ausserhalb von Strings (in der Einrueckung je 4 Leerzeichen,
/// sonst ein Leerzeichen).
fn repair(src: &str) -> String {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    if !src.contains('\t') {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        let mut in_string = false;
        let mut escaped = false;
        let mut leading = true;
        for c in line.chars() {
            match c {
                '\t' if !in_string => out.push_str(if leading { "    " } else { " " }),
                '"' if !escaped => {
                    in_string = !in_string;
                    leading = false;
                    out.push(c);
                }
                _ => {
                    if c != ' ' {
                        leading = false;
                    }
                    out.push(c);
                }
            }
            escaped = in_string && c == '\\' && !escaped;
        }
    }
    out
}

fn checked_tokens(src: &str) -> Result<Tokens<'_>, Vec<Diagnostic>> {
    let toks = tokenize(src);
    if toks.errors.is_empty() { Ok(toks) } else { Err(toks.errors.clone()) }
}

fn finish(e: emit::Emitter<'_, '_>) -> Result<String, Vec<Diagnostic>> {
    if e.errors.is_empty() { Ok(align::render(&e.lines)) } else { Err(e.errors) }
}
