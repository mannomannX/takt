//! Teilsprachen innerhalb von Stringliteralen (lexer.md L5.3 bis L5.6):
//! Formatstrings, Musterliterale, Hardware-Adressen. Die Eingabe ist der
//! Inhalt des Strings mit aufgeloesten Escapes; Positionen zaehlen Zeichen ab 1.

use crate::token::{ErrorCode, LexError};

/// Baustein eines Formatstrings (3.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatPiece {
    /// Text, Escapes `{{` und `}}` bereits aufgeloest.
    Text(String),
    /// Ausdruck zwischen `{` und `}` beziehungsweise `:`, unzerlegt.
    Expr(String),
    /// Formatangabe nach `:`: `hex`, `.3`, `08`.
    Spec(String),
}

/// Baustein eines Musterliterals (8.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternPiece {
    /// Literaler Text.
    Text(String),
    /// Platzhalter `{name:kind}`.
    Capture {
        /// Name der Bindung.
        name: String,
        /// Art: `int`, `hex`, `float`, `word`, `str` oder `str<N>`.
        kind: String,
    },
    /// `{_}`: beliebiger Text ohne Bindung.
    Any,
}

/// Segment einer Hardware-Adresse (8.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressSegment {
    /// Name des Segments.
    pub name: String,
    /// Bereich `[a:b]` fuer Channel-Arrays.
    pub range: Option<(String, String)>,
}

fn err(code: ErrorCode, col: usize, detail: &str) -> LexError {
    LexError { code, line: 1, col: col as u32 + 1, detail: detail.to_string() }
}

fn push_text(out: &mut Vec<FormatPiece>, text: &mut String) {
    if !text.is_empty() {
        out.push(FormatPiece::Text(std::mem::take(text)));
    }
}

/// Zerlegt einen Formatstring (L5.4).
pub fn format_text(s: &str) -> Result<Vec<FormatPiece>, LexError> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '{' if cs.get(i + 1) == Some(&'{') => {
                text.push('{');
                i += 2;
            }
            '}' if cs.get(i + 1) == Some(&'}') => {
                text.push('}');
                i += 2;
            }
            '}' => return Err(err(ErrorCode::Format, i, "} ohne {")),
            '{' => {
                push_text(&mut out, &mut text);
                let open = i;
                i += 1;
                let mut depth = 0i32;
                let mut expr = String::new();
                let mut spec: Option<String> = None;
                loop {
                    let Some(&c) = cs.get(i) else {
                        return Err(err(ErrorCode::Format, open, "{ ohne }"));
                    };
                    match spec.as_mut() {
                        None => match c {
                            '(' | '[' => {
                                depth += 1;
                                expr.push(c);
                            }
                            ')' | ']' => {
                                depth -= 1;
                                expr.push(c);
                            }
                            '"' | '{' => return Err(err(ErrorCode::Format, i, "String oder { im Ausdruck")),
                            '}' if depth == 0 => break,
                            ':' if depth == 0 => spec = Some(String::new()),
                            _ => expr.push(c),
                        },
                        Some(sp) => match c {
                            '}' => break,
                            _ => sp.push(c),
                        },
                    }
                    i += 1;
                }
                i += 1;
                if expr.trim().is_empty() {
                    return Err(err(ErrorCode::Format, open, "leerer Platzhalter"));
                }
                out.push(FormatPiece::Expr(expr));
                if let Some(sp) = spec {
                    if !format_spec(&sp) {
                        return Err(err(ErrorCode::Format, open, "Formatangabe: hex, .N oder N"));
                    }
                    out.push(FormatPiece::Spec(sp));
                }
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    push_text(&mut out, &mut text);
    Ok(out)
}

fn digits(t: &str) -> bool {
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

/// `format_spec := "hex" | "." INT | INT`
fn format_spec(spec: &str) -> bool {
    spec == "hex" || digits(spec) || spec.strip_prefix('.').is_some_and(digits)
}

/// `pattern_kind := "int" | "hex" | "float" | "word" | "str" [ "<" INT ">" ]`
fn pattern_kind(kind: &str) -> bool {
    matches!(kind, "int" | "hex" | "float" | "word" | "str")
        || kind.strip_prefix("str<").and_then(|r| r.strip_suffix('>')).is_some_and(digits)
}

/// Zerlegt ein Musterliteral (L5.5, 8.7).
pub fn pattern_text(s: &str) -> Result<Vec<PatternPiece>, LexError> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let flush = |out: &mut Vec<PatternPiece>, text: &mut String| {
        if !text.is_empty() {
            out.push(PatternPiece::Text(std::mem::take(text)));
        }
    };
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '{' if cs.get(i + 1) == Some(&'{') => {
                text.push('{');
                i += 2;
            }
            '}' if cs.get(i + 1) == Some(&'}') => {
                text.push('}');
                i += 2;
            }
            '}' => return Err(err(ErrorCode::Pattern, i, "} ohne {")),
            '{' => {
                flush(&mut out, &mut text);
                let open = i;
                let close =
                    cs[i..].iter().position(|&c| c == '}').ok_or_else(|| err(ErrorCode::Pattern, open, "{ ohne }"))?;
                let inner: String = cs[i + 1..i + close].iter().collect();
                i += close + 1;
                if inner == "_" {
                    out.push(PatternPiece::Any);
                    continue;
                }
                let (name, kind) =
                    inner.split_once(':').ok_or_else(|| err(ErrorCode::Pattern, open, "{name:kind} erwartet"))?;
                let name_ok = name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                    && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
                if !name_ok || !pattern_kind(kind) {
                    return Err(err(ErrorCode::Pattern, open, "Arten: int hex float word str str<N>"));
                }
                out.push(PatternPiece::Capture { name: name.to_string(), kind: kind.to_string() });
            }
            c => {
                text.push(c);
                i += 1;
            }
        }
    }
    flush(&mut out, &mut text);
    Ok(out)
}

/// Zerlegt eine Hardware-Adresse (L5.6, 8.1).
pub fn address_text(s: &str) -> Result<Vec<AddressSegment>, LexError> {
    let mut out = Vec::new();
    let mut col = 0;
    for seg in s.split('/') {
        out.push(address_segment(seg, col)?);
        col += seg.chars().count() + 1;
    }
    Ok(out)
}

/// `address_segment := ADDR_WORD [ "[" INT ":" INT "]" ]`; `col` ist die Spalte des Segments.
fn address_segment(seg: &str, col: usize) -> Result<AddressSegment, LexError> {
    let bytes = seg.as_bytes();
    let name_len = bytes
        .iter()
        .position(|&b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')))
        .unwrap_or(bytes.len());
    if name_len == 0 || !(bytes[0].is_ascii_alphanumeric() || bytes[0] == b'_') {
        return Err(err(ErrorCode::Address, col, "Segment erwartet"));
    }
    let rest = &seg[name_len..];
    let range = if rest.is_empty() {
        None
    } else {
        let bad = || err(ErrorCode::Address, col + name_len, "Bereich als [a:b]");
        let inner = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')).ok_or_else(bad)?;
        let (a, b) = inner.split_once(':').ok_or_else(bad)?;
        if !digits(a) || !digits(b) {
            return Err(bad());
        }
        Some((a.to_string(), b.to_string()))
    };
    Ok(AddressSegment { name: seg[..name_len].to_string(), range })
}
