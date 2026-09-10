//! Tokens, Beiwerk und Fehler des Tokenizers (grammar/lexer.md).

use takt_diag::{Diagnostic, Span};

/// Tokenart. Die Namen entsprechen den Tokenarten in lexer.md; `Op` deckt alle
/// Operatoren und Interpunktion aus L6 ab, der Text unterscheidet sie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Ende einer logischen Zeile.
    Newline,
    /// Einrueckung um eine Stufe.
    Indent,
    /// Rueckkehr um eine Stufe.
    Dedent,
    /// `IDENT`: snake_case oder mit `_` beginnend.
    Ident,
    /// `UPPER_IDENT`: nur Grossbuchstaben, Ziffern, `_`.
    UpperIdent,
    /// `TYPE_IDENT`: PascalCase.
    TypeIdent,
    /// Schluesselwort aus 2.2.
    Keyword,
    /// Reserviertes Wort aus 2.5 (der Parser meldet `E_RESERVED`).
    Reserved,
    /// Dezimale ganze Zahl.
    Int,
    /// `0x…`
    Hex,
    /// `0b…`
    Bin,
    /// `0o…`
    Oct,
    /// Fliesskommazahl.
    Float,
    /// Dauer; der Wert in Nanosekunden steht in `Token::value`.
    Duration,
    /// Stringliteral einschliesslich der Anfuehrungszeichen; `Tokens::unescape` loest Escapes.
    Str,
    /// Das Wort `_`.
    Wild,
    /// Operator oder Interpunktion (L6); Text ist das Zeichen.
    Op,
    /// Zeichen, die kein Token ergeben; der Fehler steht in `Tokens::errors`.
    Error,
    /// Dateiende; traegt das restliche Beiwerk.
    Eof,
}

impl TokenKind {
    /// Struktur- statt Inhaltstoken (Zeilenende, Ein- und Ausrueckung, Dateiende).
    pub fn is_layout(self) -> bool {
        matches!(self, TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Eof)
    }
}

/// Ein Token als Verweis in den Quelltext.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    /// Art.
    pub kind: TokenKind,
    /// Byte-Offset des ersten Zeichens.
    pub start: u32,
    /// Byte-Offset hinter dem letzten Zeichen.
    pub end: u32,
    /// Zeile ab 1.
    pub line: u32,
    /// Spalte ab 1 (Zeichen; ausserhalb von Strings gleich Bytes).
    pub col: u32,
    /// Das naechste Token folgt ohne Leerraum (lexer.md, "Anliegen").
    pub joint: bool,
    /// Beiwerk vor diesem Token: Indexbereich in `Tokens::trivia`.
    pub trivia: (u32, u32),
    /// Nur bei `Duration`: Wert in Nanosekunden.
    pub value: i64,
}

/// Art des Beiwerks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriviaKind {
    /// Kommentar ab `#` bis zum Zeilenende.
    Comment,
    /// Leere Zeile.
    BlankLine,
}

/// Beiwerk: Kommentar oder Leerzeile, als Verweis in den Quelltext.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trivia {
    /// Art.
    pub kind: TriviaKind,
    /// Byte-Offset des Anfangs.
    pub start: u32,
    /// Byte-Offset des Endes.
    pub end: u32,
}

/// Fehlercodes aus lexer.md, Abschnitt L9.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum ErrorCode {
    Bom,
    Cr,
    NonAscii,
    Tab,
    Indent,
    Dedent,
    Unclosed,
    Reserved,
    Number,
    UnitSpace,
    Duration,
    String,
    Escape,
    Format,
    Pattern,
    Address,
    Char,
}

