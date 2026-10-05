//! Formatstrings (Referenz 3.9, Pruefung 16): Platzhalter parsen und im
//! aktuellen Bereich elaborieren, Formatangaben gegen den Typ pruefen,
//! Hoechstlaenge bestimmen.

use takt_diag::Span;
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::types::{Const, FloatWidth, IntWidth, Range, Type};
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
        if let Type::Optional(inner) = t {
            return self.placeholder_width(inner, spec, span).map(|w| w.max(4));
        }
        // Formatierbar sind Skalare, Enums, Texte, Records und Arrays; in
        // ihnen steht jeder Teil, den die Textform schreibt.
        let formattable = matches!(
            t,
            Type::Bool
                | Type::Int { .. }
                | Type::Float { .. }
                | Type::Duration { .. }
                | Type::Enum(_)
                | Type::Str { .. }
                | Type::Line { .. }
                | Type::Record(_)
                | Type::Array { .. }
        );
        let Some(base_width) = self.display_width(ty).filter(|_| formattable) else {
            let name = self.type_name(ty);
            self.error(SC16, span, format!("Wert vom Typ `{name}` ist nicht formatierbar"));
            return None;
        };
        // 3.5: Ein ungueltiger Wert steht als `<invalid>` da; auch der
        // passt, gekuerzt wird nie (Pruefung 16).
        let base_width = base_width.max(INVALID.len() as u32);
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
                let Type::Float { width, .. } = t else {
                    self.error_hint(SC16, span, format!("`{s}` nur fuer Fliesskommazahlen"), "Formatangabe entfernen");
                    return None;
                };
                let prec: u32 = s[1..].parse().ok().filter(|p| *p <= 17).or_else(|| {
                    self.error(SC16, span, format!("Genauigkeit `{s}` ausserhalb 0..17"));
                    None
                })?;
                // Dieselbe Form mit N Nachkommastellen (3.9): Vorzeichen, die
                // Stellen vor dem Punkt, Punkt, N; die Exponentform ist kuerzer.
                Some(fixed_digits(width) + 2 + prec)
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

/// Stellen vor dem Punkt ohne Exponent (3.9, `grammar/trace.md` T2): Ein
/// `f64` steht unter 1e16 in fester Form, ein `f32` unter 1e7.
fn fixed_digits(width: FloatWidth) -> u32 {
    match width {
        FloatWidth::F64 => 16,
        FloatWidth::F32 => 7,
    }
}

/// Hoechstbreite eines Gleitkommawerts in kuerzester Form (3.9), Exponent
/// unter 1e-5 und ab 1e16 (`f64`) bzw. 1e7 (`f32`). `f64`:
/// `-1.2345678901234567e-308` und `-0.000012345678901234567`, je 24
/// Zeichen. `f32`: `-0.0000123456789`, 16 Zeichen; die Exponentform
/// (`-1.23456789e-38`) und die feste Form unter 1e7 sind kuerzer.
fn float_width(width: FloatWidth) -> u32 {
    match width {
        FloatWidth::F64 => 24,
        FloatWidth::F32 => 16,
    }
}

/// Der Text eines ungueltigen Platzhalters (3.5), wie der Interpreter ihn
/// schreibt.
const INVALID: &str = "<invalid>";

/// Hoechstbreite einer Ganzzahl in Dezimalschreibweise samt Vorzeichen
/// (8.8): aus der Range, sonst aus der Breite — `int in 0..9` braucht ein
/// Zeichen, `i64` zwanzig. Die Range ist Invariante des Werts (3.4).
fn int_width(width: IntWidth, range: &Option<Range>) -> u32 {
    let (lo, hi) = match range {
        Some(Range { lo: Const::Int(lo), hi: Const::Int(hi), .. }) => (i128::from(*lo), i128::from(*hi)),
        _ if width.signed() => (-(1i128 << (width.bits() - 1)), (1i128 << (width.bits() - 1)) - 1),
        _ => (0, (1i128 << width.bits()) - 1),
    };
    let digits = |v: i128| v.unsigned_abs().checked_ilog10().map_or(1, |d| d + 1) + u32::from(v < 0);
    digits(lo).max(digits(hi))
}

impl Lowerer<'_> {
    /// Hoechstbreite der Textform eines Werts, wie `takt_interp::format`
    /// sie schreibt (8.8): Records als `Name(a, b)`, Arrays als `[a, b]`,
    /// Enums mit Feldern als `V(a, b)`, je Teil seine eigene Breite. `None`,
    /// wenn ein Teil nicht formatierbar ist.
    fn display_width(&self, ty: takt_mir::TypeId) -> Option<u32> {
        Some(match self.ty(ty) {
            Type::Bool => 5,
            Type::Int { width, range, .. } => int_width(*width, range),
            Type::Float { width, .. } => float_width(*width),
            // `-9223372036854775808 ns`
            Type::Duration { .. } => 23,
            Type::Str { cap } | Type::Line { cap } => *cap,
            Type::Optional(inner) => self.display_width(*inner)?.max(4),
            Type::Enum(e) => self.enum_width(*e)?,
            Type::Record(r) => {
                let def = &self.program.records[r.index()];
                let fields: Vec<takt_mir::TypeId> = def.fields.iter().map(|f| f.ty).collect();
                (def.name.len() as u32).saturating_add(2).saturating_add(self.parts_width(&fields)?)
            }
            Type::Array { elem, len } | Type::Vec { elem, cap: len } | Type::Samples { elem, len } => {
                let each = self.display_width(*elem)?;
                each.saturating_mul(*len).saturating_add(commas(*len)).saturating_add(2)
            }
            // Hexpaare mit Leerzeichen dazwischen.
            Type::Bytes { cap } => (3 * *cap).saturating_sub(1),
            Type::Mat { rows, cols, .. } => {
                let each = float_width(self.float_width());
                let row = each.saturating_mul(*cols).saturating_add(commas(*cols)).saturating_add(2);
                row.saturating_mul(*rows).saturating_add(commas(*rows)).saturating_add(2)
            }
            // `OK(v)` oder `ERR(e)`.
            Type::Result { ok, err } => {
                self.display_width(*ok)?.saturating_add(4).max(self.enum_width(*err)?.saturating_add(5))
            }
            _ => return None,
        })
    }

    /// Die laengste Variante: `V` oder `V(a, b)`.
    fn enum_width(&self, e: takt_mir::EnumId) -> Option<u32> {
        let mut most = 0u32;
        for v in &self.program.enums[e.index()].variants {
            let fields: Vec<takt_mir::TypeId> = v.fields.iter().map(|f| f.ty).collect();
            let args = if fields.is_empty() { 0 } else { self.parts_width(&fields)?.saturating_add(2) };
            most = most.max((v.name.len() as u32).saturating_add(args));
        }
        Some(most)
    }

    /// Teile mit `, ` dazwischen.
    fn parts_width(&self, tys: &[takt_mir::TypeId]) -> Option<u32> {
        let mut sum = commas(tys.len() as u32);
        for t in tys {
            sum = sum.saturating_add(self.display_width(*t)?);
        }
        Some(sum)
    }
}

/// Breite der `, ` zwischen `n` Teilen.
fn commas(n: u32) -> u32 {
    2 * n.saturating_sub(1)
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
