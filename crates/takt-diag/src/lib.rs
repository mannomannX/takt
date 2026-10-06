//! Diagnosen: ein Typ fuer Tokenizer, Parser, Sema, MIR und Runtime.
//!
//! Jede Diagnose traegt Schwere, Code (lexer.md L9 `E_…`, Parser `P`, Pruefungen
//! `SC-n` aus Referenz 10), Position als Byte-Bereich, Meldung und, wo moeglich,
//! einen Vorschlag. Zeile und Spalte rechnet die `SourceMap`; die Politik
//! (Warnungen eskalieren, Zertifizierungsmodus) wirkt beim Sammeln, nicht in der
//! Pruefung.

use std::collections::HashSet;
use std::fmt;
use std::panic::Location;

pub mod stack;

/// Schwere einer Diagnose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Hinweis, aendert nichts am Ergebnis.
    Note,
    /// Warnung; per Politik zum Fehler eskalierbar.
    Warning,
    /// Fehler; das Programm wird nicht angenommen.
    Error,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Note => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// Sprachstufe, ab der ein Konstrukt ausgefuehrt wird (Referenz Anhang, plan.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Stage {
    V1_1,
    V1_2,
    V2,
}

impl Stage {
    /// Name wie in der Referenz.
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::V1_1 => "v1.1",
            Stage::V1_2 => "v1.2",
            Stage::V2 => "v2",
        }
    }
}

/// Kennung einer Datei in der `SourceMap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FileId(pub u32);

/// Byte-Bereich in einer Datei.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    /// Datei.
    pub file: FileId,
    /// Offset des ersten Bytes.
    pub start: u32,
    /// Offset hinter dem letzten Byte.
    pub end: u32,
}

impl Span {
    /// Bereich in der Datei 0 (Einzeldateien; die `SourceMap` des Aufrufers
    /// ordnet spaeter zu).
    pub fn new(start: u32, end: u32) -> Self {
        Span { file: FileId(0), start, end }
    }
}

/// Eine Diagnose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Schwere.
    pub severity: Severity,
    /// Code (`E_BOM`, `P`, `SC-50`, …).
    pub code: &'static str,
    /// Was erwartet wurde und was da war.
    pub message: String,
    /// Position.
    pub span: Span,
    /// Handlungsvorschlag.
    pub suggestion: Option<String>,
    /// Weitere Stellen mit Erklaerung.
    pub notes: Vec<(Span, String)>,
    /// Konstrukt einer spaeteren Stufe („ab v1.1").
    pub stage: Option<Stage>,
    /// Die Stelle im Quelltext des Compilers, an der die Diagnose entstand
    /// (`#[track_caller]`): Der Diagnosekatalog haelt jede Meldestelle
    /// gegen einen Fall im Korpus (13.8).
    pub origin: &'static Location<'static>,
}

impl Diagnostic {
    /// Neue Diagnose der angegebenen Schwere.
    #[track_caller]
    pub fn new(severity: Severity, code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity,
            code,
            message: message.into(),
            span,
            suggestion: None,
            notes: Vec::new(),
            stage: None,
            origin: Location::caller(),
        }
    }

    /// Fehler.
    #[track_caller]
    pub fn error(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, span, message)
    }

    /// Warnung.
    #[track_caller]
    pub fn warning(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, span, message)
    }

    /// Mit Vorschlag.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Mit einer weiteren Stelle.
    pub fn with_note(mut self, span: Span, text: impl Into<String>) -> Self {
        self.notes.push((span, text.into()));
        self
    }

    /// Mit Stufe.
    pub fn with_stage(mut self, stage: Stage) -> Self {
        self.stage = Some(stage);
        self
    }

    /// Ordnet die Diagnose (und ihre Notizen) einer Datei zu.
    pub fn in_file(mut self, file: FileId) -> Self {
        self.span.file = file;
        for (span, _) in &mut self.notes {
            span.file = file;
        }
        self
    }

    /// Ist es ein Fehler?
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    /// Kurzform ohne Zeile und Spalte (dafuer `SourceMap::render_line`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity.label(), self.code, self.message)?;
        if let Some(s) = &self.suggestion {
            write!(f, " ({s})")?;
        }
        Ok(())
    }
}

