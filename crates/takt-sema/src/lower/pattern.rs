//! Muster, Captures und Bindungen (Referenz 8.7, Pruefung 18).
//!
//! Ein Musterliteral wird von `takt_syntax::subtext::pattern_text` in
//! Bausteine zerlegt; hier entstehen daraus der MIR-Knoten, der Typ jedes
//! Captures und der Recordtyp der Bindung. Die Mehrdeutigkeitsregel aus 8.7
//! wird dabei geprueft: sie ist zugleich der Beweis, dass der Abgleich mit
//! einem Vorwaertsdurchlauf eindeutig ist (plan/m2.md 1.1).

use takt_diag::Span;
use takt_mir::machine::{VarDef, VarScope};
use takt_mir::pattern::{CaptureKind, Pattern, PatternPiece};
use takt_mir::types::{FieldDef, RecordDef, Type};
use takt_mir::{RecordId, TypeId, VarId};
use takt_syntax::ast;
use takt_syntax::subtext;

use super::{Lowerer, SC2, SC3};
use crate::checks::SC18;
use crate::symbols::Entity;

/// Ein gelowertes Muster samt dem Typ seiner Bindung.
pub struct Lowered {
    /// Knoten fuer die MIR.
    pub pattern: Pattern,
    /// Namen und Typen der Captures in Musterreihenfolge.
    pub captures: Vec<(String, TypeId)>,
}

/// Namen, die eine Bindung ohnehin traegt und die deshalb als Capture-Name
/// verboten sind (2.5).
const RESERVED: &[&str] = &["t", "seq", "text", "data"];

