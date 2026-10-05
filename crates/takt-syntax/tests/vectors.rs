//! Fuehrt die Testvektoren aus grammar/lexer.md aus.
//!
//! Notation (siehe lexer.md, Einleitung): `"eingabe" => TOKENS`, `!` markiert
//! Fehlervektoren, `~` zwischen Tokens verlangt das Flag *anliegend*, `_` das
//! Gegenteil; `@zeile:spalte` hinter einem Token oder Fehlercode verlangt dessen
//! Stelle. Ein Fehlervektor nennt alle Fehler der Eingabe in Reihenfolge, auf
//! Wunsch gefolgt von `=> TOKENS`, der Folge trotz der Fehler (L9).

use takt_syntax::subtext::{FormatPiece, PatternPiece, address_text, format_text, pattern_text};
use takt_syntax::{SourceMap, TokenKind, Tokens, tokenize};

struct Vector {
    block: String,
    line: usize,
    negative: bool,
    input: String,
    expected: String,
}

fn load() -> Vec<Vector> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/lexer.md");
    let text = std::fs::read_to_string(path).expect("grammar/lexer.md lesbar");
    let lines: Vec<&str> = text.lines().collect();
    let mut vectors = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "```" && i + 1 < lines.len() && lines[i + 1].starts_with("vectors") {
            let block = lines[i + 1].trim().to_string();
            let opened = i + 2;
            let before = vectors.len();
            i += 2;
            while i < lines.len() && lines[i].trim() != "```" {
                if let Some(v) = parse_vector(&block, i + 1, lines[i]) {
                    vectors.push(v);
                }
                i += 1;
            }
            assert!(vectors.len() > before, "lexer.md:{opened}: Block {block} ohne Vektor");
        }
        i += 1;
    }
    assert!(vectors.len() > 50, "zu wenige Vektoren gefunden: {}", vectors.len());
    vectors
}

/// Liest eine Zeile eines Vektorblocks. Jede nicht leere Zeile muss ein Vektor
/// sein: Ein Tippfehler soll scheitern, nicht still einen Vektor loeschen.
fn parse_vector(block: &str, line: usize, raw: &str) -> Option<Vector> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (negative, rest) = match raw.strip_prefix('!') {
        Some(r) => (true, r),
        None => (false, raw),
    };
    let rest = rest.strip_prefix('"').unwrap_or_else(|| panic!("lexer.md:{line}: keine Vektorzeile: {raw}"));
    let (input, after) = decode_input(rest);
    let expected = after.trim_start().strip_prefix("=>").unwrap_or_else(|| panic!("lexer.md:{line}: => fehlt"));
    Some(Vector { block: block.to_string(), line, negative, input, expected: expected.trim().to_string() })
}

/// Liest den Eingabetext bis zum schliessenden Anfuehrungszeichen und loest die
/// Vektor-Escapes auf. Gibt (Eingabe, Rest hinter dem Anfuehrungszeichen).
fn decode_input(s: &str) -> (String, &str) {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => return (decode_escapes(&s[..i]), &s[i + 1..]),
            b'\\' => i += 2,
            _ => i += 1,
        }
    }
    panic!("Vektor-Eingabe ohne schliessendes Anfuehrungszeichen: {s}");
}

/// Loest die Vektor-Escapes `\n \t \r \" \\ \xHH` auf; ein nacktes `"` bleibt.
fn decode_escapes(s: &str) -> String {
    let mut bytes = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            bytes.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        match b[i] {
            b'n' => bytes.push(b'\n'),
            b't' => bytes.push(b'\t'),
            b'r' => bytes.push(b'\r'),
            b'"' => bytes.push(b'"'),
            b'\\' => bytes.push(b'\\'),
            b'x' => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).expect("Hex");
                bytes.push(u8::from_str_radix(hex, 16).expect("Hex-Escape"));
                i += 2;
            }
            other => panic!("unbekanntes Escape \\{}", other as char),
        }
        i += 1;
    }
    String::from_utf8(bytes).expect("Vektor-Text ist UTF-8")
}

