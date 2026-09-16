//! Parser: rekursiver Abstieg ueber `grammar/takt.ebnf`, eine Funktion je
//! Produktion (`parse_<produktion>`). Der Test `tests/productions.rs` prueft,
//! dass keine Produktion ohne Funktion ist.
//!
//! Fehler tragen Position und Vorschlag. Nach einem Fehler setzt der Parser an
//! der naechsten Anweisung oder Deklaration auf derselben Einrueckung wieder auf,
//! damit eine Datei mehrere Fehler auf einmal meldet. Er bricht nie ab.

mod decl;
mod expr;
mod machine;
mod stmt;
mod types;

use takt_diag::Diagnostic;

use crate::ast::*;
use crate::keywords::is_contextual;
use crate::token::{Token, TokenKind, Tokens};

/// Ergebnis einer Produktion.
pub type PResult<T> = Result<T, Diagnostic>;

/// Code der Parserdiagnosen (Pruefung 1 in Referenz 10).
pub const CODE: &str = "P";

/// Parst eine Datei (`file`). Fehler werden gesammelt; der Baum enthaelt alles,
/// was sich parsen liess.
pub fn parse_file(toks: &Tokens<'_>) -> (File, Vec<Diagnostic>) {
    with_deep_stack(|| parse_file_here(toks))
}

/// Parst einen Schnipsel: Deklarationen, Zustandsinhalte, Sequenzschritte und
/// Anweisungen in beliebiger Folge (Testeinstieg fuer die Codebloecke der Referenz).
pub fn parse_snippet(toks: &Tokens<'_>) -> (Vec<SnippetItem>, Vec<Diagnostic>) {
    with_deep_stack(|| parse_snippet_here(toks))
}

/// Parst genau einen Ausdruck, etwa den Platzhalter eines Formatstrings (3.9);
/// nach dem Ausdruck darf nur noch Zeilenende folgen.
pub fn parse_expr(toks: &Tokens<'_>) -> Result<Expr, Diagnostic> {
    with_deep_stack(|| {
        let mut p = Parser::new(toks);
        let expr = p.parse_expr()?;
        // `is_layout` schliesst `Eof` ein, und `bump` bleibt dort stehen:
        // deshalb nur Zeilenenden ueberspringen.
        while matches!(p.kind(), TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent) {
            p.bump();
        }
        if !p.at(TokenKind::Eof) {
            return Err(p.error_here("Ende des Ausdrucks"));
        }
        Ok(expr)
    })
}

/// `parse_file` auf dem aktuellen Thread (fuer Aufrufer, die schon unter
/// `with_deep_stack` laufen).
pub(crate) fn parse_file_here(toks: &Tokens<'_>) -> (File, Vec<Diagnostic>) {
    let mut p = Parser::new(toks);
    let file = p.parse_file();
    (file, p.errors)
}

pub(crate) fn parse_snippet_here(toks: &Tokens<'_>) -> (Vec<SnippetItem>, Vec<Diagnostic>) {
    let mut p = Parser::new(toks);
    let items = p.parse_snippet_items();
    (items, p.errors)
}

/// Byte-Bereich eines Tokens als Diagnose-Position.
pub(crate) fn token_span(t: &Token) -> Span {
    Span::new(t.start, t.end)
}

/// Stapel fuer den rekursiven Abstieg: `MAX_DEPTH` Ebenen brauchen in einem
/// Debug-Build mehrere Megabyte, mehr als ein Hauptthread hat.
const DEEP_STACK: usize = 64 << 20;

/// Fuehrt `f` auf einem Thread mit grossem Stapel aus, damit die Tiefengrenze
/// `MAX_DEPTH` und nicht der Stapel entscheidet, was der Parser annimmt.
pub(crate) fn with_deep_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(DEEP_STACK)
            .spawn_scoped(scope, f)
            .expect("Parser-Thread")
            .join()
            .expect("Parser-Thread ohne Panik")
    })
}

