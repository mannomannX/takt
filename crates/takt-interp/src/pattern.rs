//! Musterabgleich (Referenz 8.7): `matches`, `has` und die Muster der
//! Handler und Stream-Guards.
//!
//! Ein Muster ist eine lineare Folge `l0 c1 l1 … ck lk` aus Literalen und
//! Platzhaltern — keine Alternative, keine Wiederholung, keine
//! Verschachtelung. Die Mehrdeutigkeitsregel aus 8.7 (auf `int`, `hex`,
//! `float` und `word` folgt ein Literal, dessen erstes Zeichen nicht zur
//! Klasse gehoert; `str` und `_` enden leftmost-shortest) macht jede
//! Capture-Grenze eindeutig. Der Abgleich ist deshalb ein einziger
//! Vorwaertsdurchlauf ohne Ruecksetzen, in derselben Komplexitaet wie der
//! Automat aus 8.7; die `Dfa`-Tabelle der MIR bleibt Codegen-Annotation
//! (plan/m2.md 1.1).

use takt_mir::pattern::{CaptureKind, Pattern, PatternPiece};

use crate::value::Value;

/// Ergebnis eines Abgleichs: die Werte der Captures in Musterreihenfolge.
pub type Captures = Vec<Value>;

/// Gleicht ein Textmuster gegen den ganzen Text ab (`matches`, 8.7).
/// `None` heisst „kein Match"; ein Muster faultet nie.
pub fn match_text(pieces: &[PatternPiece], text: &str) -> Option<Captures> {
    let mut caps = Vec::new();
    let rest = walk(pieces, text, &mut caps)?;
    // `matches` verlangt den ganzen Text; ein Rest bedeutet kein Match.
    if rest.is_empty() { Some(caps) } else { None }
}

/// `has P` ist die Kurzform fuer `matches "{_}" + P + "{_}"` (8.7): das
/// Muster darf an jeder Position beginnen und muss nicht bis zum Ende
/// reichen. Gesucht wird das linkeste Vorkommen.
pub fn match_has(pieces: &[PatternPiece], text: &str) -> Option<Captures> {
    for start in char_starts(text) {
        let mut caps = Vec::new();
        if walk(pieces, &text[start..], &mut caps).is_some() {
            return Some(caps);
        }
    }
    None
}

/// Positionen, an denen ein Zeichen beginnt, samt der Position hinter dem
/// letzten Zeichen (ein leeres Muster passt auch am Ende).
fn char_starts(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len()))
}

/// Laeuft die Bausteine von links nach rechts ab und liefert den Rest des
/// Texts. Jeder Baustein verbraucht ein Praefix des verbleibenden Texts.
fn walk<'a>(pieces: &[PatternPiece], text: &'a str, caps: &mut Captures) -> Option<&'a str> {
    let mut rest = text;
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            PatternPiece::Text(lit) => rest = rest.strip_prefix(lit.as_str())?,
            PatternPiece::Any => {
                let (_, tail) = open_end(&pieces[i + 1..], rest, None)?;
                rest = tail;
            }
            PatternPiece::Capture { kind, .. } => {
                let (taken, tail) = take(kind, &pieces[i + 1..], rest)?;
                caps.push(value_of(kind, taken)?);
                rest = tail;
            }
        }
    }
    Some(rest)
}

/// Verbraucht den Text eines Platzhalters und liefert ihn samt Rest.
fn take<'a>(kind: &CaptureKind, after: &[PatternPiece], rest: &'a str) -> Option<(&'a str, &'a str)> {
    match kind {
        // Klassengebundene Arten enden am ersten Zeichen ausserhalb der
        // Klasse; die Mehrdeutigkeitsregel garantiert, dass das die
        // gesuchte Grenze ist.
        CaptureKind::Int => signed(rest, is_int_char, 19),
        CaptureKind::Hex => hex(rest),
        CaptureKind::Float => float(rest),
        CaptureKind::Word => bounded(rest, is_word_char, 64),
        // `str<N>` endet beim ersten Vorkommen des Folgeliterals
        // (leftmost-shortest), hoechstens nach N Zeichen.
        CaptureKind::Str(n) => open_end(after, rest, Some(*n as usize)),
    }
}

/// Laengstes Praefix aus Zeichen der Klasse, hoechstens `max` Zeichen.
/// Ein leeres Praefix ist kein Match (jede Klasse verlangt `{1,…}`).
fn bounded(rest: &str, class: fn(char) -> bool, max: usize) -> Option<(&str, &str)> {
    let mut end = 0;
    for (n, (i, c)) in rest.char_indices().enumerate() {
        if !class(c) || n == max {
            break;
        }
        end = i + c.len_utf8();
    }
    if end == 0 { None } else { Some((&rest[..end], &rest[end..])) }
}

fn is_int_char(c: char) -> bool {
    c.is_ascii_digit()
}

