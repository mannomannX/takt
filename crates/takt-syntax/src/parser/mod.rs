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

use std::collections::BTreeSet;

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

/// Die Produktionen aus `grammar/takt.ebnf`, deren Parserfunktion beim Parsen
/// lief: die Abdeckung der Grammatik durch eine Eingabe (plan.md 3).
pub fn productions_of(toks: &Tokens<'_>, snippet: bool) -> BTreeSet<&'static str> {
    with_deep_stack(|| {
        let mut p = Parser::new(toks);
        p.hits = Some(BTreeSet::new());
        if snippet {
            p.parse_snippet_items();
        } else {
            p.parse_file();
        }
        p.hits.unwrap_or_default()
    })
}

/// Byte-Bereich eines Tokens als Diagnose-Position.
pub(crate) fn token_span(t: &Token) -> Span {
    Span::new(t.start, t.end)
}

// Die Tiefengrenze `MAX_DEPTH` und nicht der Stapel entscheidet, was der
// Parser annimmt (2.1).
pub(crate) use takt_diag::stack::with_deep_stack;

/// Zustand des Parsers.
pub struct Parser<'t, 's> {
    toks: &'t Tokens<'s>,
    pos: usize,
    errors: Vec<Diagnostic>,
    /// In einer `property`: Temporaloperatoren und `implies` sind Ausdruecke.
    temporal: bool,
    /// Tiefe offener `<` in Typen: dort schliesst `>` und vergleicht nicht.
    angle: u32,
    /// Ebenen aus dem Tokenstrom je Token: offene eingerueckte Bloecke und
    /// offene runde und eckige Klammern davor (2.1).
    levels: Vec<u32>,
    /// Ebenen, die erst der Parser sieht: Praefixoperatoren, `else`-Zweige
    /// bedingter Ausdruecke, Typklammern `<…>` und Elementtypen von Feldern.
    depth: u32,
    /// Die Produktionen, deren Funktion lief; nur fuer [`productions_of`].
    hits: Option<BTreeSet<&'static str>>,
}

/// Tiefer verschachtelt darf kein Programm sein (2.1): schuetzt den Stapel des
/// Parsers (und jedes spaeteren Durchlaufs ueber den Baum) vor pathologischen
/// Eingaben. Eine Ebene ist ein eingerueckter Block, ein Klammerpaar (rund,
/// eckig, Typklammer), ein Praefixoperator und der `else`-Zweig eines
/// bedingten Ausdrucks, gezaehlt ueber alle Arten zusammen von der Datei an;
/// Anweisungen, aeussere Ausdruecke, Infix-Operatoren und Kettenglieder zaehlen nicht.
pub const MAX_DEPTH: u32 = 64;

/// So viele Knoten tief ist ein Ausdruck hoechstens (2.1), jedes Kettenglied
/// eingerechnet: `a + b + …` mit N Gliedern ist N tief. Neben `MAX_DEPTH`
/// beschraenkt das die Rekursion jedes Werkzeugs, das einen Ausdruck durchlaeuft.
pub const MAX_TREE: u32 = 256;

/// Tiefe eines Ausdrucks in Knoten (2.1), mit den Teilausdruecken in
/// Argumenten, Generik, Record-Mustern und Typen. Der Parser misst nur Baeume,
/// deren Teile er schon gemessen hat; die Rekursion endet darum nach
/// hoechstens `MAX_TREE` Knoten und den Ebenen der Typen.
pub(crate) fn tree_depth(e: &Expr) -> u32 {
    let one = |e: &Expr| tree_depth(e);
    let below = match &e.kind {
        ExprKind::Number { .. }
        | ExprKind::Duration(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::None
        | ExprKind::Default
        | ExprKind::Ident(_) => 0,
        ExprKind::Call { generics, args, .. } => {
            generics.iter().map(generic_depth).max().unwrap_or(0).max(args_depth(args))
        }
        ExprKind::Upper { args, .. } | ExprKind::TypeName { args, .. } => args.as_deref().map_or(0, args_depth),
        ExprKind::Paren(x) | ExprKind::Unary { expr: x, .. } | ExprKind::Temporal { inner: x, .. } => one(x),
        ExprKind::Tuple(a, b) | ExprKind::Binary { lhs: a, rhs: b, .. } | ExprKind::Implies { lhs: a, rhs: b } => {
            one(a).max(one(b))
        }
        ExprKind::Index { base: a, index: b } => one(a).max(one(b)),
        ExprKind::Array(items) => items.iter().map(one).max().unwrap_or(0),
        ExprKind::InstanceArray { count, args, .. } => one(count).max(args_depth(args)),
        ExprKind::Member { base, args, .. } => one(base).max(args.as_deref().map_or(0, args_depth)),
        ExprKind::Slice { base: a, from: b, to: c }
        | ExprKind::Index2 { base: a, row: b, col: c }
        | ExprKind::Conditional { then: a, cond: b, otherwise: c } => one(a).max(one(b)).max(one(c)),
        ExprKind::Cast { expr, ty } => one(expr).max(scalar_depth(ty)),
        ExprKind::Match { subject, pattern, .. } => one(subject).max(match pattern {
            Pattern::Text(_) => 0,
            Pattern::Record { fields, .. } => fields.iter().map(|(_, e)| one(e)).max().unwrap_or(0),
        }),
    };
    below + 1
}

fn args_depth(args: &[Arg]) -> u32 {
    args.iter().map(|a| tree_depth(&a.value)).max().unwrap_or(0)
}

fn generic_depth(g: &GenericArg) -> u32 {
    match g {
        GenericArg::Unit(_) => 0,
        GenericArg::Type(t) => type_depth(t),
        GenericArg::Const(e) => tree_depth(e),
    }
}

fn scalar_depth(s: &ScalarType) -> u32 {
    match s {
        ScalarType::Str(e) => tree_depth(e),
        _ => 0,
    }
}

/// Die tiefsten Ausdruecke eines Typs; Typknoten selbst zaehlen als Ebenen
/// (`MAX_DEPTH`), nicht als Knoten eines Ausdrucks.
fn type_depth(t: &Type) -> u32 {
    let range = |r: &Option<Range>| r.as_ref().map_or(0, |r| tree_depth(&r.from).max(tree_depth(&r.to)));
    match &t.kind {
        TypeKind::Scalar { scalar, range: r, .. } => scalar_depth(scalar).max(range(r)),
        TypeKind::Array { len, elem } | TypeKind::Vec { elem, len } | TypeKind::Samples { elem, len } => {
            tree_depth(len).max(type_depth(elem))
        }
        TypeKind::Bytes(e) | TypeKind::Line(e) => tree_depth(e),
        TypeKind::Stream(elem) => match elem.as_ref() {
            ElemType::Bytes(e) | ElemType::Line(e) => tree_depth(e),
            ElemType::Capture { elem, len } => tree_depth(len).max(type_depth(elem)),
            ElemType::U8 | ElemType::Edge | ElemType::Named(_) => 0,
        },
        TypeKind::Table { key, value } => type_depth(key).max(type_depth(value)),
        TypeKind::Mat { rows, cols, .. } => tree_depth(rows).max(tree_depth(cols)),
        TypeKind::Map { key, value, len } => type_depth(key).max(type_depth(value)).max(tree_depth(len)),
        TypeKind::Wrapped { inner, .. } => type_depth(inner),
        TypeKind::Named { .. } | TypeKind::TypeVar { .. } | TypeKind::MatDim { .. } | TypeKind::VecDim(_) => 0,
    }
}

/// Je Token die Zahl offener eingerueckter Bloecke und runder oder eckiger
/// Klammern vor ihm.
fn token_levels(toks: &Tokens<'_>) -> Vec<u32> {
    let mut level = 0u32;
    toks.tokens
        .iter()
        .map(|t| {
            let before = level;
            match (t.kind, toks.text(t)) {
                (TokenKind::Indent, _) | (TokenKind::Op, "(" | "[") => level += 1,
                (TokenKind::Dedent, _) | (TokenKind::Op, ")" | "]") => level = level.saturating_sub(1),
                _ => {}
            }
            before
        })
        .collect()
}

const SCALAR_WORDS: &[&str] =
    &["bool", "int", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "float", "f32", "f64", "Duration", "str"];

impl<'t, 's> Parser<'t, 's> {
    fn new(toks: &'t Tokens<'s>) -> Self {
        Parser {
            toks,
            pos: 0,
            errors: Vec::new(),
            temporal: false,
            angle: 0,
            levels: token_levels(toks),
            depth: 0,
            hits: None,
        }
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

    /// Prueft an einer Stelle, an der der Parser absteigt (Ausdruck, Typ,
    /// Block), dass sie hoechstens `MAX_DEPTH` Ebenen tief liegt (2.1).
    fn enter(&self) -> PResult<()> {
        let level = self.levels.get(self.pos).copied().unwrap_or(0) + self.depth;
        if level > MAX_DEPTH {
            return Err(self.error_at(
                self.tok(),
                format!("zu tief verschachtelt (mehr als {MAX_DEPTH} Ebenen)"),
                Some("Ausdruck oder Block aufteilen"),
            ));
        }
        Ok(())
    }

    /// `inner` eine Ebene tiefer, fuer Ebenen ohne Klammer und Block:
    /// Praefixoperator, `else`-Zweig, Typklammer, Elementtyp.
    fn deeper<T>(&mut self, inner: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.depth += 1;
        let result = self.enter().and_then(|()| inner(self));
        self.depth -= 1;
        result
    }

    /// Eine linksassoziative Kette `first { op next }`. Infix-Operatoren sind
    /// keine Ebene (2.1); die Kette baut ihren Baum in einer Schleife.
    fn chain<O>(
        &mut self,
        first: impl FnOnce(&mut Self) -> PResult<Expr>,
        mut op: impl FnMut(&mut Self) -> Option<O>,
        mut next: impl FnMut(&mut Self) -> PResult<Expr>,
        join: impl Fn(&Self, usize, O, Expr, Expr) -> Expr,
    ) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = first(self)?;
        let mut depth = tree_depth(&lhs);
        loop {
            let at = self.pos;
            let Some(o) = op(self) else { break };
            let rhs = next(self)?;
            // Jedes Glied ist ein Knoten (2.1): Die Kette waechst nur, solange
            // der Baum `MAX_TREE` haelt, und baut nie einen tieferen.
            depth = 1 + depth.max(tree_depth(&rhs));
            self.within_tree(depth, at)?;
            lhs = join(self, start, o, lhs, rhs);
        }
        Ok(lhs)
    }

    /// Ein Ausdruck mit `depth` Knoten Tiefe ist zu tief, wenn er `MAX_TREE`
    /// uebersteigt; gemeldet am Token `at`, an dem er die Grenze ueberschritt.
    fn within_tree(&self, depth: u32, at: usize) -> PResult<()> {
        if depth <= MAX_TREE {
            return Ok(());
        }
        let t = &self.toks.tokens[at.min(self.toks.tokens.len() - 1)];
        Err(self.error_at(
            t,
            format!("Ausdruck zu tief (mehr als {MAX_TREE} Knoten, jedes Kettenglied eingerechnet)"),
            Some("den Ausdruck ueber eine `var` teilen (2.1)"),
        ))
    }

    /// `"<" … ">"` eines Typs; innen ist `>` kein Vergleich.
    fn in_angles<T>(&mut self, inner: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.expect_op("<")?;
        self.angle += 1;
        let result = self.deeper(inner);
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
        Diagnostic::error(
            CODE,
            self.span(t.start, t.end),
            format!("erwartet {expected}, gefunden {}", self.describe(t)),
        )
    }

    fn error_at(&self, t: &Token, message: impl Into<String>, suggestion: Option<&str>) -> Diagnostic {
        let d = Diagnostic::error(CODE, self.span(t.start, t.end), message);
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
            self.enter()
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
                // Ein eingerueckter Block hinter der Zeile gehoert zu ihr und
                // faellt mit ihr, statt Folgefehler zu melden.
                TokenKind::Newline if depth == 0 => {
                    self.bump();
                    if !self.at(TokenKind::Indent) {
                        return;
                    }
                }
                TokenKind::Indent => {
                    depth += 1;
                    self.bump();
                }
                // Nach dem Ende des Blocks beginnt die naechste Zeile; sie wird
                // wieder geprueft, nicht mit uebersprungen.
                TokenKind::Dedent => {
                    if depth == 0 {
                        return;
                    }
                    depth -= 1;
                    self.bump();
                    if depth == 0 {
                        return;
                    }
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

    /// Fehlerbehandlung auf oberster Ebene: Scheiterte eine Deklaration in
    /// ihrem eingerueckten Rumpf, bleiben dessen Blockenden uebrig; sie
    /// gehoeren zur gescheiterten Deklaration und sind kein neuer Fehler.
    fn recover_item(&mut self, start: usize) {
        self.recover(start);
        while self.eat(TokenKind::Dedent) {}
    }

    /// Zaehlt die Produktion `name` als abgedeckt, wenn [`productions_of`] fragt.
    fn cover(&mut self, name: &'static str) {
        if let Some(hits) = &mut self.hits {
            hits.insert(name);
        }
    }

    fn report(&mut self, e: Diagnostic) {
        self.errors.push(e);
    }

    // ------------------------------------------------------------ Spannen und Namen

    /// Ein Bereich in der Datei der Tokens.
    fn span(&self, start: u32, end: u32) -> Span {
        Span { file: self.toks.file, start, end }
    }

    fn span_from(&self, start: usize) -> Span {
        let first = &self.toks.tokens[start.min(self.toks.tokens.len() - 1)];
        if self.pos > start {
            self.span(first.start, self.toks.tokens[self.pos - 1].end)
        } else {
            self.span(first.start, first.start)
        }
    }

    fn ident_of(&self, t: &Token) -> Ident {
        Ident { name: self.text_of(t).to_string(), span: self.span(t.start, t.end) }
    }

    fn ident(&mut self) -> PResult<Ident> {
        // Ein reserviertes Wort hat der Tokenizer schon gemeldet; als Name
        // genommen, folgt ihm kein zweiter Fehler an derselben Stelle (FB-408).
        if self.at(TokenKind::Reserved) {
            let t = self.bump();
            return Ok(self.ident_of(t));
        }
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
        Ok(StrLit { value: self.toks.unescape(t), span: self.span(t.start, t.end) })
    }

    fn int_token(&mut self) -> PResult<IntLit> {
        let t = self.expect(TokenKind::Int, "eine ganze Zahl")?;
        Ok(IntLit { text: self.text_of(t).to_string(), span: self.span(t.start, t.end) })
    }

    /// `int_lit := INT | HEX | BIN | OCT`
    fn parse_int_lit(&mut self) -> PResult<IntLit> {
        self.cover("int_lit");
        if matches!(self.kind(), TokenKind::Int | TokenKind::Hex | TokenKind::Bin | TokenKind::Oct) {
            let t = self.bump();
            Ok(IntLit { text: self.text_of(t).to_string(), span: self.span(t.start, t.end) })
        } else {
            Err(self.error_here("eine ganze Zahl"))
        }
    }

    /// `number := int_lit | FLOAT`
    fn parse_number(&mut self) -> PResult<Number> {
        self.cover("number");
        if self.at(TokenKind::Float) {
            let t = self.bump();
            Ok(Number::Float(FloatLit { text: self.text_of(t).to_string(), span: self.span(t.start, t.end) }))
        } else {
            self.parse_int_lit().map(Number::Int)
        }
    }

    /// `duration_lit := DURATION`
    fn parse_duration_lit(&mut self) -> PResult<DurationLit> {
        self.cover("duration_lit");
        let t = self.expect(TokenKind::Duration, "eine Dauer wie `200 ms`")?;
        Ok(DurationLit { ns: t.value, span: self.span(t.start, t.end) })
    }

    // ------------------------------------------------------------ Gemeinsame Produktionen

    /// `[ "with" attr { "," attr } ]`
    fn parse_with_attrs(&mut self) -> PResult<Vec<Attr>> {
        self.cover("with_attrs");
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
        self.cover("attr");
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
            "polling" => {
                self.expect_word("unchecked")?;
                AttrKind::PollingUnchecked
            }
            "fault_is_fail" => AttrKind::FaultIsFail(self.parse_bool_word()?),
            other => {
                return Err(self.error_at(
                    name_tok,
                    format!("unbekanntes Attribut `{other}`"),
                    Some("Attribute: safe max_age rate max_rate capacity framing overflow wake jitter max_slew debounce capacity_bytes expect_len irreversible label display group doc budget polling fault_is_fail"),
                ));
            }
        };
        Ok(Attr { kind, span: self.span_from(start) })
    }

    /// `budget = {ram = 2 KiB, wcet = 20 us}` (7.2).
    fn parse_budget(&mut self) -> PResult<Vec<BudgetItem>> {
        self.cover("budget");
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
        self.cover("budget_item");
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
        self.cover("bool_word");
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
        self.cover("framing");
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
        self.cover("binding");
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
        self.cover("generic_vars");
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
        self.cover("gvar");
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
        self.cover("capability");
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
        self.cover("params");
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
        self.cover("param");
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
        self.cover("param_list");
        self.expect_op("(")?;
        let params = self.parse_params()?;
        self.expect_op(")")?;
        Ok(params)
    }

    /// `"(" [ args ] ")"`
    fn parse_arg_list(&mut self) -> PResult<Vec<Arg>> {
        self.cover("arg_list");
        self.expect_op("(")?;
        let args = self.plain(Self::parse_args)?;
        self.expect_op(")")?;
        Ok(args)
    }

    /// `args := arg { "," arg }` (leer erlaubt; der Aufrufer prueft die Klammern)
    fn parse_args(&mut self) -> PResult<Vec<Arg>> {
        self.cover("args");
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
        self.cover("arg");
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
        self.cover("range");
        let start = self.pos;
        let from = self.parse_const_expr()?;
        self.expect_op("..")?;
        let to = self.parse_const_expr()?;
        Ok(Range { from: Box::new(from), to: Box::new(to), span: self.span_from(start) })
    }

    /// `const_expr := expr` (die Einschraenkung auf Konstanten prueft die Semantik)
    fn parse_const_expr(&mut self) -> PResult<Expr> {
        self.cover("const_expr");
        self.parse_expr()
    }

    /// `duration_expr := expr`
    fn parse_duration_expr(&mut self) -> PResult<Expr> {
        self.cover("duration_expr");
        self.parse_expr()
    }

    /// `pattern`
    fn parse_pattern(&mut self) -> PResult<Pattern> {
        self.cover("pattern");
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
        self.cover("unit_lit");
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
        self.cover("unit_expr");
        let start = self.pos;
        let (first, mut rest) = if self.at(TokenKind::Int) && self.text() == "1" {
            let one = self.bump();
            let one_term = UnitTerm { name: None, exponent: None, span: self.span(one.start, one.end) };
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
        self.cover("unit_name");
        if matches!(self.kind(), TokenKind::Ident | TokenKind::UpperIdent | TokenKind::TypeIdent) {
            let t = self.bump();
            return Ok(self.ident_of(t));
        }
        Err(self.error_here("einen Einheitennamen wie `psi`, `V` oder `KiB`"))
    }

    fn parse_unit_term(&mut self, compact: bool) -> PResult<UnitTerm> {
        self.cover("unit_term");
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
        self.cover("unit_tuple");
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
        self.cover("snippet_items");
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
                    self.recover_item(start);
                }
            }
        }
        items
    }

    fn parse_snippet_item(&mut self) -> PResult<SnippetItem> {
        self.cover("snippet_item");
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
                "sequence" => {
                    let (timeout, items) = self.parse_sequence_block()?;
                    return Ok(SnippetItem::Sequence(timeout, items));
                }
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