/// Politik der Auswertung (Referenz 10 und 2.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Jede Warnung ist ein Fehler.
    pub warnings_as_errors: bool,
    /// Zertifizierungsmodus: die fehlende Edition (SC-49) ist ein Fehler,
    /// und jede unbewiesene implizite Pruefung (SC-4, SC-24, 3.4) — die meldet
    /// die Analyse selbst, weil nur sie die Stellen des Nutzers kennt.
    pub certification: bool,
    /// Abnahme durch Szenarien (`takt test`): Ein `hw`-Input ohne
    /// `sim`-Quelle (SC-13) ist ein Fehler, weil kein Stimulus ihn treibt
    /// (8.3, Festlegung 6).
    pub tested: bool,
}

impl Policy {
    /// Wendet die Politik auf eine Diagnose an.
    pub fn apply(&self, mut diag: Diagnostic) -> Diagnostic {
        let escalate = diag.severity == Severity::Warning
            && (self.warnings_as_errors
                || (self.certification && diag.code == "SC-49")
                || (self.tested && diag.code == "SC-13"));
        if escalate {
            diag.severity = Severity::Error;
        }
        diag
    }
}

/// Sammelt Diagnosen unter einer Politik.
#[derive(Debug, Default)]
pub struct Sink {
    /// Politik.
    pub policy: Policy,
    /// Gesammelte Diagnosen in Reihenfolge der Meldung.
    pub diagnostics: Vec<Diagnostic>,
}

impl Sink {
    /// Neuer Sammler.
    pub fn new(policy: Policy) -> Self {
        Sink { policy, diagnostics: Vec::new() }
    }

    /// Meldet eine Diagnose (mit Politik).
    pub fn report(&mut self, diag: Diagnostic) {
        self.diagnostics.push(self.policy.apply(diag));
    }

    /// Meldet mehrere.
    pub fn extend(&mut self, diags: impl IntoIterator<Item = Diagnostic>) {
        for d in diags {
            self.report(d);
        }
    }

    /// Gibt es Fehler?
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// Nach Position sortiert (Datei, Offset), stabil; gleiche Diagnosen
    /// (Code, Position, Meldung) nur einmal, etwa aus Instanzen einer Vorlage.
    pub fn sorted(mut self) -> Vec<Diagnostic> {
        self.diagnostics.sort_by_key(|d| (d.span.file.0, d.span.start));
        let mut seen = HashSet::new();
        self.diagnostics.retain(|d| seen.insert((d.code, d.span, d.message.clone())));
        self.diagnostics
    }
}

struct SourceFile {
    name: String,
    text: String,
    line_starts: Vec<u32>,
}

