//! Tokenizer nach `grammar/lexer.md` (Abschnitte L1 bis L6, L8, L9).
//!
//! Zeilenweise: Einrueckung (L2) wird vor dem Zeileninhalt bestimmt, innerhalb
//! offener Klammern entfallen NEWLINE, INDENT und DEDENT. Jeder Fehler wird
//! gemeldet und die Analyse fortgesetzt (L9).

use takt_diag::{Diagnostic, Span};

use crate::edition::Edition;
use crate::token::{ErrorCode, Token, TokenKind, Tokens, Trivia, TriviaKind, lex_diagnostic};

/// Zeitsuffixe und ihr Faktor in Nanosekunden (L4.4).
const TIME: &[(&str, i128)] = &[
    ("ns", 1),
    ("us", 1_000),
    ("ms", 1_000_000),
    ("s", 1_000_000_000),
    ("min", 60_000_000_000),
    ("h", 3_600_000_000_000),
    ("d", 86_400_000_000_000),
];

/// Zweizeichen-Operatoren; `>>` fehlt absichtlich (L6.1).
const OPS2: &[[u8; 2]] = &[*b"->", *b"..", *b"+=", *b"-=", *b"*=", *b"/=", *b"==", *b"!=", *b"<=", *b">=", *b"<<"];
/// Zeichen, die eine Zeile nur fortsetzen koennen: Infix-Operatoren und der
/// Feldzugriff. `-` und `~` fehlen, weil sie auch Vorzeichen sind.
const INFIX1: &[u8] = b".+*/%<>|&^";
/// Zweizeichen-Operatoren in derselben Rolle.
const INFIX2: &[[u8; 2]] = &[*b"==", *b"!=", *b"<=", *b">=", *b"<<", *b">>"];

const OPS1: &[u8] = b"+-*/%&|^~<>=.,:()[]{}@?!";

/// Zerlegt Quelltext in Tokens mit dem Wortschatz der neuesten Edition. Fehler
/// stehen in `Tokens::errors`; das letzte Token ist immer `Eof`.
pub fn tokenize(src: &str) -> Tokens<'_> {
    tokenize_in(src, Edition::LATEST)
}

/// Zerlegt Quelltext mit dem Wortschatz einer Edition (2.5; `edition::declared_edition`
/// liest sie vorab aus dem `system:`-Block).
pub fn tokenize_in(src: &str, edition: Edition) -> Tokens<'_> {
    let mut lexer = Lexer {
        src,
        bytes: src.as_bytes(),
        tokens: Vec::new(),
        trivia: Vec::new(),
        pending: 0,
        errors: Vec::new(),
        edition,
        stack: vec![0],
        depth: 0,
        line: 0,
        line_start: 0,
    };
    lexer.run();
    Tokens { src, tokens: lexer.tokens, trivia: lexer.trivia, errors: lexer.errors }
}

struct Lexer<'s> {
    src: &'s str,
    bytes: &'s [u8],
    tokens: Vec<Token>,
    trivia: Vec<Trivia>,
    pending: u32,
    errors: Vec<Diagnostic>,
    edition: Edition,
    stack: Vec<u32>,
    depth: u32,
    line: u32,
    line_start: usize,
}

