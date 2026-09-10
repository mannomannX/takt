//! Ausgabemodell des Formatters: ein Cursor ueber den Tokenstrom, der Zeilen aus
//! Zellen erzeugt und dabei Beiwerk (Kommentare, Leerzeilen) und die
//! Zeilenstruktur des Autors (Fortsetzungszeilen in Klammern) uebernimmt.
//!
//! Die Drucker (`print.rs`) laufen den Baum in Quelltextreihenfolge ab und
//! verbrauchen dabei jedes Token genau einmal; ein Token, das nicht zum Baum
//! passt, ist ein Fehler des Formatters und bricht die Ausgabe ab.

use takt_diag::Diagnostic;

use crate::parser::token_span;
use crate::token::{Token, TokenKind, Tokens, TriviaKind};

/// Art einer Ausgabezeile; gleichartige Nachbarzeilen bilden Laufgruppen fuer
/// die Spaltenausrichtung (`align.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// Keine Spaltenausrichtung, nur der Kommentar.
    Plain,
    Channel,
    Param,
    Var,
    Field,
    Bitfield,
    Variant,
    SystemItem,
    ProfileEntry,
    /// `unit x = …` und `type X = …`
    Alias,
    /// Kommentar auf eigener Zeile.
    Comment,
    /// Leerzeile.
    Blank,
}

/// Eine Ausgabezeile vor der Ausrichtung.
#[derive(Clone, Debug)]
pub(super) struct Line {
    /// Einrueckungstiefe (4 Leerzeichen je Stufe).
    pub depth: usize,
    /// Fortsetzungszeile: absolute Spalte statt Tiefe.
    pub cont: Option<usize>,
    pub kind: Kind,
    /// Zellen; Ausrichtungspunkte liegen zwischen ihnen.
    pub cells: Vec<String>,
    /// Nachgestellter Kommentar.
    pub comment: Option<String>,
}

impl Line {
    fn new(depth: usize, cont: Option<usize>, kind: Kind) -> Self {
        Line { depth, cont, kind, cells: vec![String::new()], comment: None }
    }

    /// Breite des Inhalts in Zeichen, ohne Kommentar, mit Einrueckung.
    pub fn width(&self) -> usize {
        let indent = self.cont.unwrap_or(self.depth * 4);
        indent + self.cells.iter().map(|c| c.chars().count()).sum::<usize>() + self.cells.len().saturating_sub(1)
    }
}

/// Offene Klammer: wo Fortsetzungszeilen hingehoeren.
struct Bracket {
    /// Index des oeffnenden Tokens.
    token: usize,
    /// Spalte hinter der Klammer (Ausrichtung nach L7).
    after: usize,
    /// Einrueckung der Zeile, die die Klammer oeffnete (fuer haengende Form und
    /// die schliessende Klammer auf eigener Zeile).
    base: usize,
}

pub(super) struct Emitter<'t, 's> {
    pub toks: &'t Tokens<'s>,
    pos: usize,
    pub lines: Vec<Line>,
    cur: Option<Line>,
    pub depth: usize,
    brackets: Vec<Bracket>,
    /// Nach `INDENT`, bis die erste Zeile des Blocks steht: keine Leerzeile.
    at_block_start: bool,
    /// Das naechste Token ohne Leerzeichen anfuegen.
    glue: bool,
    /// Art der naechsten Zeile (gesetzt, bevor ihr erstes Token verbraucht ist).
    next_kind: Kind,
    /// Leerzeile aus dem Beiwerk eines `DEDENT`: gehoert hinter den Block.
    pending_blank: bool,
    pub errors: Vec<Diagnostic>,
}

impl<'t, 's> Emitter<'t, 's> {
    pub fn new(toks: &'t Tokens<'s>) -> Self {
        Emitter {
            toks,
            pos: 0,
            lines: Vec::new(),
            cur: None,
            depth: 0,
            brackets: Vec::new(),
            at_block_start: true,
            glue: false,
            next_kind: Kind::Plain,
            pending_blank: false,
            errors: Vec::new(),
        }
    }

    // ------------------------------------------------------------ Cursor