/// Zustand des Parsers.
pub struct Parser<'t, 's> {
    toks: &'t Tokens<'s>,
    pos: usize,
    errors: Vec<Diagnostic>,
    /// In einer `property`: Temporaloperatoren und `implies` sind Ausdruecke.
    temporal: bool,
    /// Tiefe offener `<` in Typen: dort schliesst `>` und vergleicht nicht.
    angle: u32,
    /// Verschachtelung von Ausdruecken, Typen und Bloecken (Grenze `MAX_DEPTH`).
    depth: u32,
}

/// Tiefer verschachtelt darf kein Programm sein: schuetzt den Stapel des Parsers
/// (und jedes spaeteren Durchlaufs ueber den Baum) vor pathologischen Eingaben.
pub const MAX_DEPTH: u32 = 64;

const SCALAR_WORDS: &[&str] =
    &["bool", "int", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "float", "f32", "f64", "Duration", "str"];

impl<'t, 's> Parser<'t, 's> {
    fn new(toks: &'t Tokens<'s>) -> Self {
        Parser { toks, pos: 0, errors: Vec::new(), temporal: false, angle: 0, depth: 0 }
    }

    // ------------------------------------------------------------ Cursor

    fn tok(&self) -> &'t Token {
        &self.toks.tokens[self.pos.min(self.toks.tokens.len() - 1)]
    }

    fn tok_at(&self, n: usize) -> &'t Token {
        &self.toks.tokens[(self.pos + n).min(self.toks.tokens.len() - 1)]
    }

    fn kind(&self) -> TokenKind {
        self.tok().kind
    }

    fn text_of(&self, t: &Token) -> &'s str {
        self.toks.text(t)
    }

    fn text(&self) -> &'s str {
        self.text_of(self.tok())
    }

    fn bump(&mut self) -> &'t Token {
        let t = self.tok();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    fn at(&self, kind: TokenKind) -> bool {
        self.kind() == kind
    }

    fn at_op(&self, op: &str) -> bool {
        self.kind() == TokenKind::Op && self.text() == op
    }

    /// Folgt dem Wort eine Zuweisung oder ein Zugriff? Dann ist ein
    /// Klauselwort wie `on` ein Bezeichner (2.2, FB-92).
    fn assigned_next(&self) -> bool {
        ["=", "+=", "-=", "*=", "/=", ".", "["].iter().any(|op| self.at_op_at(1, op))
    }

    fn at_op_at(&self, n: usize, op: &str) -> bool {
        let t = self.tok_at(n);
        t.kind == TokenKind::Op && self.text_of(t) == op
    }

    fn at_kw(&self, kw: &str) -> bool {
        self.kind() == TokenKind::Keyword && self.text() == kw
    }

    fn is_word(kind: TokenKind) -> bool {
        matches!(kind, TokenKind::Ident | TokenKind::UpperIdent | TokenKind::TypeIdent | TokenKind::Keyword)
    }

    /// Kontextuelles Terminal: ein Wort mit diesem Text, gleich welcher Art.
    fn at_word(&self, word: &str) -> bool {
        Self::is_word(self.kind()) && self.text() == word
    }

    fn at_word_at(&self, n: usize, word: &str) -> bool {
        let t = self.tok_at(n);
        Self::is_word(t.kind) && self.text_of(t) == word
    }

    /// Zwei anliegende `>` bilden den Shift-Operator (lexer.md L6.1).
    fn at_shift_right(&self) -> bool {
        self.at_op(">") && self.tok().joint && self.at_op_at(1, ">")
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.at_op(op) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.at_kw(kw) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, word: &str) -> bool {
        if self.at_word(word) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Betritt eine Verschachtelungsebene (Ausdruck, Typ, Block).
    fn enter(&mut self) -> PResult<()> {
        if self.depth >= MAX_DEPTH {
            return Err(self.error_at(
                self.tok(),
                format!("zu tief verschachtelt (mehr als {MAX_DEPTH} Ebenen)"),
                Some("Ausdruck oder Block aufteilen"),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    /// `"<" … ">"` eines Typs; innen ist `>` kein Vergleich.
    fn in_angles<T>(&mut self, inner: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.expect_op("<")?;
        self.angle += 1;
        let result = inner(self);
        self.angle -= 1;
        let value = result?;
        self.expect_op(">")?;
        Ok(value)
    }

    /// Inhalt runder Klammern: dort vergleicht `>` wieder.
    fn nested<T>(&mut self, inner: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        let saved = std::mem::replace(&mut self.angle, 0);
        let result = inner(self);
        self.angle = saved;
        result
    }

    /// Argumente, Indizes, Array-Literale, Generik: gewoehnliche Ausdruecke, auch in
    /// einer Eigenschaft (dort gibt es Temporaloperatoren nur auf Formelebene).
    fn plain<T>(&mut self, inner: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        let saved = (std::mem::replace(&mut self.angle, 0), std::mem::replace(&mut self.temporal, false));
        let result = inner(self);
        (self.angle, self.temporal) = saved;
        result
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    // ------------------------------------------------------------ Fehler

    fn describe(&self, t: &Token) -> String {
        match t.kind {
            TokenKind::Newline => "Zeilenende".into(),
            TokenKind::Indent => "Einrueckung".into(),
            TokenKind::Dedent => "Blockende".into(),
            TokenKind::Eof => "Dateiende".into(),
            TokenKind::Str => "Stringliteral".into(),
            TokenKind::Duration => "Dauer".into(),
            TokenKind::Keyword => format!("Schluesselwort `{}`", self.text_of(t)),
            _ => format!("`{}`", self.text_of(t)),
        }
    }

    fn error_here(&self, expected: &str) -> Diagnostic {
        let t = self.tok();
        Diagnostic::error(CODE, token_span(t), format!("erwartet {expected}, gefunden {}", self.describe(t)))
    }

    fn error_at(&self, t: &Token, message: impl Into<String>, suggestion: Option<&str>) -> Diagnostic {
        let d = Diagnostic::error(CODE, token_span(t), message);
        match suggestion {
            Some(s) => d.with_suggestion(s),
            None => d,
        }
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> PResult<&'t Token> {
        if self.at(kind) { Ok(self.bump()) } else { Err(self.error_here(what)) }
    }

    fn expect_op(&mut self, op: &str) -> PResult<&'t Token> {
        if self.at_op(op) { Ok(self.bump()) } else { Err(self.error_here(&format!("`{op}`"))) }
    }

    fn expect_kw(&mut self, kw: &str) -> PResult<&'t Token> {
        if self.at_kw(kw) { Ok(self.bump()) } else { Err(self.error_here(&format!("`{kw}`"))) }
    }

    fn expect_word(&mut self, word: &str) -> PResult<&'t Token> {
        if self.at_word(word) { Ok(self.bump()) } else { Err(self.error_here(&format!("`{word}`"))) }
    }

    fn expect_newline(&mut self) -> PResult<()> {
        self.expect(TokenKind::Newline, "Zeilenende").map(|_| ())
    }

    fn expect_indent(&mut self) -> PResult<()> {
        if self.at(TokenKind::Indent) {
            self.bump();
            Ok(())
        } else {
            let t = self.tok();
            Err(self.error_at(t, "erwartet eingerueckten Block", Some("Block um 4 Leerzeichen einruecken")))
        }
    }

    fn expect_dedent(&mut self) -> PResult<()> {
        self.expect(TokenKind::Dedent, "Blockende").map(|_| ())
    }

    /// Ueberspringt bis zum naechsten Zeilenende auf gleicher Einrueckung (ein
    /// dazwischen geoeffneter Block wird ganz uebersprungen); haelt vor einem
    /// Blockende der umgebenden Ebene.
    fn recover_line(&mut self) {
        let mut depth = 0u32;
        loop {
            match self.kind() {
                TokenKind::Eof => return,
                TokenKind::Newline if depth == 0 => {
                    self.bump();
                    return;
                }
                TokenKind::Indent => {
                    depth += 1;
                    self.bump();
                }
                TokenKind::Dedent => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.bump();
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Fehlerbehandlung fuer Schleifen: Zeile ueberspringen und sicherstellen,
    /// dass der Parser gegenueber `start` vorangekommen ist.
    fn recover(&mut self, start: usize) {
        self.recover_line();
        if self.pos == start && !self.at(TokenKind::Eof) {
            self.bump();
        }
    }

    fn report(&mut self, e: Diagnostic) {
        self.errors.push(e);
    }

    // ------------------------------------------------------------ Spannen und Namen

    fn span_from(&self, start: usize) -> Span {
        let first = &self.toks.tokens[start.min(self.toks.tokens.len() - 1)];
        if self.pos > start {
            Span::new(first.start, self.toks.tokens[self.pos - 1].end)
        } else {
            Span::new(first.start, first.start)
        }
    }

    fn ident_of(&self, t: &Token) -> Ident {
        Ident { name: self.text_of(t).to_string(), span: Span::new(t.start, t.end) }
    }

    fn ident(&mut self) -> PResult<Ident> {
        let t = self.expect(TokenKind::Ident, "einen Namen in snake_case")?;
        Ok(self.ident_of(t))
    }

    fn upper(&mut self) -> PResult<Ident> {
        let t = self.expect(TokenKind::UpperIdent, "einen Namen in UPPER_SNAKE_CASE")?;
        Ok(self.ident_of(t))
    }

    fn type_ident(&mut self) -> PResult<Ident> {
        let t = self.expect(TokenKind::TypeIdent, "einen Typnamen in PascalCase")?;
        Ok(self.ident_of(t))
    }

    fn string(&mut self) -> PResult<StrLit> {
        let t = self.expect(TokenKind::Str, "ein Stringliteral")?;
        Ok(StrLit { value: self.toks.unescape(t), span: Span::new(t.start, t.end) })
    }

    fn int_token(&mut self) -> PResult<IntLit> {
        let t = self.expect(TokenKind::Int, "eine ganze Zahl")?;
        Ok(IntLit { text: self.text_of(t).to_string(), span: Span::new(t.start, t.end) })
    }

    /// `int_lit := INT | HEX | BIN | OCT`
    fn parse_int_lit(&mut self) -> PResult<IntLit> {
        if matches!(self.kind(), TokenKind::Int | TokenKind::Hex | TokenKind::Bin | TokenKind::Oct) {
            let t = self.bump();
            Ok(IntLit { text: self.text_of(t).to_string(), span: Span::new(t.start, t.end) })
        } else {
            Err(self.error_here("eine ganze Zahl"))
        }
    }

    /// `number := int_lit | FLOAT`
    fn parse_number(&mut self) -> PResult<Number> {
        if self.at(TokenKind::Float) {
            let t = self.bump();
            Ok(Number::Float(FloatLit { text: self.text_of(t).to_string(), span: Span::new(t.start, t.end) }))
        } else {
            self.parse_int_lit().map(Number::Int)
        }
    }

    /// `duration_lit := DURATION`
    fn parse_duration_lit(&mut self) -> PResult<DurationLit> {
        let t = self.expect(TokenKind::Duration, "eine Dauer wie `200 ms`")?;
        Ok(DurationLit { ns: t.value, span: Span::new(t.start, t.end) })
    }

    // ------------------------------------------------------------ Gemeinsame Produktionen

    /// `[ "with" attr { "," attr } ]`
    fn parse_with_attrs(&mut self) -> PResult<Vec<Attr>> {
        let mut attrs = Vec::new();
        if self.eat_kw("with") {
            loop {
                attrs.push(self.parse_attr()?);
                if !self.eat_op(",") {
                    break;
                }
            }
        }
        Ok(attrs)
    }

    /// `attr`
    fn parse_attr(&mut self) -> PResult<Attr> {
        let start = self.pos;
        if !Self::is_word(self.kind()) {
            return Err(self.error_here("ein Attribut wie `safe`, `max_age`, `capacity`"));
        }
        let name_tok = self.bump();
        let name = self.text_of(name_tok);
        self.expect_op("=")?;
        let kind = match name {
            "safe" => AttrKind::Safe(self.parse_const_expr()?),
            "max_age" => AttrKind::MaxAge(self.parse_duration_lit()?),
            "rate" => AttrKind::Rate(self.parse_const_expr()?),
            "max_rate" => AttrKind::MaxRate(self.parse_const_expr()?),
            "capacity" => AttrKind::Capacity(self.parse_int_lit()?),
            "framing" => AttrKind::Framing(self.parse_framing()?),
            "overflow" => {
                let value = if self.eat_word("fault") {
                    Overflow::Fault
                } else if self.eat_word("drop_oldest") {
                    Overflow::DropOldest
                } else if self.eat_word("drop") {
                    Overflow::Drop
                } else {
                    return Err(self.error_here("`fault`, `drop_oldest` oder `drop`"));
                };
                AttrKind::Overflow(value)
            }
            "wake" => AttrKind::Wake(self.parse_bool_word()?),
            "jitter" => AttrKind::Jitter(self.parse_duration_lit()?),
            "max_slew" => AttrKind::MaxSlew(self.parse_const_expr()?),
            "debounce" => AttrKind::Debounce(self.parse_int_lit()?),
            "capacity_bytes" => AttrKind::CapacityBytes(self.parse_int_lit()?),
            "expect_len" => AttrKind::ExpectLen(self.parse_int_lit()?),
            "irreversible" => {
                self.expect_kw("true")?;
                AttrKind::Irreversible
            }
            "label" => AttrKind::Label(self.string()?),
            "display" => AttrKind::Display(self.parse_unit_expr(false)?),
            "group" => AttrKind::Group(self.string()?),
            "doc" => AttrKind::Doc(self.string()?),
            "budget" => AttrKind::Budget(self.parse_budget()?),
            other => {
                return Err(self.error_at(
                    name_tok,
                    format!("unbekanntes Attribut `{other}`"),
                    Some("Attribute: safe max_age rate max_rate capacity framing overflow wake jitter max_slew debounce capacity_bytes expect_len irreversible label display group doc budget"),
                ));
            }
        };
        Ok(Attr { kind, span: self.span_from(start) })
    }

    /// `budget = {ram = 2 KiB, wcet = 20 us}` (7.2).
    fn parse_budget(&mut self) -> PResult<Vec<BudgetItem>> {
        self.expect_op("{")?;
        let mut out = Vec::new();
        loop {
            out.push(self.parse_budget_item()?);
            if !self.eat_op(",") {
                break;
            }
        }
        self.expect_op("}")?;
        Ok(out)
    }

    /// `budget_item`
    fn parse_budget_item(&mut self) -> PResult<BudgetItem> {
        let start = self.pos;
        let tok = self.tok();
        let kind = if self.eat_word("ram") {
            BudgetKind::Ram
        } else if self.eat_word("wcet") {
            BudgetKind::Wcet
        } else {
            return Err(self.error_at(tok, "erwartet `ram` oder `wcet`".to_string(), None));
        };
        self.expect_op("=")?;
        let value = self.parse_const_expr()?;
        Ok(BudgetItem { kind, value, span: self.span_from(start) })
    }

    fn parse_bool_word(&mut self) -> PResult<bool> {
        if self.eat_kw("true") {
            Ok(true)
        } else if self.eat_kw("false") {
            Ok(false)
        } else {
            Err(self.error_here("`true` oder `false`"))
        }
    }

    /// `framing`
    fn parse_framing(&mut self) -> PResult<Framing> {
        if self.eat_word("raw") {
            Ok(Framing::Raw)
        } else if self.eat_word("lines") {
            Ok(Framing::Lines)
        } else if self.eat_word("cobs") {
            Ok(Framing::Cobs)
        } else if self.eat_word("length_prefixed") {
            self.expect_op("(")?;
            let width = self.ident()?;
            self.expect_op(")")?;
            Ok(Framing::LengthPrefixed(width))
        } else if self.eat_word("fixed") {
            self.expect_op("(")?;
            let n = self.int_token()?;
            self.expect_op(")")?;
            Ok(Framing::Fixed(n))
        } else {
            Err(self.error_here("`raw`, `lines`, `cobs`, `length_prefixed(…)` oder `fixed(N)`"))
        }
    }

    /// `binding`
    fn parse_binding(&mut self) -> PResult<Binding> {
        if self.eat_word("hw") {
            self.expect_op("(")?;
            let s = self.string()?;
            self.expect_op(")")?;
            Ok(Binding::Hw(s))
        } else if self.eat_word("sim") {
            self.expect_op("(")?;
            let s = self.string()?;
            self.expect_op(")")?;
            Ok(Binding::Sim(s))
        } else if self.eat_kw("none") {
            Ok(Binding::None)
        } else {
            Err(self.error_here("`hw(\"…\")`, `sim(\"…\")` oder `none`"))
        }
    }

    /// `generic_vars := "[" gvar { "," gvar } "]"`
    fn parse_generic_vars(&mut self) -> PResult<Vec<GenericVar>> {
        let mut vars = Vec::new();
        if self.eat_op("[") {
            loop {
                vars.push(self.parse_gvar()?);
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op("]")?;
        }
        Ok(vars)
    }

    /// `gvar`
    fn parse_gvar(&mut self) -> PResult<GenericVar> {
        if self.eat_kw("type") {
            let name = self.upper()?;
            let capability = if self.eat_op(":") { Some(self.parse_capability()?) } else { None };
            Ok(GenericVar::Type { name, capability })
        } else if self.eat_kw("const") {
            let name = self.upper()?;
            let range = if self.eat_kw("in") { Some(self.parse_range()?) } else { None };
            Ok(GenericVar::Const { name, range })
        } else {
            Ok(GenericVar::Unit(self.upper()?))
        }
    }

    /// `capability`
    fn parse_capability(&mut self) -> PResult<Capability> {
        let c = if self.eat_word("pod") {
            Capability::Pod
        } else if self.eat_word("eq") {
            Capability::Eq
        } else if self.eat_word("ord") {
            Capability::Ord
        } else if self.eat_word("numeric") {
            Capability::Numeric
        } else if self.eat_word("integer") {
            Capability::Integer
        } else if self.eat_word("float") {
            Capability::Float
        } else {
            return Err(self.error_here("`pod`, `eq`, `ord`, `numeric`, `integer` oder `float`"));
        };
        Ok(c)
    }

    /// `params := param { "," param }`; der Aufrufer prueft die Klammern.
    fn parse_params(&mut self) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        if self.at_op(")") {
            return Ok(params);
        }
        loop {
            params.push(self.parse_param()?);
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(params)
    }

    /// `param`
    fn parse_param(&mut self) -> PResult<Param> {
        let start = self.pos;
        let inout = self.eat_word("inout");
        let name = self.ident()?;
        self.expect_op(":")?;
        let dir = if self.eat_kw("input") {
            Some(Direction::Input)
        } else if self.eat_kw("output") {
            Some(Direction::Output)
        } else {
            None
        };
        let ty = self.parse_type()?;
        let default = if self.eat_op("=") { Some(self.parse_const_expr()?) } else { None };
        Ok(Param { inout, name, dir, ty, default, span: self.span_from(start) })
    }

    /// `"(" [ params ] ")"`
    fn parse_param_list(&mut self) -> PResult<Vec<Param>> {
        self.expect_op("(")?;
        let params = self.parse_params()?;
        self.expect_op(")")?;
        Ok(params)
    }

    /// `"(" [ args ] ")"`
    fn parse_arg_list(&mut self) -> PResult<Vec<Arg>> {
        self.expect_op("(")?;
        let args = self.plain(Self::parse_args)?;
        self.expect_op(")")?;
        Ok(args)
    }

    /// `args := arg { "," arg }` (leer erlaubt; der Aufrufer prueft die Klammern)
    fn parse_args(&mut self) -> PResult<Vec<Arg>> {
        let mut args = Vec::new();
        if self.at_op(")") {
            return Ok(args);
        }
        loop {
            args.push(self.parse_arg()?);
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(args)
    }

    /// `arg := [ IDENT "=" ] expr`
    fn parse_arg(&mut self) -> PResult<Arg> {
        let start = self.pos;
        let name = if self.at(TokenKind::Ident) && self.at_op_at(1, "=") {
            let n = self.ident()?;
            self.expect_op("=")?;
            Some(n)
        } else {
            None
        };
        let value = self.parse_expr()?;
        Ok(Arg { name, value, span: self.span_from(start) })
    }

    /// `range := const_expr ".." const_expr`
    fn parse_range(&mut self) -> PResult<Range> {
        let start = self.pos;
        let from = self.parse_const_expr()?;
        self.expect_op("..")?;
        let to = self.parse_const_expr()?;
        Ok(Range { from: Box::new(from), to: Box::new(to), span: self.span_from(start) })
    }

    /// `const_expr := expr` (die Einschraenkung auf Konstanten prueft die Semantik)
    fn parse_const_expr(&mut self) -> PResult<Expr> {
        self.parse_expr()
    }

    /// `duration_expr := expr`
    fn parse_duration_expr(&mut self) -> PResult<Expr> {
        self.parse_expr()
    }

    /// `pattern`
    fn parse_pattern(&mut self) -> PResult<Pattern> {
        if self.at(TokenKind::Str) {
            return Ok(Pattern::Text(self.string()?));
        }
        let ty = self.type_ident()?;
        self.expect_op("(")?;
        let mut fields = Vec::new();
        if !self.at_op(")") {
            loop {
                let name = self.ident()?;
                self.expect_op("=")?;
                let value = self.parse_const_expr()?;
                fields.push((name, value));
                if !self.eat_op(",") {
                    break;
                }
            }
        }
        self.expect_op(")")?;
        Ok(Pattern::Record { ty, fields })
    }

    /// `unit_lit := unit_expr`, kompakt: nach einem Zahlenliteral reicht der
    /// Ausdruck genau so weit, wie die Tokens anliegen (lexer.md L4.3).
    fn parse_unit_lit(&mut self) -> PResult<UnitExpr> {
        let unit = self.parse_unit_expr(true)?;
        let prev_joint = self.toks.tokens[self.pos - 1].joint;
        if prev_joint && (self.at_op("*") || self.at_op("/") || self.at_op("^")) {
            return Err(self.error_at(
                self.tok(),
                "die anliegende Folge hinter der Zahl ist kein Einheitenausdruck",
                Some("Einheit ohne Leerraum schreiben (`5 K/min`), Operatoren mit Leerraum (`5 K / 2`)"),
            ));
        }
        Ok(unit)
    }

    /// `unit_expr := ( unit_term | "1" "/" unit_term ) { ("*" | "/") unit_term }`
    ///
    /// `compact`: nur anliegende Tokens gehoeren dazu (`unit_lit`).
    fn parse_unit_expr(&mut self, compact: bool) -> PResult<UnitExpr> {
        let start = self.pos;
        let (first, mut rest) = if self.at(TokenKind::Int) && self.text() == "1" {
            let one = self.bump();
            let one_term = UnitTerm { name: None, exponent: None, span: Span::new(one.start, one.end) };
            if !self.at_op("/") || (compact && !(one.joint && self.tok().joint)) {
                return Err(self.error_here("`/` nach der `1` eines Einheitenausdrucks (dimensionslos ist `1/s`)"));
            }
            self.bump();
            (one_term, vec![(UnitOp::Div, self.parse_unit_term(compact)?)])
        } else {
            (self.parse_unit_term(compact)?, Vec::new())
        };
        loop {
            let prev_joint = self.toks.tokens[self.pos - 1].joint;
            let continues = (self.at_op("*") || self.at_op("/")) && (!compact || (prev_joint && self.tok().joint));
            if !continues {
                break;
            }
            let op = if self.eat_op("*") {
                UnitOp::Mul
            } else {
                self.bump();
                UnitOp::Div
            };
            rest.push((op, self.parse_unit_term(compact)?));
        }
        Ok(UnitExpr { first, rest, span: self.span_from(start) })
    }

    /// `unit_term := ( IDENT | UPPER_IDENT | TYPE_IDENT ) [ "^" INT ]`; in einem
    /// `unit_lit` gehoert der Exponent nur anliegend dazu.
    /// `unit_name`: Einheitennamen tragen jede Namensform (3.2).
    pub(super) fn parse_unit_name(&mut self) -> PResult<Ident> {
        if matches!(self.kind(), TokenKind::Ident | TokenKind::UpperIdent | TokenKind::TypeIdent) {
            let t = self.bump();
            return Ok(self.ident_of(t));
        }
        Err(self.error_here("einen Einheitennamen wie `psi`, `V` oder `KiB`"))
    }

    fn parse_unit_term(&mut self, compact: bool) -> PResult<UnitTerm> {
        let start = self.pos;
        let name = if matches!(self.kind(), TokenKind::Ident | TokenKind::UpperIdent | TokenKind::TypeIdent) {
            let t = self.bump();
            Some(self.ident_of(t))
        } else {
            return Err(self.error_here("einen Einheitennamen wie `bar`, `K/min` oder `1/s`"));
        };
        let prev_joint = self.toks.tokens[self.pos - 1].joint;
        let exponent = if self.at_op("^") && (!compact || (prev_joint && self.tok().joint)) {
            self.bump();
            Some(self.int_token()?)
        } else {
            None
        };
        Ok(UnitTerm { name, exponent, span: self.span_from(start) })
    }

    /// Steht ein Einheitenausdruck an? Ein Wort, das kein kontextuelles Terminal
    /// ist (lexer.md L4.3), oder die `1` eines Kehrwerts.
    fn at_unit_start(&self) -> bool {
        (matches!(self.kind(), TokenKind::Ident | TokenKind::UpperIdent | TokenKind::TypeIdent)
            && !is_contextual(self.text()))
            || (self.at(TokenKind::Int) && self.text() == "1" && self.tok().joint && self.at_op_at(1, "/"))
    }

    /// `unit_tuple`
    fn parse_unit_tuple(&mut self) -> PResult<UnitTuple> {
        if self.at(TokenKind::Int) && self.text() == "1" {
            self.bump();
            self.expect_op("/")?;
            return Ok(UnitTuple::Inverse(self.upper()?));
        }
        if self.eat_op("(") {
            let mut units = vec![self.parse_unit_expr(false)?];
            while self.eat_op(",") {
                units.push(self.parse_unit_expr(false)?);
            }
            self.expect_op(")")?;
            return Ok(UnitTuple::Literal(units));
        }
        Ok(UnitTuple::Named(self.upper()?))
    }

    fn is_scalar_word(&self) -> bool {
        Self::is_word(self.kind()) && SCALAR_WORDS.contains(&self.text())
    }

    // ------------------------------------------------------------ Schnipsel

    fn parse_snippet_items(&mut self) -> Vec<SnippetItem> {
        let mut items = Vec::new();
        while !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Newline) {
                continue;
            }
            let start = self.pos;
            match self.parse_snippet_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.report(e);
                    self.recover(start);
                }
            }
        }
        items
    }

    fn parse_snippet_item(&mut self) -> PResult<SnippetItem> {
        if Self::is_word(self.kind()) && !self.assigned_next() {
            match self.text() {
                "fault" => return Ok(SnippetItem::MachinePrelude(MachinePrelude::Fault(self.parse_fault_clause()?))),
                "persist" => {
                    return Ok(SnippetItem::MachinePrelude(MachinePrelude::Persist(self.parse_persist_decl()?)));
                }
                "signal" => return Ok(SnippetItem::MachinePrelude(MachinePrelude::Signal(self.parse_signal_decl()?))),
                "initial" => {
                    self.bump();
                    let name = self.upper()?;
                    self.expect_newline()?;
                    return Ok(SnippetItem::Initial(name));
                }
                "enter" => return Ok(SnippetItem::Enter(self.parse_enter_block()?)),
                "exit" => return Ok(SnippetItem::Exit(self.parse_exit_block()?)),
                "loop" => return Ok(SnippetItem::Loop(self.parse_loop_block()?)),
                "on" => return Ok(SnippetItem::On(self.parse_on_handler()?)),
                "sequence" => return Ok(SnippetItem::Sequence(self.parse_sequence_block()?.1)),
                "when" | "after" => return Ok(SnippetItem::Transition(self.parse_transition()?)),
                "state" => return Ok(SnippetItem::State(self.parse_state_decl()?)),
                "instance" => return Ok(SnippetItem::Instance(self.parse_instance_decl()?)),
                "step" if self.at_op_at(1, "(") => return Ok(SnippetItem::Step(self.parse_step_decl()?)),
                "wait" | "until" | "expect" | "repeat" | "step" => return Ok(SnippetItem::Seq(self.parse_seq_item()?)),
                _ => {}
            }
        }
        if self.at_item_start() {
            return Ok(SnippetItem::Item(self.parse_item()?));
        }
        Ok(SnippetItem::Seq(SeqItem::Stmt(self.parse_stmt()?)))
    }
}