impl Lexer<'_> {
    fn error(&mut self, code: ErrorCode, at: usize, detail: impl Into<String>) {
        let detail = detail.into();
        let at = at.min(self.src.len());
        let end = if !detail.is_empty() && self.src[at..].starts_with(&detail) { at + detail.len() } else { at + 1 };
        self.errors.push(lex_diagnostic(code, Span::new(at as u32, end.min(self.src.len()).max(at) as u32), &detail));
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize, value: i64, content_end: usize) {
        let joint = end < content_end && !matches!(self.bytes[end], b' ' | b'\t' | b'#');
        let trivia = (self.pending, self.trivia.len() as u32);
        self.pending = self.trivia.len() as u32;
        self.tokens.push(Token {
            kind,
            start: start as u32,
            end: end as u32,
            line: self.line.max(1),
            col: (start.saturating_sub(self.line_start) + 1) as u32,
            joint,
            trivia,
            value,
        });
    }

    fn add_trivia(&mut self, kind: TriviaKind, start: usize, end: usize) {
        self.trivia.push(Trivia { kind, start: start as u32, end: end as u32 });
    }

    fn run(&mut self) {
        let n = self.bytes.len();
        let mut pos = 0;
        if self.bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            self.line = 1;
            self.error(ErrorCode::Bom, 0, "");
            pos = 3;
        }
        while pos < n {
            self.line += 1;
            self.line_start = pos;
            let mut end = pos;
            while end < n && self.bytes[end] != b'\n' {
                end += 1;
            }
            let mut content_end = end;
            // Nur "\r\n" ist ein Zeilenende; ein "\r" am Dateiende ist einzeln (L1.2).
            if end < n && content_end > pos && self.bytes[content_end - 1] == b'\r' {
                content_end -= 1;
            }
            self.scan_line(pos, content_end);
            pos = if end < n { end + 1 } else { end };
        }
        if self.depth > 0 {
            self.error(ErrorCode::Unclosed, self.line_start, "");
        }
        self.line_start = n;
        let open = self.stack.len() - 1;
        self.stack.truncate(1);
        for _ in 0..open {
            self.push(TokenKind::Dedent, n, n, 0, n);
        }
        self.push(TokenKind::Eof, n, n, 0, n);
    }

    fn scan_line(&mut self, line_start: usize, content_end: usize) {
        let b = self.bytes;
        let mut p = line_start;
        let mut spaces = 0u32;
        let mut tab_at = None;
        while p < content_end && (b[p] == b' ' || b[p] == b'\t') {
            if b[p] == b'\t' {
                tab_at.get_or_insert(p);
            } else {
                spaces += 1;
            }
            p += 1;
        }
        if p >= content_end {
            self.add_trivia(TriviaKind::BlankLine, line_start, content_end);
            return;
        }
        if b[p] == b'#' {
            self.add_trivia(TriviaKind::Comment, p, content_end);
            return;
        }
        if let Some(at) = tab_at {
            self.error(ErrorCode::Tab, at, "");
        }
        // L2.2: eine Zeile hinter einem haengenden Trennzeichen und eine
        // Zeile, die mit einem verbindenden Zeichen beginnt, setzen die
        // vorige fort. Beides ist an einem Token entscheidbar und kann keine
        // heute gueltige Zeile umdeuten: eine vollstaendige Zeile endet nie
        // auf `,` und beginnt nie mit einem Infix-Zeichen.
        let continued = self.depth == 0 && (self.after_comma() || self.starts_continuation(p, content_end));
        if self.depth == 0 && !continued {
            self.indentation(spaces, line_start);
        }
        if continued {
            // Das `NEWLINE` der vorigen Zeile war verfrueht.
            self.drop_trailing_newline();
        }
        while p < content_end {
            let c = b[p];
            p = match c {
                b' ' => p + 1,
                b'\t' => {
                    self.error(ErrorCode::Tab, p, "");
                    p + 1
                }
                b'\r' => {
                    self.error(ErrorCode::Cr, p, "");
                    p + 1
                }
                b'#' => {
                    self.add_trivia(TriviaKind::Comment, p, content_end);
                    break;
                }
                b'"' => self.scan_string(p, content_end),
                b'0'..=b'9' => self.scan_number(p, content_end),
                _ if c.is_ascii_alphabetic() || c == b'_' => self.scan_word(p, content_end),
                _ if c >= 0x80 => {
                    let len = utf8_len(c).min(content_end - p);
                    let shown = String::from_utf8_lossy(&b[p..p + len]).into_owned();
                    self.error(ErrorCode::NonAscii, p, shown);
                    p + len
                }
                _ => self.scan_op(p, content_end),
            };
        }
        if self.depth == 0 {
            self.push(TokenKind::Newline, content_end, content_end, 0, content_end);
        }
    }

    /// Beginnt die Zeile mit einem Zeichen, das nur *zwischen* zwei
    /// Operanden stehen kann? `-` und `~` sind ausgenommen, weil sie auch
    /// Vorzeichen sind (2.3, `unary`) und eine neue Zeile eroeffnen koennen.
    fn starts_continuation(&self, p: usize, content_end: usize) -> bool {
        // Vor der ersten Zeile gibt es nichts fortzusetzen.
        if !self.tokens.iter().any(|t| !t.kind.is_layout()) {
            return false;
        }
        let b = self.bytes;
        let two = if p + 1 < content_end { Some([b[p], b[p + 1]]) } else { None };
        if two.is_some_and(|x| INFIX2.contains(&x)) {
            return true;
        }
        if INFIX1.contains(&b[p]) {
            // `.` vor einer Ziffer ist ein Zahlfehler, keine Fortsetzung.
            return b[p] != b'.' || p + 1 >= content_end || !b[p + 1].is_ascii_digit();
        }
        // Wortoperatoren und das anhaengende `with` einer Deklaration.
        let mut e = p;
        while e < content_end && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
            e += 1;
        }
        matches!(&self.src[p..e], "and" | "or" | "with")
    }

    /// Endet die bisherige Tokenfolge auf einem haengenden Trennzeichen?
    /// Das `NEWLINE` dahinter zaehlt nicht, es wird gleich zurueckgenommen.
    fn after_comma(&self) -> bool {
        let mut it = self.tokens.iter().rev();
        if !matches!(it.next().map(|t| t.kind), Some(TokenKind::Newline)) {
            return false;
        }
        it.next().is_some_and(|t| t.kind == TokenKind::Op && &self.src[t.start as usize..t.end as usize] == ",")
    }

    /// Nimmt ein gerade erzeugtes `NEWLINE` zurueck, wenn die naechste Zeile
    /// die logische Zeile fortsetzt.
    fn drop_trailing_newline(&mut self) {
        if self.tokens.last().is_some_and(|t| t.kind == TokenKind::Newline) {
            let dropped = self.tokens.pop().expect("gerade geprueft");
            // Die Trivia des zurueckgenommenen Tokens gehoeren an das
            // naechste, damit kein Kommentar verlorengeht.
            self.pending = dropped.trivia.0;
        }
    }

    fn indentation(&mut self, spaces: u32, at: usize) {
        let top = *self.stack.last().expect("Stapel hat immer die 0");
        if spaces > top {
            if spaces % 4 != 0 || spaces != top + 4 {
                self.error(ErrorCode::Indent, at, format!("von {top} auf {spaces}"));
            }
            self.stack.push(spaces);
            self.push(TokenKind::Indent, at, at, 0, at);
        } else if spaces < top {
            while spaces < *self.stack.last().expect("Stapel hat immer die 0") {
                self.stack.pop();
                self.push(TokenKind::Dedent, at, at, 0, at);
            }
            if spaces != *self.stack.last().expect("Stapel hat immer die 0") {
                self.error(ErrorCode::Dedent, at, format!("Einrueckung {spaces}"));
                self.stack.push(spaces);
            }
        }
    }

    fn scan_string(&mut self, start: usize, content_end: usize) -> usize {
        let b = self.bytes;
        let mut q = start + 1;
        loop {
            if q >= content_end {
                self.error(ErrorCode::String, start, "");
                self.push(TokenKind::Error, start, content_end, 0, content_end);
                return content_end;
            }
            match b[q] {
                b'"' => {
                    q += 1;
                    break;
                }
                b'\\' => {
                    if q + 1 >= content_end || !matches!(b[q + 1], b'\\' | b'"' | b'n' | b't' | b'r' | b'0') {
                        self.error(ErrorCode::Escape, q, "");
                        q += 1;
                    } else {
                        q += 2;
                    }
                }
                _ => q += 1,
            }
        }
        self.push(TokenKind::Str, start, q, 0, content_end);
        q
    }

    fn scan_number(&mut self, start: usize, content_end: usize) -> usize {
        let b = self.bytes;
        let mut q = start;
        let mut kind = TokenKind::Int;
        if b[q] == b'0' && q + 1 < content_end && matches!(b[q + 1], b'x' | b'b' | b'o') {
            let base = b[q + 1];
            kind = match base {
                b'x' => TokenKind::Hex,
                b'b' => TokenKind::Bin,
                _ => TokenKind::Oct,
            };
            q += 2;
            if q < content_end && b[q] == b'_' {
                self.error(ErrorCode::Number, q, "Unterstrich direkt nach dem Praefix");
            }
            let digits = q;
            while q < content_end && (digit_of(base, b[q]) || b[q] == b'_') {
                q += 1;
            }
            if q == digits {
                self.error(ErrorCode::Number, start, "Ziffern fehlen");
            }
        } else {
            while q < content_end && (b[q].is_ascii_digit() || b[q] == b'_') {
                q += 1;
            }
            if q < content_end && b[q] == b'.' {
                if q + 1 < content_end && b[q + 1].is_ascii_digit() {
                    kind = TokenKind::Float;
                    q += 1;
                    while q < content_end && (b[q].is_ascii_digit() || b[q] == b'_') {
                        q += 1;
                    }
                } else if !(q + 1 < content_end && b[q + 1] == b'.') {
                    self.error(ErrorCode::Number, q, "Ziffer nach dem Punkt fehlt");
                }
            }
            if q < content_end && (b[q] == b'e' || b[q] == b'E') {
                let mut r = q + 1;
                if r < content_end && (b[r] == b'+' || b[r] == b'-') {
                    r += 1;
                }
                if r < content_end && b[r].is_ascii_digit() {
                    kind = TokenKind::Float;
                    q = r;
                    while q < content_end && b[q].is_ascii_digit() {
                        q += 1;
                    }
                } else {
                    self.error(ErrorCode::Number, q, "Exponent ohne Ziffern");
                    q = r;
                }
            }
        }
        if b[q - 1] == b'_' {
            self.error(ErrorCode::Number, q - 1, "Unterstrich am Ende");
        }
        if q < content_end {
            let c = b[q];
            if c.is_ascii_alphabetic() {
                if kind == TokenKind::Int && q - start == 1 && b[start] == b'0' && matches!(c, b'X' | b'B' | b'O') {
                    self.error(ErrorCode::Number, q, "Praefix klein schreiben: 0x 0b 0o");
                } else {
                    self.error(ErrorCode::UnitSpace, q, "");
                }
            } else if c.is_ascii_digit() || c == b'_' {
                self.error(ErrorCode::Number, q, "");
            }
        }
        // Dauer: Zahl, Leerraum, genau ein Zeitsuffix, danach kein * / ^ (L4.4)
        if matches!(kind, TokenKind::Int | TokenKind::Float) {
            let mut r = q;
            while r < content_end && b[r] == b' ' {
                r += 1;
            }
            if r > q {
                let word_start = r;
                while r < content_end && b[r].is_ascii_lowercase() {
                    r += 1;
                }
                let word = &self.src[word_start..r];
                if let Some(&(_, factor)) = TIME.iter().find(|(name, _)| *name == word) {
                    let follows = r < content_end
                        && (matches!(b[r], b'*' | b'/' | b'^') || b[r].is_ascii_alphanumeric() || b[r] == b'_');
                    if !follows {
                        match duration_ns(&self.src[start..q], factor) {
                            Some(value) => self.push(TokenKind::Duration, start, r, value, content_end),
                            None => {
                                let shown = format!("{} {}", &self.src[start..q], word);
                                self.error(ErrorCode::Duration, start, shown);
                                self.push(TokenKind::Error, start, r, 0, content_end);
                            }
                        }
                        return r;
                    }
                }
            }
        }
        self.push(kind, start, q, 0, content_end);
        q
    }

    fn scan_word(&mut self, start: usize, content_end: usize) -> usize {
        let b = self.bytes;
        let mut q = start;
        while q < content_end && (b[q].is_ascii_alphanumeric() || b[q] == b'_') {
            q += 1;
        }
        let word = &self.src[start..q];
        let kind = if word == "_" {
            TokenKind::Wild
        } else if self.edition.is_keyword(word) {
            TokenKind::Keyword
        } else if self.edition.is_reserved(word) {
            TokenKind::Reserved
        } else if b[start].is_ascii_lowercase() || b[start] == b'_' {
            TokenKind::Ident
        } else if word.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_') {
            TokenKind::UpperIdent
        } else {
            TokenKind::TypeIdent
        };
        if kind == TokenKind::Reserved {
            self.error(ErrorCode::Reserved, start, word.to_string());
        }
        self.push(kind, start, q, 0, content_end);
        q
    }

    fn scan_op(&mut self, p: usize, content_end: usize) -> usize {
        let b = self.bytes;
        if p + 1 < content_end {
            let pair = [b[p], b[p + 1]];
            if pair == *b"=>" {
                self.error(ErrorCode::Char, p, "=>");
                self.push(TokenKind::Error, p, p + 2, 0, content_end);
                return p + 2;
            }
            if OPS2.contains(&pair) {
                self.push(TokenKind::Op, p, p + 2, 0, content_end);
                return p + 2;
            }
        }
        let c = b[p];
        if c == b'.' && p + 1 < content_end && b[p + 1].is_ascii_digit() {
            self.error(ErrorCode::Number, p, "Ziffer vor dem Punkt fehlt");
            self.push(TokenKind::Error, p, p + 1, 0, content_end);
            return p + 1;
        }
        if OPS1.contains(&c) {
            match c {
                b'(' | b'[' | b'{' => self.depth += 1,
                b')' | b']' | b'}' => self.depth = self.depth.saturating_sub(1),
                _ => {}
            }
            self.push(TokenKind::Op, p, p + 1, 0, content_end);
            return p + 1;
        }
        self.error(ErrorCode::Char, p, (c as char).to_string());
        self.push(TokenKind::Error, p, p + 1, 0, content_end);
        p + 1
    }
}

