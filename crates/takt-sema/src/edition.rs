//! Pruefung 49 (Referenz 2.5, 10): die Edition aus `system: language = N`.
//! Fehlt sie, gilt die neueste mit Warnung (im Zertifizierungsmodus ein
//! Fehler, siehe `Policy`); eine unbekannte Nummer ist ein Fehler.

use takt_diag::{Diagnostic, Span};
use takt_syntax::Edition;
use takt_syntax::edition::declared_edition;

/// Code der Pruefung.
pub const CODE: &str = "SC-49";

/// Liest die Edition vor dem Tokenizer und liefert sie mit den Diagnosen.
pub fn resolve(src: &str) -> (Edition, Vec<Diagnostic>) {
    match declared_edition(src) {
        None => {
            let known = Edition::LATEST.number();
            let d = Diagnostic::warning(CODE, Span::new(0, 0), "Edition fehlt: kein `language` im `system:`-Block")
                .with_suggestion(format!("`language = {known}` eintragen (takt fmt --edition traegt es ein)"));
            (Edition::LATEST, vec![d])
        }
        Some((text, span)) => match text.parse::<u32>().ok().and_then(Edition::from_number) {
            Some(edition) => (edition, Vec::new()),
            None => {
                let known: Vec<String> = Edition::ALL.iter().map(|e| e.number().to_string()).collect();
                let d = Diagnostic::error(CODE, span, format!("unbekannte Edition `{text}`"))
                    .with_suggestion(format!("bekannte Editionen: {}", known.join(", ")));
                (Edition::LATEST, vec![d])
            }
        },
    }
}