    pub fn peek(&self) -> &'t Token {
        &self.toks.tokens[self.pos.min(self.toks.tokens.len() - 1)]
    }

    pub fn peek_text(&self) -> &'s str {
        self.toks.text(self.peek())
    }

    pub fn at_kind(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }

    pub fn at_text(&self, text: &str) -> bool {
        self.peek_text() == text
    }

    fn fail(&mut self, message: String) {
        if self.errors.is_empty() {
            let t = self.peek();
            self.errors.push(Diagnostic::error("F", token_span(t), message));
        }
    }

    /// Verbraucht das naechste Token und liefert es; Beiwerk und Zeilenstruktur
    /// werden dabei uebernommen.
    fn consume(&mut self) -> &'t Token {
        let idx = self.pos.min(self.toks.tokens.len() - 1);
        let t = &self.toks.tokens[idx];
        self.trivia(idx);
        if !self.brackets.is_empty() && !matches!(t.kind, TokenKind::Eof) {
            let prev = &self.toks.tokens[idx - 1];
            if t.line > prev.line {
                self.continuation(idx);
            }
        }
        if t.kind != TokenKind::Eof {
            self.pos = idx + 1;
        }
        t
    }

    /// Verbraucht ein Token und prueft seinen Text.
    fn take(&mut self, expected: &str) -> Option<&'t Token> {
        let t = self.peek();
        if self.toks.text(t) != expected {
            self.fail(format!("Formatter erwartet `{expected}`, findet `{}`", self.toks.text(t)));
            return None;
        }
        Some(self.consume())
    }

    // ------------------------------------------------------------ Schreiben

    fn line_mut(&mut self) -> &mut Line {
        if self.cur.is_none() {
            let kind = std::mem::replace(&mut self.next_kind, Kind::Plain);
            self.cur = Some(Line::new(self.depth, None, kind));
        }
        self.cur.as_mut().expect("Zeile offen")
    }

    fn push_text(&mut self, text: &str, space: bool) {
        let glue = std::mem::take(&mut self.glue);
        let line = self.line_mut();
        let cell = line.cells.last_mut().expect("mindestens eine Zelle");
        if space && !glue && !cell.is_empty() {
            cell.push(' ');
        }
        cell.push_str(text);
    }

    /// Das naechste Token folgt ohne Leerzeichen (nach `(`, `[`, `.`, unaerem `-`).
    pub fn glue(&mut self) {
        self.glue = true;
    }

    /// Token ohne Leerzeichen davor: `(`, `,`, `.`, `]`, `^`.
    pub fn op(&mut self, expected: &str) {
        if let Some(t) = self.take(expected) {
            self.push_token(t, false);
        }
    }

    /// Token mit einem Leerzeichen davor: Woerter, binaere Operatoren.
    pub fn sp(&mut self, expected: &str) {
        if let Some(t) = self.take(expected) {
            self.push_token(t, true);
        }
    }

    /// Beliebiges Token (Name, Literal) mit oder ohne Leerzeichen davor.
    pub fn any(&mut self, space: bool) {
        let t = self.consume();
        self.push_token(t, space);
    }

    fn push_token(&mut self, t: &'t Token, space: bool) {
        let text = self.toks.text(t);
        match text {
            "(" | "[" | "{" => {
                self.push_text(text, space);
                let line = self.line_mut();
                let after = line.width();
                let base = line.cont.unwrap_or(line.depth * 4);
                self.brackets.push(Bracket { token: self.pos - 1, after, base });
            }
            ")" | "]" | "}" => {
                self.brackets.pop();
                self.push_text(text, space);
            }
            _ if t.kind == TokenKind::Duration => {
                // Zahl, genau ein Leerzeichen, Suffix
                let mut parts = text.split_whitespace();
                let number = parts.next().unwrap_or_default();
                let suffix = parts.next().unwrap_or_default();
                self.push_text(&format!("{number} {suffix}"), space);
            }
            _ => self.push_text(text, space),
        }
    }

    /// Fuellt die aktuelle Zelle auf eine Mindestbreite auf.
    pub fn pad(&mut self, width: usize) {
        let cell = self.line_mut().cells.last_mut().expect("mindestens eine Zelle");
        while cell.chars().count() < width {
            cell.push(' ');
        }
    }

    /// Beginnt eine neue Zelle (Ausrichtungspunkt).
    pub fn cell(&mut self) {
        self.line_mut().cells.push(String::new());
    }

    /// Art der Zeile, die mit dem naechsten Token beginnt. Steht die Deklaration
    /// einzeilig hinter einem Blockkopf, bleibt die Zeile, was der Kopf ist.
    pub fn kind(&mut self, kind: Kind) {
        if self.cur.is_none() {
            self.next_kind = kind;
        }
    }

    fn push_blank(&mut self) {
        if self.lines.is_empty() || self.lines.last().is_some_and(|l| l.kind == Kind::Blank) {
            return;
        }
        self.close_line();
        self.lines.push(Line::new(0, None, Kind::Blank));
    }

    fn close_line(&mut self) {
        if let Some(line) = self.cur.take() {
            self.lines.push(line);
            self.at_block_start = false;
        }
    }

    /// Verbraucht `NEWLINE` und schliesst die Zeile.
    pub fn newline(&mut self) {
        if !self.at_kind(TokenKind::Newline) {
            self.fail(format!("Formatter erwartet Zeilenende, findet `{}`", self.peek_text()));
            return;
        }
        self.consume();
        self.close_line();
    }

    pub fn indent(&mut self) {
        if !self.at_kind(TokenKind::Indent) {
            self.fail("Formatter erwartet Einrueckung".into());
            return;
        }
        self.consume();
        self.depth += 1;
        self.at_block_start = true;
    }

    pub fn dedent(&mut self) {
        if !self.at_kind(TokenKind::Dedent) {
            self.fail("Formatter erwartet Blockende".into());
            return;
        }
        self.consume();
        self.depth = self.depth.saturating_sub(1);
    }

    /// Verbraucht `EOF` samt Beiwerk am Dateiende.
    pub fn finish(&mut self) {
        self.close_line();
        self.consume();
        self.close_line();
    }

    // ------------------------------------------------------------ Beiwerk und Zeilenstruktur

    /// Kommentare und Leerzeilen vor Token `idx`.
    fn trivia(&mut self, idx: usize) {
        let t = &self.toks.tokens[idx];
        let items = self.toks.trivia_of(t);
        if items.is_empty() {
            if self.pending_blank && !matches!(t.kind, TokenKind::Dedent | TokenKind::Eof) {
                self.pending_blank = false;
                self.push_blank();
            }
            return;
        }
        let prev_end = if idx == 0 { 0 } else { self.toks.tokens[idx - 1].end as usize };
        // Tiefe fuer Kommentarzeilen: vor einem DEDENT zwischen innen und aussen
        // nach der eigenen Spalte, sonst die aktuelle Tiefe.
        let dedents = self.toks.tokens[idx..].iter().take_while(|t| t.kind == TokenKind::Dedent).count();
        let (depth_min, depth_max) = match t.kind {
            TokenKind::Dedent => (self.depth.saturating_sub(dedents), self.depth),
            TokenKind::Indent => (self.depth, self.depth + 1),
            _ => (self.depth, self.depth),
        };
        let ends_block = matches!(t.kind, TokenKind::Dedent | TokenKind::Eof);
        if self.pending_blank && !ends_block {
            self.pending_blank = false;
            self.push_blank();
        }
        let mut seen_comment = false;
        for (k, item) in items.iter().enumerate() {
            match item.kind {
                TriviaKind::Comment => {
                    seen_comment = true;
                    let raw = &self.toks.src[item.start as usize..item.end as usize];
                    let text = comment_text(raw);
                    let trailing = !self.toks.src[prev_end..item.start as usize].contains('\n');
                    if trailing && self.cur.is_some() {
                        self.line_mut().comment = Some(text);
                        continue;
                    }
                    if trailing && self.lines.last().is_some_and(|l| l.comment.is_none()) && idx > 0 {
                        // Kommentar hinter einem Token, dessen Zeile schon geschlossen ist (z. B. hinter NEWLINE)
                        self.lines.last_mut().expect("Zeile").comment = Some(text);
                        continue;
                    }
                    if self.brackets.is_empty() {
                        let line_start = self.toks.src[..item.start as usize].rfind('\n').map_or(0, |p| p + 1);
                        let own = (item.start as usize - line_start) / 4;
                        let depth = own.clamp(depth_min, depth_max);
                        let mut line = Line::new(depth, None, Kind::Comment);
                        line.cells[0] = text;
                        self.lines.push(line);
                        self.at_block_start = false;
                    } else {
                        // Kommentar in Klammern: auf eigener Fortsetzungszeile
                        let col = self.continuation_col(idx);
                        self.close_line();
                        let mut line = Line::new(self.depth, Some(col), Kind::Comment);
                        line.cells[0] = text;
                        self.lines.push(line);
                    }
                }
                TriviaKind::BlankLine => {
                    let block_start = (self.at_block_start || t.kind == TokenKind::Indent) && !seen_comment;
                    if !self.brackets.is_empty() || block_start {
                        continue;
                    }
                    if ends_block && !items[k + 1..].iter().any(|i| i.kind == TriviaKind::Comment) {
                        // zwischen Blockende und naechster Deklaration: hinter den Block
                        self.pending_blank = t.kind == TokenKind::Dedent;
                        continue;
                    }
                    self.push_blank();
                }
            }
        }
    }

    /// Spalte einer Fortsetzungszeile vor Token `idx` (Regeln aus format.md F1).
    fn continuation_col(&self, idx: usize) -> usize {
        let b = self.brackets.last().expect("in Klammern");
        let text = self.toks.text(&self.toks.tokens[idx]);
        if matches!(text, ")" | "]" | "}") {
            return b.base;
        }
        let bracket_line = self.toks.tokens[b.token].line;
        let first_after = &self.toks.tokens[b.token + 1];
        if first_after.line > bracket_line {
            // Klammer stand am Zeilenende: haengende Einrueckung
            return b.base + 4;
        }
        b.after
    }

    /// Der Autor hat in Klammern umgebrochen: Zeile schliessen, Fortsetzung oeffnen.
    fn continuation(&mut self, idx: usize) {
        let col = self.continuation_col(idx);
        if let Some(line) = self.cur.as_mut() {
            line.kind = Kind::Plain;
        }
        self.close_line();
        self.cur = Some(Line::new(self.depth, Some(col), Kind::Plain));
    }
}

/// Kommentartext: `#`, mindestens ein Leerzeichen, kein Leerraum am Ende.
fn comment_text(raw: &str) -> String {
    let body = raw.strip_prefix('#').unwrap_or(raw).trim_end();
    if body.is_empty() {
        "#".to_string()
    } else if body.starts_with(char::is_whitespace) {
        format!("#{body}")
    } else {
        format!("# {body}")
    }
}