impl Lowerer<'_> {
    /// `pattern` aus dem Syntaxbaum, mit Pruefung 18.
    pub fn pattern(&mut self, p: &ast::Pattern, subject: Option<TypeId>, span: Span) -> Option<Lowered> {
        match p {
            ast::Pattern::Text(lit) => self.text_pattern(lit, span),
            ast::Pattern::Record { ty, fields } => self.record_pattern(ty, fields, subject, span),
        }
    }

    /// Musterliteral: zerlegen, pruefen, Typen der Captures bestimmen.
    fn text_pattern(&mut self, lit: &ast::StrLit, span: Span) -> Option<Lowered> {
        let pieces = match subtext::pattern_text(&lit.value) {
            Ok(p) => p,
            Err(d) => {
                // Die Spanne des Teiltexts ist relativ zum Literal.
                self.error(SC18, span, format!("Muster: {}", d.message));
                return None;
            }
        };
        let pieces: Vec<PatternPiece> = pieces
            .into_iter()
            .map(|p| match p {
                subtext::PatternPiece::Text(t) => PatternPiece::Text(t),
                subtext::PatternPiece::Any => PatternPiece::Any,
                subtext::PatternPiece::Capture { name, kind } => {
                    PatternPiece::Capture { name, kind: capture_kind(&kind) }
                }
            })
            .collect();
        self.check_pattern(&pieces, span)?;
        let captures = self.capture_types(&pieces, span)?;
        Some(Lowered { pattern: Pattern::Text { pieces, dfa: None }, captures })
    }

    /// Pruefung 18: Wohlgeformtheit und Mehrdeutigkeit (8.7).
    fn check_pattern(&mut self, pieces: &[PatternPiece], span: Span) -> Option<()> {
        let mut ok = true;
        for (i, piece) in pieces.iter().enumerate() {
            let next = pieces.get(i + 1);
            match piece {
                PatternPiece::Text(_) => {}
                // Zwei Platzhalter duerfen nicht direkt aufeinander folgen:
                // ihre Grenze waere nicht bestimmt.
                PatternPiece::Any | PatternPiece::Capture { .. }
                    if matches!(next, Some(PatternPiece::Any | PatternPiece::Capture { .. })) =>
                {
                    self.error_hint(
                        SC18,
                        span,
                        "zwei Platzhalter folgen direkt aufeinander",
                        "einen literalen Text dazwischen setzen (8.7)",
                    );
                    ok = false;
                }
                PatternPiece::Any => {}
                PatternPiece::Capture { name, kind } => {
                    // Auf eine klassengebundene Art muss ein Literal folgen,
                    // dessen erstes Zeichen nicht zur Klasse gehoert, sonst
                    // ist die Capture-Grenze nicht eindeutig.
                    if let Some(first) = class_of(kind) {
                        match next {
                            None => {}
                            Some(PatternPiece::Text(t)) if !t.starts_with(first) => {}
                            Some(PatternPiece::Text(t)) => {
                                let c = t.chars().next().unwrap_or(' ');
                                self.error_hint(
                                    SC18,
                                    span,
                                    format!("mehrdeutiges Muster: `{c}` nach `{{{name}:{}}}`", kind_name(kind)),
                                    "ein Zeichen ausserhalb der Klasse des Platzhalters anfuegen (8.7)",
                                );
                                ok = false;
                            }
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        ok.then_some(())
    }

    /// Typ je Capture, mit den Namensregeln aus 2.5.
    fn capture_types(&mut self, pieces: &[PatternPiece], span: Span) -> Option<Vec<(String, TypeId)>> {
        let mut out: Vec<(String, TypeId)> = Vec::new();
        let mut ok = true;
        for piece in pieces {
            let PatternPiece::Capture { name, kind } = piece else { continue };
            if RESERVED.contains(&name.as_str()) {
                self.error_hint(
                    SC18,
                    span,
                    format!("`{name}` ist als Capture-Name reserviert"),
                    "eine Bindung traegt `t`, `seq`, `text` und `data` bereits (2.5)",
                );
                ok = false;
            }
            if out.iter().any(|(n, _)| n == name) {
                self.error(SC18, span, format!("Capture `{name}` doppelt"));
                ok = false;
            }
            let ty = match kind {
                CaptureKind::Int | CaptureKind::Hex => self.tys.int,
                CaptureKind::Float => self.tys.float,
                CaptureKind::Word => self.intern(Type::Str { cap: 64 }),
                CaptureKind::Str(n) => self.intern(Type::Str { cap: *n }),
            };
            out.push((name.clone(), ty));
        }
        ok.then_some(out)
    }

    /// Record-Muster `CanFrame(id = 0x7E8)`: aufgefuehrte Felder gleichen
    /// konstanten Ausdruecken (8.7).
    fn record_pattern(
        &mut self,
        ty: &ast::Ident,
        fields: &[(ast::Ident, ast::Expr)],
        subject: Option<TypeId>,
        span: Span,
    ) -> Option<Lowered> {
        let record = self.record_id(ty, subject, span)?;
        let defs = self.program.records[record.index()].fields.clone();
        let mut out = Vec::new();
        let mut ok = true;
        for (name, value) in fields {
            let Some(index) = defs.iter().position(|f| f.name == name.name) else {
                self.error(SC2, name.span, format!("`{}` hat kein Feld `{}`", ty.name, name.name));
                ok = false;
                continue;
            };
            let Some(expr) = self.check(value, defs[index].ty) else {
                ok = false;
                continue;
            };
            // 8.7: nur konstante Feldwerte; der Vergleich ist eine endliche
            // Konjunktion von Gleichheiten und damit total.
            let Some(folded) = self.fold(expr) else {
                self.error_hint(
                    SC18,
                    value.span,
                    "Record-Muster verlangt einen konstanten Feldwert",
                    "einen Literalwert oder eine Konstante einsetzen (8.7)",
                );
                ok = false;
                continue;
            };
            out.push((index as u32, folded));
        }
        ok.then(|| Lowered { pattern: Pattern::Record { record, fields: out }, captures: Vec::new() })
    }

    /// Recordtyp eines Record-Musters: aus dem Namen, gegen den Elementtyp
    /// des Subjekts geprueft.
    fn record_id(&mut self, ty: &ast::Ident, subject: Option<TypeId>, span: Span) -> Option<RecordId> {
        let record = match self.lookup(ty)? {
            crate::symbols::Entity::Record(r) => r,
            _ => {
                self.error(SC3, ty.span, format!("`{}` ist kein Record", ty.name));
                return None;
            }
        };
        if let Some(subject) = subject {
            if !matches!(self.ty(subject), Type::Record(r) if *r == record) {
                let n = self.type_name(subject);
                self.error(SC3, span, format!("Muster fuer `{}`, Subjekt ist `{n}`", ty.name));
                return None;
            }
        }
        Some(record)
    }

    /// Recordtyp der Bindung `as b`: die Captures, gefolgt von den Feldern,
    /// die ein Stream-Element traegt (8.7). Der Name ist nicht aus Quelltext
    /// erreichbar, erscheint aber in Dump und Diagnosen.
    pub fn binding_type(
        &mut self,
        name: &str,
        captures: &[(String, TypeId)],
        elem: Option<TypeId>,
        span: Span,
    ) -> TypeId {
        let mut fields: Vec<FieldDef> = captures.iter().map(|(n, ty)| field(n, *ty, span)).collect();
        if let Some(elem) = elem {
            let duration = self.tys.duration;
            let int = self.tys.int;
            fields.push(field("t", duration, span));
            fields.push(field("seq", int, span));
            // `line<N>` traegt `.text`, `bytes<N>` traegt `.data` (8.6).
            match self.ty(elem) {
                Type::Line { .. } => fields.push(field("text", elem, span)),
                Type::Bytes { .. } => fields.push(field("data", elem, span)),
                _ => {}
            }
        }
        let id = RecordId(self.program.records.len() as u32);
        self.program.records.push(RecordDef { name: name.to_string(), fields, layout: None, builtin: true, span });
        self.intern(Type::Record(id))
    }
}

impl Lowerer<'_> {
    /// Legt die Bindung `as b` an: eine gehobene Variable vom Recordtyp der
    /// Bindung (plan/m2.md 1.6, 1.7). Sie ist erst nach dem Guard
    /// zugewiesen, was die Definite-Assignment-Pruefung ausloest.
    pub fn binding_var(
        &mut self,
        name: &ast::Ident,
        captures: &[(String, TypeId)],
        elem: Option<TypeId>,
        span: Span,
    ) -> Option<VarId> {
        let state = self.mctx.as_ref().and_then(|m| m.current);
        let type_name = match &self.mctx {
            Some(m) => format!("{}.{}.binding", m.machine.name, name.name),
            None => format!("{}.binding", name.name),
        };
        let ty = self.binding_type(&type_name, captures, elem, span);
        let scope = match state {
            Some(s) => VarScope::Lifted(s),
            None => VarScope::Machine,
        };
        let id = self.new_var(VarDef {
            name: name.name.clone(),
            ty,
            // Ohne Initialisierer: die Bindung entsteht erst, wenn das Muster
            // trifft (8.7, „nur im dominierten Zweig sichtbar").
            init: None,
            scope,
            public: false,
            span,
        });
        self.declare(name, Entity::Var(id, ty)).then_some(id)
    }
}

fn field(name: &str, ty: TypeId, span: Span) -> FieldDef {
    FieldDef { name: name.to_string(), ty, const_value: None, offset: None, len_field: None, bits: Vec::new(), span }
}

/// Art eines Platzhalters aus dem Text der Teilsprache.
fn capture_kind(kind: &str) -> CaptureKind {
    match kind {
        "int" => CaptureKind::Int,
        "hex" => CaptureKind::Hex,
        "float" => CaptureKind::Float,
        "word" => CaptureKind::Word,
        // `str` ohne Schranke ist `str<64>` wie `word` (8.7).
        "str" => CaptureKind::Str(64),
        other => {
            let n = other.strip_prefix("str<").and_then(|r| r.strip_suffix('>')).and_then(|d| d.parse().ok());
            CaptureKind::Str(n.unwrap_or(64))
        }
    }
}

fn kind_name(kind: &CaptureKind) -> String {
    match kind {
        CaptureKind::Int => "int".into(),
        CaptureKind::Hex => "hex".into(),
        CaptureKind::Float => "float".into(),
        CaptureKind::Word => "word".into(),
        CaptureKind::Str(n) => format!("str<{n}>"),
    }
}

/// Gehoert ein Zeichen zur Klasse der Art? `None` fuer die offenen Arten,
/// deren Grenze das Folgeliteral bestimmt (8.7).
fn class_of(kind: &CaptureKind) -> Option<fn(char) -> bool> {
    match kind {
        CaptureKind::Int => Some(|c| c.is_ascii_digit() || c == '+' || c == '-'),
        CaptureKind::Hex => Some(|c| c.is_ascii_hexdigit() || c == 'x'),
        CaptureKind::Float => Some(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E')),
        CaptureKind::Word => Some(|c| c.is_ascii_alphanumeric() || c == '_'),
        CaptureKind::Str(_) => None,
    }
}
