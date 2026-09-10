//! Pruefung 50 (Referenz 2.5, 10): reservierte Membernamen als Feld-, Bitfeld-
//! oder Capture-Namen. Reservierte *Woerter* als Bezeichner meldet der
//! Tokenizer (`E_RESERVED`); `lib.rs` fuehrt sie unter derselben Nummer.

use takt_diag::{Diagnostic, Span};
use takt_syntax::Edition;
use takt_syntax::ast::*;
use takt_syntax::subtext::{PatternPiece, pattern_text};

use crate::visit::{Scopes, Visitor, walk_file};

/// Code der Pruefung.
pub const CODE: &str = "SC-50";

/// Prueft Felder, Varianten, Bitfelder und Capture-Namen.
pub fn check(file: &File, edition: Edition) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for item in &file.items {
        match item {
            Item::Record(r) => {
                for f in &r.fields {
                    match f {
                        RecordField::Plain(field) => check_field(&field.name, edition, &mut out),
                        RecordField::Bits { name, bits, .. } => {
                            check_field(name, edition, &mut out);
                            for b in bits {
                                check_field(&b.name, edition, &mut out);
                            }
                        }
                    }
                }
            }
            Item::Enum(e) => {
                for v in &e.variants {
                    for f in &v.fields {
                        check_field(&f.name, edition, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    let mut captures = Captures { edition, out: &mut out };
    walk_file(file, &mut captures);
    out
}

fn check_field(name: &Ident, edition: Edition, out: &mut Vec<Diagnostic>) {
    if edition.wrapper_accessors().contains(&name.name.as_str()) {
        out.push(
            Diagnostic::error(CODE, name.span, format!("`{}` ist als Feldname verboten", name.name))
                .with_suggestion(format!(
                    "Zugriff eines Wrappers (T?, T!E, Channel, Handle): auf `x.{}` ist immer der Wrapper gemeint; Feld umbenennen, etwa `is_{}`",
                    name.name, name.name
                )),
        );
    }
}

struct Captures<'a> {
    edition: Edition,
    out: &'a mut Vec<Diagnostic>,
}

impl Visitor for Captures<'_> {
    fn pattern(&mut self, pattern: &Pattern) {
        let Pattern::Text(lit) = pattern else { return };
        let Ok(pieces) = pattern_text(&lit.value) else { return };
        for piece in pieces {
            if let PatternPiece::Capture { name, .. } = piece {
                if self.edition.capture_names().contains(&name.as_str()) {
                    let span = Span { file: lit.span.file, start: lit.span.start, end: lit.span.end };
                    self.out.push(
                        Diagnostic::error(CODE, span, format!("`{name}` ist als Capture-Name verboten"))
                            .with_suggestion(
                                "die Bindung traegt selbst t, seq, text und data (8.7); Capture umbenennen",
                            ),
                    );
                }
            }
        }
    }

    fn stmt(&mut self, _stmt: &Stmt, _scopes: &Scopes) {}
}