/// Dateien mit Text, fuer Zeile/Spalte und Quellauszuege.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    /// Leere Karte.
    pub fn new() -> Self {
        Self::default()
    }

    /// Karte mit einer Datei (Kennung 0).
    pub fn single(name: impl Into<String>, text: impl Into<String>) -> Self {
        let mut map = Self::new();
        map.add(name, text);
        map
    }

    /// Fuegt eine Datei hinzu.
    pub fn add(&mut self, name: impl Into<String>, text: impl Into<String>) -> FileId {
        let text = text.into();
        let mut line_starts = vec![0u32];
        line_starts.extend(text.match_indices('\n').map(|(i, _)| i as u32 + 1));
        self.files.push(SourceFile { name: name.into(), text, line_starts });
        FileId(self.files.len() as u32 - 1)
    }

    /// Name der Datei.
    pub fn name(&self, file: FileId) -> &str {
        self.files.get(file.0 as usize).map_or("?", |f| f.name.as_str())
    }

    /// Text der Datei.
    pub fn text(&self, file: FileId) -> &str {
        self.files.get(file.0 as usize).map_or("", |f| f.text.as_str())
    }

    /// Zeile und Spalte (beide ab 1, Spalte in Zeichen) des Anfangs eines Bereichs.
    pub fn line_col(&self, span: Span) -> (u32, u32) {
        let Some(f) = self.files.get(span.file.0 as usize) else { return (0, 0) };
        let offset = (span.start as usize).min(f.text.len());
        let line = f.line_starts.partition_point(|&s| s as usize <= offset);
        let line_start = f.line_starts[line - 1] as usize;
        let rel = offset - line_start;
        // Ein Anfang mitten in einem Mehrbytezeichen zaehlt als dieses Zeichen.
        let col = f.text[line_start..].char_indices().take_while(|&(i, c)| i + c.len_utf8() <= rel).count() + 1;
        (line as u32, col as u32)
    }

    /// Text einer Zeile (ab 1), ohne Zeilenende.
    pub fn line_text(&self, file: FileId, line: u32) -> &str {
        let Some(f) = self.files.get(file.0 as usize) else { return "" };
        let Some(&start) = f.line_starts.get(line.saturating_sub(1) as usize) else { return "" };
        let rest = &f.text[start as usize..];
        rest.split_once('\n').map_or(rest, |(l, _)| l).trim_end_matches('\r')
    }

    /// Einzeilige Form: `datei:zeile:spalte: severity[code]: meldung (vorschlag)`.
    pub fn render_line(&self, diag: &Diagnostic) -> String {
        let (line, col) = self.line_col(diag.span);
        format!("{}:{line}:{col}: {diag}", self.name(diag.span.file))
    }

    /// Mehrzeilige Form mit Quellauszug und Markierung.
    pub fn render(&self, diag: &Diagnostic) -> String {
        let mut out = format!("{}[{}]: {}\n", diag.severity.label(), diag.code, diag.message);
        self.render_span(&mut out, diag.span, None);
        for (span, text) in &diag.notes {
            self.render_span(&mut out, *span, Some(text));
        }
        if let Some(stage) = diag.stage {
            out.push_str(&format!("  = ab {}\n", stage.as_str()));
        }
        if let Some(s) = &diag.suggestion {
            out.push_str(&format!("  = Vorschlag: {s}\n"));
        }
        out
    }

    fn render_span(&self, out: &mut String, span: Span, label: Option<&str>) {
        let (line, col) = self.line_col(span);
        let name = self.name(span.file);
        let text = self.line_text(span.file, line);
        let width = line.to_string().len();
        let pad = " ".repeat(width);
        out.push_str(&format!("{pad}--> {name}:{line}:{col}\n{pad} |\n{line} | {text}\n"));
        let len = self
            .text(span.file)
            .get(span.start as usize..span.end as usize)
            .map_or(1, |s| s.split('\n').next().unwrap_or("").chars().count().max(1));
        let prefix: String =
            text.chars().take(col.saturating_sub(1) as usize).map(|c| if c == '\t' { c } else { ' ' }).collect();
        let underline = format!("{prefix}{}", "^".repeat(len));
        match label {
            Some(l) => out.push_str(&format!("{pad} | {underline} {l}\n")),
            None => out.push_str(&format!("{pad} | {underline}\n")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column_are_one_based_and_count_chars() {
        let map = SourceMap::single("a.takt", "x = 1\nnäme = 2\n");
        assert_eq!(map.line_col(Span::new(0, 1)), (1, 1));
        assert_eq!(map.line_col(Span::new(6, 10)), (2, 1));
        assert_eq!(map.line_col(Span::new(13, 14)), (2, 7), "Spalte in Zeichen, nicht Bytes");
        assert_eq!(map.line_text(FileId(0), 2), "näme = 2");
    }

    #[test]
    fn rendering_shows_source_and_suggestion() {
        let map = SourceMap::single("ctrl.takt", "record H:\n    valid : bool\n");
        let d = Diagnostic::error("SC-50", Span::new(14, 19), "`valid` ist als Feldname verboten")
            .with_suggestion("Feld umbenennen, etwa `is_valid`");
        let text = map.render(&d);
        assert!(text.starts_with("error[SC-50]: `valid` ist als Feldname verboten\n"), "{text}");
        assert!(text.contains(" --> ctrl.takt:2:5\n"), "{text}");
        assert!(text.contains("2 |     valid : bool\n  |     ^^^^^\n"), "{text}");
        assert!(text.ends_with("  = Vorschlag: Feld umbenennen, etwa `is_valid`\n"), "{text}");
        assert_eq!(
            map.render_line(&d),
            "ctrl.takt:2:5: error[SC-50]: `valid` ist als Feldname verboten (Feld umbenennen, etwa `is_valid`)"
        );
    }

    #[test]
    fn odd_spans_do_not_panic_and_underline_one_line() {
        let map = SourceMap::single("a.takt", "\tvar näme = 1\nnext\n");
        let inside_char = Span::new(6, 7);
        assert_eq!(map.line_col(inside_char), (1, 7), "Offset in einem Mehrbytezeichen");
        assert!(!map.render(&Diagnostic::error("P", inside_char, "x")).is_empty());
        let multi = Span::new(1, 18);
        let text = map.render(&Diagnostic::error("P", multi, "x"));
        assert!(text.contains("1 | \tvar näme = 1\n  | \t^^^^^^^^^^^^\n"), "{text}");
        let beyond = Span::new(100, 200);
        assert_eq!(map.line_col(beyond), (3, 1));
        assert!(map.render(&Diagnostic::error("P", beyond, "x")).contains("3 | \n"));
        assert_eq!(map.line_col(Span::new(0, 0)), (1, 1));
        assert_eq!(SourceMap::new().line_col(Span::new(3, 4)), (0, 0));
        assert_eq!(SourceMap::new().line_text(FileId(7), 1), "");
    }

    #[test]
    fn policy_escalates_warnings() {
        let w = Diagnostic::warning("SC-49", Span::new(0, 0), "Edition fehlt");
        assert_eq!(Policy::default().apply(w.clone()).severity, Severity::Warning);
        assert_eq!(Policy { certification: true, ..Default::default() }.apply(w.clone()).severity, Severity::Error);
        let other = Diagnostic::warning("SC-14", Span::new(0, 0), "x");
        assert_eq!(
            Policy { certification: true, ..Default::default() }.apply(other.clone()).severity,
            Severity::Warning
        );
        assert_eq!(Policy { warnings_as_errors: true, ..Default::default() }.apply(other).severity, Severity::Error);
        let mut sink = Sink::new(Policy { certification: true, ..Default::default() });
        sink.report(w);
        assert!(sink.has_errors());
    }

    #[test]
    fn sorted_drops_duplicates() {
        let mut sink = Sink::new(Policy::default());
        sink.report(Diagnostic::error("SC-3", Span::new(5, 6), "x"));
        sink.report(Diagnostic::error("SC-3", Span::new(1, 2), "y"));
        sink.report(Diagnostic::error("SC-3", Span::new(5, 6), "x"));
        sink.report(Diagnostic::error("SC-2", Span::new(5, 6), "x"));
        let out = sink.sorted();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].span, Span::new(1, 2));
    }

    #[test]
    fn sorted_drops_duplicates_that_are_not_neighbours() {
        // Zwei Instanzen einer Vorlage melden A und B an denselben Stellen.
        let mut sink = Sink::new(Policy::default());
        for _ in 0..2 {
            sink.report(Diagnostic::error("SC-3", Span::new(5, 6), "a"));
            sink.report(Diagnostic::error("SC-7", Span::new(5, 6), "b"));
        }
        let out = sink.sorted();
        let shown: Vec<_> = out.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(shown, [("SC-3", "a"), ("SC-7", "b")], "Reihenfolge der Meldung bleibt");
    }

    #[test]
    fn a_column_inside_a_multibyte_char_is_the_column_of_that_char() {
        let map = SourceMap::single("a.takt", "\tvar näme = 1\nnext\n");
        assert_eq!(map.line_col(Span::new(7, 8)), (1, 7), "zweites Byte von ä");
        assert_eq!(map.line_col(Span::new(8, 9)), (1, 8), "m hinter ä");
        let crlf = SourceMap::single("a.takt", "a = 1\r\nbä = 2\r\n");
        assert_eq!(crlf.line_col(Span::new(7, 8)), (2, 1));
        assert_eq!(crlf.line_col(Span::new(10, 11)), (2, 3), "Spalte in Zeichen auch nach CRLF");
        assert_eq!(crlf.line_text(FileId(0), 1), "a = 1");
        // Die BOM ist ein Zeichen (L1.2 zaehlt Zeichen) und nimmt Spalte 1 ein.
        let bom = SourceMap::single("a.takt", "\u{feff}x = 1\n");
        assert_eq!(bom.line_col(Span::new(3, 4)), (1, 2));
        let reversed = Span::new(5, 2);
        assert_eq!(map.line_col(reversed), (1, 6), "der Anfang zaehlt");
        assert!(map.render(&Diagnostic::error("P", reversed, "x")).contains("  | \t    ^\n"));
    }

    #[test]
    fn notes_in_another_file_and_the_stage_are_rendered() {
        let mut map = SourceMap::new();
        let main = map.add("main.takt", "import lib\nx = f()\n");
        let lib = map.add("lib.takt", "fn f() -> int:\n    return 1\n");
        let d = Diagnostic::error("SC-9", Span::new(15, 18), "Aufruf ab v1.1")
            .with_note(Span::new(3, 4), "hier deklariert")
            .with_stage(Stage::V1_1)
            .with_suggestion("Stufe heben")
            .in_file(main);
        // in_file setzt die Datei auch an der Notiz; die zweite Datei kommt per Hand.
        assert_eq!(d.notes[0].0.file, main);
        let d = Diagnostic { notes: vec![(Span { file: lib, start: 3, end: 4 }, "hier deklariert".into())], ..d };
        assert_eq!(
            map.render(&d),
            "error[SC-9]: Aufruf ab v1.1\n \
             --> main.takt:2:5\n  |\n2 | x = f()\n  |     ^^^\n \
             --> lib.takt:1:4\n  |\n1 | fn f() -> int:\n  |    ^ hier deklariert\n  \
             = ab v1.1\n  = Vorschlag: Stufe heben\n"
        );
        assert_eq!(map.render_line(&d), "main.takt:2:5: error[SC-9]: Aufruf ab v1.1 (Stufe heben)");
    }

    #[test]
    fn sorted_orders_by_file_before_offset() {
        let mut sink = Sink::new(Policy::default());
        sink.report(Diagnostic::error("SC-1", Span { file: FileId(1), start: 0, end: 1 }, "b"));
        sink.report(Diagnostic::error("SC-1", Span { file: FileId(0), start: 9, end: 10 }, "a"));
        let files: Vec<_> = sink.sorted().iter().map(|d| d.span.file.0).collect();
        assert_eq!(files, [0, 1]);
    }

    #[test]
    fn only_warnings_escalate_under_every_policy() {
        let policies = [
            Policy::default(),
            Policy { warnings_as_errors: true, ..Default::default() },
            Policy { certification: true, ..Default::default() },
            Policy { tested: true, ..Default::default() },
            Policy { warnings_as_errors: true, certification: true, tested: true },
        ];
        // Erwartete Schwere je Politik fuer (Schwere, Code).
        use Severity::{Error as E, Note as N, Warning as W};
        let table = [
            (N, "SC-49", [N, N, N, N, N]),
            (N, "SC-13", [N, N, N, N, N]),
            (E, "SC-14", [E, E, E, E, E]),
            (W, "SC-14", [W, E, W, W, E]),
            (W, "SC-49", [W, E, E, W, E]),
            (W, "SC-13", [W, E, W, E, E]),
        ];
        for (severity, code, want) in table {
            for (policy, want) in policies.iter().zip(want) {
                let d = Diagnostic::new(severity, code, Span::new(0, 0), "x");
                assert_eq!(policy.apply(d).severity, want, "{severity:?} {code} unter {policy:?}");
            }
        }
    }
}