fn digit_of(base: u8, c: u8) -> bool {
    match base {
        b'x' => c.is_ascii_hexdigit(),
        b'b' => c == b'0' || c == b'1',
        _ => (b'0'..=b'7').contains(&c),
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Exakter Wert einer Dauer in Nanosekunden (L4.4): Dezimaltext mal Faktor,
/// nur wenn das Ergebnis ganzzahlig ist und in i64 passt.
fn duration_ns(number: &str, factor: i128) -> Option<i64> {
    let text: String = number.chars().filter(|&c| c != '_').collect();
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], text[i + 1..].parse::<i32>().ok()?),
        None => (text.as_str(), 0),
    };
    let (int_part, frac_part) = match mantissa.find('.') {
        Some(i) => (&mantissa[..i], &mantissa[i + 1..]),
        None => (mantissa, ""),
    };
    let digits = format!("{int_part}{frac_part}");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    if digits.len() > 36 {
        return None;
    }
    let mut value: i128 = digits.parse().ok()?;
    value = value.checked_mul(factor)?;
    let scale = exponent - frac_part.len() as i32;
    if scale > 40 {
        return None;
    }
    if scale >= 0 {
        for _ in 0..scale {
            value = value.checked_mul(10)?;
        }
    } else {
        for _ in 0..-scale {
            if value % 10 != 0 {
                return None;
            }
            value /= 10;
        }
    }
    i64::try_from(value).ok()
}