/// Loest die Vektor-Escapes in einem erwarteten Text (z. B. `STRING(a\nb)`).
fn decode_expected_text(s: &str) -> String {
    decode_escapes(s)
}

/// Ein erwartetes Element: Art, Text, ob das naechste Token anliegen muss
/// (`~` ja, `_` nein, sonst ungeprueft) und die Stelle.
struct Item {
    kind: String,
    text: Option<String>,
    joint_next: Option<bool>,
    at: Option<(u32, u32)>,
}

/// Trennt eine Stelle `@zeile:spalte` vom Ende eines Elements ab. Ein `@`
/// in Klammern (`STRING(a@b)`) und der Operator `@` selbst bleiben.
fn split_at(piece: &str) -> (&str, Option<(u32, u32)>) {
    if let Some(i) = piece.rfind('@')
        && i > 0
        && (piece[..i].ends_with(')') || !piece[..i].contains('('))
        && let Some((l, c)) = piece[i + 1..].split_once(':')
        && let (Ok(l), Ok(c)) = (l.parse(), c.parse())
    {
        return (&piece[..i], Some((l, c)));
    }
    (piece, None)
}

/// Zerlegt die Erwartung an Leerraum, aber nicht innerhalb von Klammern
/// (`STRING(a # b)` bleibt ein Element).
fn split_groups(expected: &str) -> Vec<String> {
    let mut groups = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let is_kind = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_uppercase() || b == b'_');
    for c in expected.chars() {
        match c {
            // "(" oeffnet nur direkt nach einer Tokenart (KIND(...)); sonst ist es
            // das Interpunktionstoken "(".
            '(' if depth > 0 || is_kind(&current) => {
                depth += 1;
                current.push(c);
            }
            ')' if depth > 0 => {
                depth -= 1;
                current.push(c);
            }
            ' ' | '\t' if depth == 0 => {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

fn parse_items(expected: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for group in split_groups(expected) {
        let group = group.as_str();
        if let ("~", at) = split_at(group) {
            items.push(Item { kind: "OP".into(), text: Some("~".into()), joint_next: None, at });
            continue;
        }
        // Stuecke mit dem Zeichen, das auf sie folgt (`~` oder `_`).
        let mut pieces = Vec::new();
        let mut depth = 0;
        let mut current = String::new();
        for c in group.chars() {
            match c {
                '(' => {
                    depth += 1;
                    current.push(c);
                }
                ')' => {
                    depth -= 1;
                    current.push(c);
                }
                '~' | '_' if depth == 0 => pieces.push((std::mem::take(&mut current), Some(c == '~'))),
                _ => current.push(c),
            }
        }
        pieces.push((current, None));
        for (piece, joint_next) in pieces {
            let (piece, at) = split_at(&piece);
            let (kind, text) = match piece.find('(') {
                Some(p) if piece.ends_with(')') && piece[..p].bytes().all(|b| b.is_ascii_uppercase() || b == b'_') => {
                    (piece[..p].to_string(), Some(piece[p + 1..piece.len() - 1].to_string()))
                }
                _ if matches!(piece, "NEWLINE" | "INDENT" | "DEDENT" | "WILD" | "ANY") => (piece.to_string(), None),
                _ => ("OP".to_string(), Some(piece.to_string())),
            };
            items.push(Item { kind, text, joint_next, at });
        }
    }
    items
}

fn kind_name(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Newline => "NEWLINE",
        TokenKind::Indent => "INDENT",
        TokenKind::Dedent => "DEDENT",
        TokenKind::Ident => "IDENT",
        TokenKind::UpperIdent => "UPPER",
        TokenKind::TypeIdent => "TYPE",
        TokenKind::Keyword => "KW",
        TokenKind::Reserved => "RESERVED",
        TokenKind::Int => "INT",
        TokenKind::Hex => "HEX",
        TokenKind::Bin => "BIN",
        TokenKind::Oct => "OCT",
        TokenKind::Float => "FLOAT",
        TokenKind::Duration => "DUR",
        TokenKind::Str => "STRING",
        TokenKind::Wild => "WILD",
        TokenKind::Op => "OP",
        TokenKind::Error => "ERROR",
        TokenKind::Eof => "EOF",
    }
}

fn check_main(v: &Vector) -> Result<(), String> {
    let toks = tokenize(&v.input);
    let map = SourceMap::single("", v.input.as_str());
    let found = || toks.errors.iter().map(|e| format!("{}@{:?}", e.code, map.line_col(e.span))).collect::<Vec<_>>();
    // L9: Jeder Fehler nennt einen Vorschlag.
    if let Some(e) = toks.errors.iter().find(|e| e.suggestion.is_none()) {
        return Err(format!("Fehler ohne Vorschlag: {e}"));
    }
    if !v.negative {
        if let Some(e) = toks.errors.first() {
            return Err(format!("unerwarteter Fehler: {e}"));
        }
        return check_tokens(&toks, &v.input, &v.expected);
    }
    let (errors, rest) = match v.expected.split_once("=>") {
        Some((e, r)) => (e.trim(), Some(r.trim())),
        None => (v.expected.as_str(), None),
    };
    let wanted = split_groups(errors);
    if wanted.len() != toks.errors.len() {
        return Err(format!("{} Fehler erwartet, erhalten {:?}", wanted.len(), found()));
    }
    for (want, got) in wanted.iter().zip(&toks.errors) {
        let (want, at) = split_at(want);
        let (code, detail) = match want.split_once('(') {
            Some((c, d)) => (c, Some(d.trim_end_matches(')'))),
            None => (want, None),
        };
        let place = map.line_col(got.span);
        if got.code != code || detail.is_some_and(|d| !got.message.contains(d)) || at.is_some_and(|a| a != place) {
            return Err(format!("erwartet {want} {at:?}, erhalten {got} {place:?}"));
        }
    }
    match rest {
        Some(rest) => check_tokens(&toks, &v.input, rest),
        None => Ok(()),
    }
}

/// Vergleicht die Tokenfolge (ohne `Eof`) mit der Erwartung.
fn check_tokens(toks: &Tokens<'_>, input: &str, expected: &str) -> Result<(), String> {
    let items = parse_items(expected);
    let mut actual: Vec<_> = toks.tokens.iter().filter(|t| t.kind != TokenKind::Eof).collect();
    // Notation: Ohne Zeilenende in der Eingabe darf das NEWLINE aus L1.3 in der
    // Erwartung entfallen.
    let expects_newline = items.last().is_some_and(|it| it.kind == "NEWLINE");
    if !input.ends_with('\n') && !expects_newline && actual.last().is_some_and(|t| t.kind == TokenKind::Newline) {
        actual.pop();
    }
    let show = |list: Vec<String>| list.join(" ");
    let actual_shown = show(
        actual
            .iter()
            .map(|t| match t.kind {
                TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Wild => {
                    kind_name(t.kind).to_string()
                }
                TokenKind::Op => toks.text(t).to_string(),
                TokenKind::Duration => format!("DUR({})", t.value),
                TokenKind::Str => format!("STRING({})", toks.unescape(t).escape_debug()),
                _ => format!("{}({})", kind_name(t.kind), toks.text(t)),
            })
            .collect(),
    );
    if actual.len() != items.len() {
        return Err(format!("{} Tokens erwartet, {} erhalten: {actual_shown}", items.len(), actual.len()));
    }
    for (t, item) in actual.iter().zip(&items) {
        let kind = kind_name(t.kind);
        let text_ok = match (item.kind.as_str(), item.text.as_deref()) {
            ("STRING", Some(want)) => toks.unescape(t) == decode_expected_text(want),
            ("DUR", Some(want)) => t.value.to_string() == want,
            (_, Some(want)) => toks.text(t) == want,
            (_, None) => true,
        };
        if kind != item.kind || !text_ok {
            return Err(format!(
                "erwartet {}{}, erhalten {}({}); Folge: {actual_shown}",
                item.kind,
                item.text.as_ref().map(|s| format!("({s})")).unwrap_or_default(),
                kind,
                toks.text(t)
            ));
        }
        if item.joint_next.is_some_and(|j| j != t.joint) {
            let how = if t.joint { "darf nicht anliegen" } else { "muss anliegen" };
            return Err(format!("{}({}) {how}; Folge: {actual_shown}", kind, toks.text(t)));
        }
        if item.at.is_some_and(|at| at != (t.line, t.col)) {
            return Err(format!("{}({}) steht an {}:{}; Folge: {actual_shown}", kind, toks.text(t), t.line, t.col));
        }
    }
    Ok(())
}

fn check_sub(v: &Vector) -> Result<(), String> {
    // Fehler als `CODE@spalte`, die Spalte ab 1 im Stringinhalt.
    let shown = |e: takt_syntax::Diagnostic| {
        assert!(e.suggestion.is_some(), "Fehler ohne Vorschlag: {e}");
        format!("{}@{}", e.code, e.span.start + 1)
    };
    let actual: Result<Vec<String>, String> = match v.block.as_str() {
        "vectors-format" => format_text(&v.input).map_err(shown).map(|ps| {
            ps.into_iter()
                .map(|p| match p {
                    FormatPiece::Text(t) => format!("TEXT({t})"),
                    FormatPiece::Expr(t, _) => format!("EXPR({t})"),
                    FormatPiece::Spec(t) => format!("SPEC({t})"),
                })
                .collect()
        }),
        "vectors-pattern" => pattern_text(&v.input).map_err(shown).map(|ps| {
            ps.into_iter()
                .map(|p| match p {
                    PatternPiece::Text(t) => format!("TEXT({t})"),
                    PatternPiece::Capture { name, kind } => format!("CAP({name}:{kind})"),
                    PatternPiece::Any => "ANY".to_string(),
                })
                .collect()
        }),
        "vectors-address" => address_text(&v.input).map_err(shown).map(|segs| {
            let mut out = Vec::new();
            for s in segs {
                out.push(format!("SEG({})", s.name));
                if let Some((a, b)) = s.range {
                    out.push(format!("RANGE({a}:{b})"));
                }
            }
            out
        }),
        other => return Err(format!("unbekannter Block {other}")),
    };
    match (v.negative, actual) {
        // Ohne `@spalte` in der Erwartung zaehlt nur der Code.
        (true, Err(got)) if got == v.expected || got.split('@').next() == Some(v.expected.as_str()) => Ok(()),
        (true, Err(code)) => Err(format!("erwartet {}, erhalten {}", v.expected, code)),
        (true, Ok(pieces)) => Err(format!("erwartet {}, erhalten {}", v.expected, pieces.join(" "))),
        (false, Err(code)) => Err(format!("unerwarteter Fehler {}", code)),
        (false, Ok(pieces)) => {
            let want: Vec<String> = parse_items(&v.expected)
                .into_iter()
                .map(|it| match it.text {
                    Some(t) => format!("{}({t})", it.kind),
                    None => it.kind,
                })
                .collect();
            if pieces == want {
                Ok(())
            } else {
                Err(format!("erwartet {}, erhalten {}", want.join(" "), pieces.join(" ")))
            }
        }
    }
}

#[test]
fn all_vectors_pass() {
    let vectors = load();
    let mut failures = Vec::new();
    for v in &vectors {
        let result = if v.block == "vectors" { check_main(v) } else { check_sub(v) };
        if let Err(msg) = result {
            failures.push(format!("lexer.md:{} [{}] {:?}: {}", v.line, v.block, v.input, msg));
        }
    }
    assert!(
        failures.is_empty(),
        "{} von {} Vektoren scheitern:\n{}",
        failures.len(),
        vectors.len(),
        failures.join("\n")
    );
}
