//! Fuehrt die Testvektoren aus grammar/lexer.md aus.
//!
//! Notation (siehe lexer.md, Einleitung): `"eingabe" => TOKENS`, `!` markiert
//! Fehlervektoren, `~` zwischen Tokens verlangt das Flag *anliegend*.

use takt_syntax::subtext::{FormatPiece, PatternPiece, address_text, format_text, pattern_text};
use takt_syntax::{ErrorCode, TokenKind, tokenize};

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
            i += 2;
            while i < lines.len() && lines[i].trim() != "```" {
                if let Some(v) = parse_vector(&block, i + 1, lines[i]) {
                    vectors.push(v);
                }
                i += 1;
            }
        }
        i += 1;
    }
    assert!(vectors.len() > 50, "zu wenige Vektoren gefunden: {}", vectors.len());
    vectors
}

fn parse_vector(block: &str, line: usize, raw: &str) -> Option<Vector> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (negative, rest) = match raw.strip_prefix('!') {
        Some(r) => (true, r),
        None => (false, raw),
    };
    let rest = rest.strip_prefix('"')?;
    let (input, after) = decode_input(rest);
    let expected = after.trim_start().strip_prefix("=>").expect("=> fehlt").trim().to_string();
    Some(Vector { block: block.to_string(), line, negative, input, expected })
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

/// Ein erwartetes Element: Art, Text und ob das naechste Token anliegen muss.
struct Item {
    kind: String,
    text: Option<String>,
    joint_next: bool,
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
        if group == "~" {
            items.push(Item { kind: "OP".into(), text: Some("~".into()), joint_next: false });
            continue;
        }
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
                '~' if depth == 0 => pieces.push(std::mem::take(&mut current)),
                _ => current.push(c),
            }
        }
        pieces.push(current);
        let n = pieces.len();
        for (k, piece) in pieces.into_iter().enumerate() {
            let (kind, text) = match piece.find('(') {
                Some(p) if piece.ends_with(')') && piece[..p].bytes().all(|b| b.is_ascii_uppercase() || b == b'_') => {
                    (piece[..p].to_string(), Some(piece[p + 1..piece.len() - 1].to_string()))
                }
                _ if matches!(piece.as_str(), "NEWLINE" | "INDENT" | "DEDENT" | "WILD" | "ANY") => {
                    (piece.clone(), None)
                }
                _ => ("OP".to_string(), Some(piece.clone())),
            };
            items.push(Item { kind, text, joint_next: k + 1 < n });
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

fn error_name(code: ErrorCode) -> &'static str {
    code.as_str()
}

fn check_main(v: &Vector) -> Result<(), String> {
    let toks = tokenize(&v.input);
    if v.negative {
        let (want_code, want_detail) = match v.expected.split_once('(') {
            Some((c, d)) => (c, Some(d.trim_end_matches(')'))),
            None => (v.expected.as_str(), None),
        };
        let first = toks.errors.first().ok_or_else(|| format!("erwartet {want_code}, aber kein Fehler"))?;
        if error_name(first.code) != want_code {
            return Err(format!("erwartet {want_code}, erhalten {}", first));
        }
        if let Some(d) = want_detail {
            if first.detail != d {
                return Err(format!("erwartet {want_code}({d}), erhalten {}", first));
            }
        }
        return Ok(());
    }
    if let Some(e) = toks.errors.first() {
        return Err(format!("unerwarteter Fehler: {e}"));
    }
    let items = parse_items(&v.expected);
    let mut actual: Vec<_> = toks.tokens.iter().filter(|t| t.kind != TokenKind::Eof).collect();
    // Notation: Ohne Zeilenende in der Eingabe darf das NEWLINE aus L1.3 in der
    // Erwartung entfallen.
    let expects_newline = items.last().is_some_and(|it| it.kind == "NEWLINE");
    if !v.input.ends_with('\n') && !expects_newline && actual.last().is_some_and(|t| t.kind == TokenKind::Newline) {
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
        if item.joint_next && !t.joint {
            return Err(format!("{}({}) muss anliegen; Folge: {actual_shown}", kind, toks.text(t)));
        }
    }
    Ok(())
}

fn check_sub(v: &Vector) -> Result<(), String> {
    let actual: Result<Vec<String>, ErrorCode> = match v.block.as_str() {
        "vectors-format" => format_text(&v.input).map_err(|e| e.code).map(|ps| {
            ps.into_iter()
                .map(|p| match p {
                    FormatPiece::Text(t) => format!("TEXT({t})"),
                    FormatPiece::Expr(t) => format!("EXPR({t})"),
                    FormatPiece::Spec(t) => format!("SPEC({t})"),
                })
                .collect()
        }),
        "vectors-pattern" => pattern_text(&v.input).map_err(|e| e.code).map(|ps| {
            ps.into_iter()
                .map(|p| match p {
                    PatternPiece::Text(t) => format!("TEXT({t})"),
                    PatternPiece::Capture { name, kind } => format!("CAP({name}:{kind})"),
                    PatternPiece::Any => "ANY".to_string(),
                })
                .collect()
        }),
        "vectors-address" => address_text(&v.input).map_err(|e| e.code).map(|segs| {
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
        (true, Err(code)) if error_name(code) == v.expected => Ok(()),
        (true, Err(code)) => Err(format!("erwartet {}, erhalten {}", v.expected, error_name(code))),
        (true, Ok(pieces)) => Err(format!("erwartet {}, erhalten {}", v.expected, pieces.join(" "))),
        (false, Err(code)) => Err(format!("unerwarteter Fehler {}", error_name(code))),
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
