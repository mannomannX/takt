//! Formatstrings (Referenz 3.9, Pruefung 16): Platzhalter parsen und im
//! aktuellen Bereich elaborieren, Formatangaben gegen den Typ pruefen,
//! Hoechstlaenge bestimmen.

use takt_diag::Span;
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::types::Type;
use takt_syntax::ast::StrLit;
use takt_syntax::subtext::{self, FormatPiece as Piece};
use takt_syntax::{parse_expr, tokenize_in};

use super::Lowerer;

/// Code der Formatstring-Pruefung.
pub const SC16: &str = "SC-16";

impl Lowerer<'_> {
    /// Formatstring eines Literals.
    pub fn format(&mut self, lit: &StrLit) -> Option<Format> {
        let pieces = match subtext::format_text(&lit.value) {
            Ok(p) => p,
            Err(d) => {
                let d = shift(d, lit.span, 0);
                self.diags.push(d);
                return None;
            }
        };
        let mut out = Vec::new();
        let mut len_max: u32 = 0;
        let mut it = pieces.into_iter().peekable();
        while let Some(piece) = it.next() {
            match piece {
                Piece::Text(t) => {
                    len_max += t.len() as u32;
                    out.push(FormatPiece::Text(t));
                }
                Piece::Expr(text, at) => {
                    let spec = match it.peek() {
                        Some(Piece::Spec(_)) => match it.next() {
                            Some(Piece::Spec(s)) => Some(s),
                            _ => None,
                        },
                        _ => None,
                    };
                    let edition = self.edition;
                    let toks = tokenize_in(&text, edition);
                    if let Some(e) = toks.errors.first() {
                        self.diags.push(shift(e.clone(), lit.span, at));
                        return None;
                    }
                    let ast = match parse_expr(&toks) {
                        Ok(e) => e,
                        Err(d) => {
                            self.diags.push(shift(d, lit.span, at));
                            return None;
                        }
                    };
                    // Die Spans des Teilausdrucks zaehlen ab dem Anfang des
                    // Platzhalters, nicht ab dem Dateianfang. Ohne Verschiebung
                    // landet jede Diagnose aus `expr` auf 1:1 (FB-31).
                    let first = self.diags.len();
                    let expr = self.expr(&ast, None);
                    for d in &mut self.diags[first..] {
                        d.span = shift_span(d.span, lit.span, at);
                    }
                    let mut expr = expr?;
                    expr.span = lit.span;
                    let width = self.placeholder_width(expr.ty, spec.as_deref(), lit.span)?;
                    len_max += width;
                    out.push(FormatPiece::Expr { expr, spec });
                }
                Piece::Spec(_) => {}
            }
        }
        Some(Format { pieces: out, len_max })
    }

    /// Hoechstbreite eines Platzhalters und Pruefung der Formatangabe.
    fn placeholder_width(&mut self, ty: takt_mir::TypeId, spec: Option<&str>, span: Span) -> Option<u32> {
        let t = self.ty(ty).clone();
        let base_width = match &t {
            Type::Bool => 5,
            Type::Int { .. } => 20,
            Type::Float { .. } => 24,
            Type::Duration { .. } => 24,
            Type::Enum(e) => {
                let def = &self.program.enums[e.index()];
                def.variants.iter().map(|v| v.name.len() as u32 + 2 + 24 * v.fields.len() as u32).max().unwrap_or(4)
            }
            Type::Str { cap } | Type::Line { cap } => *cap,
            Type::Optional(inner) => return self.placeholder_width(*inner, spec, span).map(|w| w.max(4)),
            Type::Record(r) => {
                let def = &self.program.records[r.index()];
                def.name.len() as u32 + 2 + def.fields.len() as u32 * 26
            }
            Type::Array { len, .. } => 2 + len * 26,
            _ => {
                let name = self.type_name(ty);
                self.error(SC16, span, format!("Wert vom Typ `{name}` ist nicht formatierbar"));
                return None;
            }
        };
        match spec {
            None => Some(base_width),
            Some("hex") => {
                if !matches!(t, Type::Int { .. }) {
                    self.error_hint(SC16, span, "`hex` nur fuer Ganzzahlen", "Formatangabe entfernen");
                    return None;
                }
                Some(16)
            }
            Some(s) if s.starts_with('.') => {
                if !matches!(t, Type::Float { .. }) {
                    self.error_hint(SC16, span, format!("`{s}` nur fuer Fliesskommazahlen"), "Formatangabe entfernen");
                    return None;
                }
                let prec: u32 = s[1..].parse().ok().filter(|p| *p <= 17).or_else(|| {
                    self.error(SC16, span, format!("Genauigkeit `{s}` ausserhalb 0..17"));
                    None
                })?;
                Some(24 + prec)
            }
            Some(s) if s.starts_with('0') => {
                if !matches!(t, Type::Int { .. }) {
                    self.error_hint(SC16, span, format!("`{s}` nur fuer Ganzzahlen"), "Formatangabe entfernen");
                    return None;
                }
                let width: u32 = s.parse().ok().filter(|w| *w <= 64).or_else(|| {
                    self.error(SC16, span, format!("Breite `{s}` ausserhalb 0..64"));
                    None
                })?;
                Some(base_width.max(width))
            }
            Some(other) => {
                self.error_hint(SC16, span, format!("unbekannte Formatangabe `{other}`"), "erlaubt: `hex`, `.N`, `0N`");
                None
            }
        }
    }
}

/// Verschiebt eine Diagnose relativ zum Stringinhalt an das Literal.
fn shift(mut d: takt_diag::Diagnostic, lit: Span, at: u32) -> takt_diag::Diagnostic {
    d.span = shift_span(d.span, lit, at);
    d.code = SC16;
    d
}

/// Rechnet einen Span im Platzhaltertext auf die Datei um: Anfang des
/// Literals, das oeffnende Anfuehrungszeichen, der Versatz des Platzhalters.
fn shift_span(inner: Span, lit: Span, at: u32) -> Span {
    let base = lit.start + 1 + at;
    Span { file: lit.file, start: base + inner.start, end: base + inner.end }
}
