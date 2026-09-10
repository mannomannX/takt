//! Spaltenausrichtung und Endausgabe: Laufgruppen gleichartiger Nachbarzeilen
//! bekommen gemeinsame Zellbreiten (wie gofmts tabwriter), nachgestellte
//! Kommentare stehen in aufeinanderfolgenden Zeilen in einer Spalte.

use super::emit::{Kind, Line};

/// Setzt die Zeilen zum fertigen Text zusammen.
pub(super) fn render(lines: &[Line]) -> String {
    let texts = align_cells(lines);
    let texts = align_comments(lines, texts);
    let mut out = String::new();
    for text in texts {
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out
}

/// Zellen zu Text; innerhalb einer Laufgruppe mit gemeinsamen Breiten.
fn align_cells(lines: &[Line]) -> Vec<String> {
    let mut texts = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let run_end = run_end(lines, i);
        let widths = if lines[i].kind.aligns() { column_widths(&lines[i..run_end]) } else { Vec::new() };
        for line in &lines[i..run_end] {
            texts.push(join_cells(line, &widths));
        }
        i = run_end;
    }
    texts
}

/// Ende der Laufgruppe, die bei `start` beginnt (mindestens `start + 1`).
fn run_end(lines: &[Line], start: usize) -> usize {
    let first = &lines[start];
    if !first.kind.aligns() {
        return start + 1;
    }
    let mut end = start + 1;
    while end < lines.len() {
        let l = &lines[end];
        if l.kind != first.kind || l.depth != first.depth || l.cont.is_some() {
            break;
        }
        end += 1;
    }
    end
}

/// Breite je Spalte; wie bei gofmts tabwriter zaehlt eine Zelle nur, wenn in
/// ihrer Zeile noch eine Zelle folgt (die letzte Zelle wird nie aufgefuellt).
fn column_widths(run: &[Line]) -> Vec<usize> {
    let cells = run.iter().map(|l| l.cells.len()).max().unwrap_or(0);
    (0..cells)
        .map(|c| run.iter().filter(|l| l.cells.len() > c + 1).map(|l| width(&l.cells[c])).max().unwrap_or(0))
        .collect()
}

/// Zellen mit Auffuellung; die letzte Zelle einer Zeile wird nicht aufgefuellt,
/// eine leere Zelle in einer leeren Spalte laesst keine Luecke.
fn join_cells(line: &Line, widths: &[usize]) -> String {
    let mut s = " ".repeat(line.cont.unwrap_or(line.depth * 4));
    let last = line.cells.len().saturating_sub(1);
    for (c, cell) in line.cells.iter().enumerate() {
        let width = widths.get(c).copied().unwrap_or(0);
        if c > 0 && (width > 0 || !cell.is_empty()) {
            s.push(' ');
        }
        s.push_str(cell);
        if c < last && width > self::width(cell) {
            s.push_str(&" ".repeat(width - self::width(cell)));
        }
    }
    s
}

/// Nachgestellte Kommentare: in Bloecken aufeinanderfolgender Zeilen mit
/// Kommentar (gleiche Einrueckung) in einer Spalte, mindestens zwei Leerzeichen.
fn align_comments(lines: &[Line], texts: Vec<String>) -> Vec<String> {
    let mut out = texts;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].comment.is_none() {
            i += 1;
            continue;
        }
        let key = (lines[i].depth, lines[i].cont.is_some());
        let mut end = i + 1;
        while end < lines.len()
            && lines[end].comment.is_some()
            && (lines[end].depth, lines[end].cont.is_some()) == key
            && lines[end].kind != Kind::Comment
        {
            end += 1;
        }
        let col = (i..end).map(|k| width(out[k].trim_end())).max().unwrap_or(0) + 2;
        for k in i..end {
            let text = out[k].trim_end().to_string();
            let comment = lines[k].comment.as_deref().unwrap_or_default();
            out[k] = if text.is_empty() {
                comment.to_string()
            } else {
                format!("{text}{}{comment}", " ".repeat(col - width(&text)))
            };
        }
        i = end;
    }
    out
}

/// Breite in Zeichen (Strings und Kommentare duerfen Nicht-ASCII enthalten).
fn width(text: &str) -> usize {
    text.chars().count()
}

impl Kind {
    fn aligns(self) -> bool {
        !matches!(self, Kind::Plain | Kind::Comment | Kind::Blank)
    }
}
