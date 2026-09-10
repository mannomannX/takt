//! Sema: Symbole, Edition und die Pruefungen aus Referenz 10.
//!
//! `check` liest die Edition vorab (`system: language = N`), tokenisiert mit
//! ihrem Wortschatz, parst und fuehrt die Pruefungen; jede Diagnose traegt den
//! Code ihrer Pruefung (`SC-n`), damit Inventur, Tests und Meldungen dieselbe
//! Nummer haben. Umgesetzt in diesem Schritt: 49 (Edition), 50 (reservierte
//! Namen), 51 (offene Enums, als Naeherung ohne Typinferenz).

pub mod edition;
pub mod enums;
pub mod names;
pub mod visit;

use takt_diag::{Diagnostic, Policy, Sink};
use takt_syntax::Edition;
use takt_syntax::ast::File;
use takt_syntax::{parse_file, tokenize_in};

/// Ergebnis von `check`.
#[derive(Debug)]
pub struct Checked {
    /// Der Baum, wenn Tokenizer und Parser ohne Fehler waren.
    pub file: Option<File>,
    /// Die verwendete Edition.
    pub edition: Edition,
    /// Alle Diagnosen, nach Position sortiert und mit angewandter Politik.
    pub diagnostics: Vec<Diagnostic>,
}

impl Checked {
    /// Gibt es Fehler?
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Prueft eine Datei: Edition, Tokenizer, Parser, dann die semantischen
/// Pruefungen (nur bei fehlerfreier Syntax, damit ein kaputter Baum keine
/// Folgemeldungen erzeugt).
pub fn check(src: &str, policy: Policy) -> Checked {
    let mut sink = Sink::new(policy);
    let (edition, edition_diags) = edition::resolve(src);
    sink.extend(edition_diags);
    let toks = tokenize_in(src, edition);
    let syntax_ok = toks.errors.is_empty();
    sink.extend(toks.errors.iter().cloned().map(relabel_reserved));
    let (file, parse_errors) = parse_file(&toks);
    let syntax_ok = syntax_ok && parse_errors.is_empty();
    sink.extend(parse_errors);
    if !syntax_ok {
        return Checked { file: None, edition, diagnostics: sink.sorted() };
    }
    sink.extend(names::check(&file, edition));
    sink.extend(enums::check(&file, edition));
    Checked { file: Some(file), edition, diagnostics: sink.sorted() }
}

/// Reservierte Woerter als Bezeichner meldet der Tokenizer; Referenz 10 fuehrt
/// sie unter Pruefung 50.
fn relabel_reserved(mut d: Diagnostic) -> Diagnostic {
    if d.code == "E_RESERVED" {
        d.code = names::CODE;
    }
    d
}