/// `[+-]?[0-9]{1,19}` (8.7): das Vorzeichen zaehlt nicht zu den 19 Ziffern.
fn signed(rest: &str, class: fn(char) -> bool, max: usize) -> Option<(&str, &str)> {
    let sign = usize::from(rest.starts_with(['+', '-']));
    let (digits, tail) = bounded(&rest[sign..], class, max)?;
    Some((&rest[..sign + digits.len()], tail))
}

/// `(0x)?[0-9a-fA-F]{1,16}` (8.7).
fn hex(rest: &str) -> Option<(&str, &str)> {
    let prefix = if rest.starts_with("0x") { 2 } else { 0 };
    let (digits, tail) = bounded(&rest[prefix..], |c| c.is_ascii_hexdigit(), 16)?;
    Some((&rest[..prefix + digits.len()], tail))
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `[+-]?[0-9]+(.[0-9]+)?([eE][+-]?[0-9]+)?` (8.7).
fn float(rest: &str) -> Option<(&str, &str)> {
    let (head, mut tail) = signed(rest, is_int_char, usize::MAX)?;
    let mut end = head.len();
    if let Some(after_dot) = tail.strip_prefix('.') {
        if let Some((frac, t)) = bounded(after_dot, is_int_char, usize::MAX) {
            end += 1 + frac.len();
            tail = t;
        }
    }
    if tail.starts_with(['e', 'E']) {
        if let Some((exp, t)) = signed(&tail[1..], is_int_char, usize::MAX) {
            end += 1 + exp.len();
            tail = t;
        }
    }
    Some((&rest[..end], tail))
}

/// Offenes Ende (`str<N>`, `{_}`): endet beim ersten Vorkommen des naechsten
/// Literals, sonst am Textende (leftmost-shortest, 8.7). `max` begrenzt die
/// Zahl der Zeichen; `None` heisst unbegrenzt.
fn open_end<'a>(after: &[PatternPiece], rest: &'a str, max: Option<usize>) -> Option<(&'a str, &'a str)> {
    let limit = |end: usize| max.is_none_or(|n| rest[..end].chars().count() <= n);
    match after.iter().find_map(literal) {
        Some(lit) => {
            // Das kuerzeste Praefix, nach dem das Literal folgt.
            for (i, _) in char_starts(rest).map(|i| (i, ())) {
                if rest[i..].starts_with(lit) && limit(i) {
                    return Some((&rest[..i], &rest[i..]));
                }
            }
            None
        }
        None if limit(rest.len()) => Some((rest, "")),
        None => None,
    }
}

/// Das naechste literale Textstueck; `None`, wenn nur Platzhalter folgen.
fn literal(p: &PatternPiece) -> Option<&str> {
    match p {
        PatternPiece::Text(t) => Some(t.as_str()),
        _ => None,
    }
}

/// Wert eines Captures. Ein Ueberlauf bei `int`/`hex` und ein nicht endlicher
/// Wert bei `float` sind „kein Match", kein Fault (8.7).
fn value_of(kind: &CaptureKind, text: &str) -> Option<Value> {
    match kind {
        CaptureKind::Int => text.parse::<i64>().ok().map(Value::Int),
        CaptureKind::Hex => {
            let digits = text.strip_prefix("0x").unwrap_or(text);
            i64::from_str_radix(digits, 16).ok().map(Value::Int)
        }
        CaptureKind::Float => text.parse::<f64>().ok().filter(|f| f.is_finite()).map(Value::F64),
        CaptureKind::Word | CaptureKind::Str(_) => Some(Value::Str(text.to_string())),
    }
}

/// Text eines Werts fuer den Abgleich: `str<N>` und `line<N>` (3.9).
pub fn text_of(v: &Value) -> Option<&str> {
    match v {
        Value::Str(s) => Some(s.as_str()),
        Value::Line { text, .. } => Some(text.as_str()),
        _ => None,
    }
}

/// Gleicht ein Muster gegen einen Wert ab. Record-Muster vergleichen die
/// aufgefuehrten Felder mit ihren konstanten Werten (endliche Konjunktion
/// von Gleichheiten, total, 8.7).
pub fn match_value(
    pattern: &Pattern,
    kind: takt_mir::expr::MatchKind,
    v: &Value,
    consts: &[Value],
) -> Option<Captures> {
    match pattern {
        Pattern::Text { pieces, .. } => {
            let text = text_of(v)?;
            match kind {
                takt_mir::expr::MatchKind::Matches => match_text(pieces, text),
                takt_mir::expr::MatchKind::Has => match_has(pieces, text),
            }
        }
        Pattern::Record { fields, .. } => {
            let Value::Record(got) = v else { return None };
            for ((index, _), want) in fields.iter().zip(consts) {
                let field = got.get(*index as usize)?;
                if field != want {
                    return None;
                }
            }
            Some(Vec::new())
        }
    }
}