impl ErrorCode {
    /// Name wie in lexer.md (`E_BOM`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Bom => "E_BOM",
            ErrorCode::Cr => "E_CR",
            ErrorCode::NonAscii => "E_NONASCII",
            ErrorCode::Tab => "E_TAB",
            ErrorCode::Indent => "E_INDENT",
            ErrorCode::Dedent => "E_DEDENT",
            ErrorCode::Unclosed => "E_UNCLOSED",
            ErrorCode::Reserved => "E_RESERVED",
            ErrorCode::Number => "E_NUMBER",
            ErrorCode::UnitSpace => "E_UNIT_SPACE",
            ErrorCode::Duration => "E_DURATION",
            ErrorCode::String => "E_STRING",
            ErrorCode::Escape => "E_ESCAPE",
            ErrorCode::Format => "E_FORMAT",
            ErrorCode::Pattern => "E_PATTERN",
            ErrorCode::Address => "E_ADDRESS",
            ErrorCode::Char => "E_CHAR",
        }
    }

    /// Ursache in einem Satzteil (Tabelle in L9).
    pub fn describe(self) -> &'static str {
        match self {
            ErrorCode::Bom => "Byte-Order-Mark am Dateianfang",
            ErrorCode::Cr => "einzelnes Wagenruecklauf-Zeichen",
            ErrorCode::NonAscii => "Zeichen ausserhalb von ASCII",
            ErrorCode::Tab => "Tabulator",
            ErrorCode::Indent => "Einrueckung kein Vielfaches von 4 oder Sprung um mehr als eine Stufe",
            ErrorCode::Dedent => "Einrueckung passt zu keinem offenen Block",
            ErrorCode::Unclosed => "Klammer am Dateiende nicht geschlossen",
            ErrorCode::Reserved => "reserviertes Wort",
            ErrorCode::Number => "ungueltiges Zahlenliteral",
            ErrorCode::UnitSpace => "Einheit ohne Leerzeichen nach der Zahl",
            ErrorCode::Duration => "Dauer nicht darstellbar",
            ErrorCode::String => "Stringliteral nicht geschlossen",
            ErrorCode::Escape => "unbekannte Escape-Sequenz",
            ErrorCode::Format => "ungueltiger Formatstring",
            ErrorCode::Pattern => "ungueltiges Musterliteral",
            ErrorCode::Address => "ungueltige Adresse",
            ErrorCode::Char => "Zeichen ohne Bedeutung",
        }
    }

    /// Handlungsvorschlag aus der Tabelle in L9.
    pub fn suggestion(self) -> &'static str {
        match self {
            ErrorCode::Bom => "takt fmt entfernt die Byte-Order-Mark",
            ErrorCode::Cr => "Zeilenenden \\n oder \\r\\n verwenden",
            ErrorCode::NonAscii => "Bezeichner und Einheiten in ASCII schreiben (degC, uA, ohm)",
            ErrorCode::Tab => "4 Leerzeichen je Stufe; takt fmt ersetzt Tabulatoren",
            ErrorCode::Indent => "Block um genau 4 Leerzeichen einruecken",
            ErrorCode::Dedent => "Einrueckung an den umgebenden Block angleichen",
            ErrorCode::Unclosed => "schliessende Klammer ergaenzen",
            ErrorCode::Reserved => "anderes Wort waehlen",
            ErrorCode::Number => "Formen: 42, 1_000, 0x1F, 0b1010, 0o17, 4.25, 1e-3",
            ErrorCode::UnitSpace => "Leerzeichen zwischen Zahl und Einheit: 85 degC",
            ErrorCode::Duration => "Dauer muss ganzzahlig in Nanosekunden und kleiner als 292 Jahre sein",
            ErrorCode::String => "schliessendes Anfuehrungszeichen ergaenzen",
            ErrorCode::Escape => "erlaubt sind \\\\ \\\" \\n \\t \\r \\0",
            ErrorCode::Format => "{ausdruck} oder {ausdruck:hex}; {{ fuer ein geschweiftes Zeichen",
            ErrorCode::Pattern => "{name:int}, {name:word}, {_}; {{ fuer ein geschweiftes Zeichen",
            ErrorCode::Address => "geraet/kanal, Bereich als kanal[0:16]",
            ErrorCode::Char => "Zeichen ohne Bedeutung; siehe lexer.md L6.3",
        }
    }
}

/// Baut die Diagnose eines Tokenizer-Fehlers (lexer.md L9): Meldung aus Code und
/// Detail, Vorschlag aus der Tabelle; `while` bekommt die eigene Meldung aus 2.5.
pub fn lex_diagnostic(code: ErrorCode, span: Span, detail: &str) -> Diagnostic {
    let message = match (code, detail.is_empty()) {
        (ErrorCode::Reserved, _) => format!("`{detail}` ist ein reserviertes Wort"),
        (_, true) => code.describe().to_string(),
        (_, false) => format!("{}: {detail}", code.describe()),
    };
    let suggestion = if code == ErrorCode::Reserved && detail == "while" {
        "nicht erlaubt: for mit Schranke oder sequence mit until"
    } else {
        code.suggestion()
    };
    Diagnostic::error(code.as_str(), span, message).with_suggestion(suggestion)
}

/// Ergebnis des Tokenizers.
#[derive(Debug)]
pub struct Tokens<'src> {
    /// Quelltext, auf den die Tokens verweisen.
    pub src: &'src str,
    /// Tokens in Reihenfolge; das letzte ist immer `Eof`.
    pub tokens: Vec<Token>,
    /// Beiwerk, referenziert ueber `Token::trivia`.
    pub trivia: Vec<Trivia>,
    /// Fehler in Reihenfolge des Auftretens.
    pub errors: Vec<Diagnostic>,
}

impl<'src> Tokens<'src> {
    /// Quelltext eines Tokens.
    pub fn text(&self, token: &Token) -> &'src str {
        &self.src[token.start as usize..token.end as usize]
    }

    /// Inhalt eines Stringliterals mit aufgeloesten Escapes (L5.2).
    pub fn unescape(&self, token: &Token) -> String {
        unescape(self.text(token))
    }

    /// Beiwerk vor einem Token.
    pub fn trivia_of(&self, token: &Token) -> &[Trivia] {
        &self.trivia[token.trivia.0 as usize..token.trivia.1 as usize]
    }
}

/// Loest die Escapes eines Stringliterals (mit Anfuehrungszeichen) auf.
pub fn unescape(raw: &str) -> String {
    let inner = raw.strip_prefix('"').unwrap_or(raw);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}
