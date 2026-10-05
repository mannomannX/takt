//! Das Orakel des Musterabgleichs (8.7): `takt-match` mit der Umwandlung
//! der Werte, wie der Interpreter sie vornimmt. Geteilt von
//! `match_agreement.rs` und `dfa_agreement.rs`.

use takt_interp::pattern::{match_has, match_text};
use takt_interp::value::Value;
use takt_mir::pattern::{CaptureKind, PatternPiece};
use takt_mir::types::FloatWidth;

/// Dasselbe Muster in der Form von `takt-match`.
pub fn as_match(pieces: &[PatternPiece]) -> Vec<takt_match::Piece<'_>> {
    pieces
        .iter()
        .map(|p| match p {
            PatternPiece::Text(t) => takt_match::Piece::Text(t.as_bytes()),
            PatternPiece::Any => takt_match::Piece::Capture(takt_match::Kind::Any),
            PatternPiece::Capture { kind, .. } => takt_match::Piece::Capture(match kind {
                CaptureKind::Int => takt_match::Kind::Int,
                CaptureKind::Hex => takt_match::Kind::Hex,
                CaptureKind::Float => takt_match::Kind::Float,
                CaptureKind::Word => takt_match::Kind::Word,
                CaptureKind::Str(n) => takt_match::Kind::Str(*n),
            }),
        })
        .collect()
}

/// Die Werte eines Treffers von `takt-match`: Jeder folgt aus dem Text
/// seines Bereichs. Ein Wert, der sich nicht lesen laesst (Ueberlauf, nicht
/// endlich), ist kein Treffer (8.7).
pub fn values_of(pieces: &[PatternPiece], m: &takt_match::Match, text: &[u8]) -> Option<Vec<Value>> {
    let kinds: Vec<&CaptureKind> = pieces
        .iter()
        .filter_map(|p| match p {
            PatternPiece::Capture { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    (0..m.count as usize)
        .map(|n| {
            let raw = m.spans[n].of(text);
            let s = core::str::from_utf8(raw).ok()?;
            match kinds.get(n)? {
                CaptureKind::Int => takt_match::parse_int(raw).map(Value::Int),
                CaptureKind::Hex => takt_match::parse_hex(raw).map(Value::Int),
                CaptureKind::Float => s.parse::<f64>().ok().filter(|f| f.is_finite()).map(Value::F64),
                CaptureKind::Word | CaptureKind::Str(_) => Some(Value::Str(s.to_string())),
            }
        })
        .collect()
}

/// `has` ueber `takt-match`, wie 8.7 es definiert: die frueheste Stelle an
/// einem Zeichenanfang, an der Durchlauf und Werte gelingen. Der Rest der
/// Zeile ist ein `{_}`, das nichts bindet.
pub fn has_by_oracle(pieces: &[PatternPiece], line: &str) -> Option<Vec<Value>> {
    let mut mp = as_match(pieces);
    mp.push(takt_match::Piece::Capture(takt_match::Kind::Any));
    let starts = line.char_indices().map(|(i, _)| i).chain([line.len()]);
    starts.into_iter().find_map(|s| {
        let rest = &line.as_bytes()[s..];
        takt_match::matches(&mp, rest).and_then(|m| values_of(pieces, &m, rest))
    })
}

/// Fliesskomma bitweise: -0.0 ist nicht 0.0.
pub fn bits(v: &Option<Vec<Value>>) -> Option<Vec<String>> {
    v.as_ref().map(|caps| {
        caps.iter()
            .map(|c| match c {
                Value::F64(f) => format!("f{:x}", f.to_bits()),
                other => format!("{other:?}"),
            })
            .collect()
    })
}

/// Vergleicht Urteil und Werte einer Zeile, fuer `matches` oder `has`.
pub fn agree(pieces: &[PatternPiece], line: &str, has: bool) {
    let interp = if has { match_has(pieces, line, FloatWidth::F64) } else { match_text(pieces, line, FloatWidth::F64) };
    let oracle = if has {
        has_by_oracle(pieces, line)
    } else {
        takt_match::matches(&as_match(pieces), line.as_bytes()).and_then(|m| values_of(pieces, &m, line.as_bytes()))
    };
    assert_eq!(bits(&interp), bits(&oracle), "`{line}` (has {has}): {pieces:?}");
    // `takt_match::has` selbst: Lassen sich die Werte seines Treffers
    // lesen, ist er der des Interpreters.
    if has {
        let own =
            takt_match::has(&as_match(pieces), line.as_bytes()).and_then(|m| values_of(pieces, &m, line.as_bytes()));
        if own.is_some() {
            assert_eq!(bits(&interp), bits(&own), "`{line}`: takt_match::has ({pieces:?})");
        }
    }
}
