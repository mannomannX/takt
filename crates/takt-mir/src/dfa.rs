//! Der Produkt-DFA der Muster eines Handler-Blocks (8.7, 11.2).
//!
//! **Wofuer er da ist — und wofuer nicht.** 8.7 sagt, Matching und
//! Extraktion seien „ein einziger Vorwaertsdurchlauf": Die
//! Mehrdeutigkeitsregel (auf `int`, `hex`, `float`, `word` folgt ein
//! Literal, dessen erstes Zeichen nicht zur Klasse gehoert) macht jede
//! Capture-Grenze eindeutig, und der Interpreter laeuft darum ohne
//! Backtracking. Ein DFA *je Muster* brauchte es dafuer nicht.
//!
//! Der Nutzen steht in 11.2: „alle Muster der Handler eines Zustands
//! werden zu einem Produkt-DFA vereinigt, sodass jedes Element einmal
//! durchlaufen wird". Und 8.7 rechnet damit: Das Budget eines Zustands
//! mit Handlern ist `CAP * (max_len + max_h cost(h.body))` — `max_len`
//! *einmal*, nicht je Handler. Ohne den Produkt-DFA kostet ein Zustand
//! mit drei Handlern drei Durchlaeufe, und die Kostenrechnung aus 9.4.3
//! waere zu optimistisch.
//!
//! **Was er liefert.** Welches Muster trifft — nicht, was in den
//! Captures steht. Die Extraktion bleibt der Vorwaertsdurchlauf: Ein
//! Automat ueber Zeichenklassen kann die Grenzen nennen, aber nicht die
//! Werte, und eine zweite Maschinerie dafuer waere eine zweite Stelle,
//! an der `int` seine Grenzen bekommt (4.1).
//!
//! **Alphabetklassen.** Bytes, die alle Muster gleich behandeln, teilen
//! eine Spalte (11.2: „typisch 10-25 statt 256"). Die Klassenabbildung
//! ist 256 Byte je Block, die Tabelle `states × classes` — beides geht
//! in `takt size` ein (11.5).

use std::collections::{BTreeMap, BTreeSet};

use crate::pattern::{CaptureKind, Dfa, PatternPiece};

/// Ein Zustand des Automaten waehrend der Konstruktion.
///
/// Die Teilmengenkonstruktion (8.7) bildet Mengen von NFA-Positionen auf
/// DFA-Zustaende ab; eine Position ist `(Muster, Stueck, Versatz im
/// Literal)`.
type Set = BTreeSet<Position>;

/// Eine Stelle in einem Muster.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    /// Welches Muster des Blocks.
    pattern: u32,
    /// Welches Stueck darin.
    piece: u32,
    /// Innerhalb eines Literals: das naechste erwartete Byte.
    offset: u32,
}

/// Baut den Produkt-DFA ueber mehrere Muster (11.2).
///
/// `patterns` sind die Muster der Handler eines Zustands in
/// Quelltextreihenfolge; die Reihenfolge ist die Prioritaet (8.7: „der
/// erste passende Handler gewinnt").
///
/// `None` heisst: Der Automat lohnt nicht oder ist nicht darstellbar —
/// ein Muster mit `str<N>` oder `{_}` endet erst am Folgeliteral, was
/// der Vorwaertsdurchlauf besser kann als eine Tabelle. Der Aufrufer
/// faellt dann auf den Durchlauf je Muster zurueck; das Ergebnis ist
/// dasselbe, nur die Kosten sind hoeher.
pub fn build(patterns: &[&[PatternPiece]]) -> Option<Dfa> {
    if patterns.is_empty() || patterns.len() > 64 {
        return None;
    }
    // Ein offenes Ende (`str`, `{_}`) macht die Sprache zwar nicht
    // irregulaer, aber die Teilmengenkonstruktion muesste das
    // Folgeliteral vorziehen — und genau das ist die Staerke des
    // Vorwaertsdurchlaufs. Der Automat bleibt den geschlossenen Klassen.
    if patterns.iter().any(|p| p.iter().any(is_open)) {
        return None;
    }
    let classes = alphabet(patterns);
    let class_count = classes.iter().copied().max().map_or(1, |m| u32::from(m) + 1);

    let start: Set = (0..patterns.len() as u32).map(|p| Position { pattern: p, piece: 0, offset: 0 }).collect();
    let mut ids: BTreeMap<Set, u32> = BTreeMap::new();
    let mut queue = vec![start.clone()];
    ids.insert(start, 0);
    let mut table: Vec<u32> = Vec::new();
    let mut accept: Vec<u32> = Vec::new();

    // Die Grenze ist die Zahl der Teilmengen, die entstehen koennen;
    // 8.7 sagt, sie sei „durch die Musterlaenge beschraenkt". Die
    // Schranke hier ist grosszuegig und schuetzt vor einem Entwurfsfehler,
    // nicht vor einem gueltigen Muster.
    const MAX_STATES: usize = 4096;
    let mut i = 0;
    while i < queue.len() {
        if queue.len() > MAX_STATES {
            return None;
        }
        let set = queue[i].clone();
        i += 1;
        if set.iter().any(|p| accepts(patterns, *p)) {
            accept.push(u32::try_from(i - 1).ok()?);
        }
        for class in 0..class_count {
            let next = step(patterns, &set, class, &classes);
            let id = match ids.get(&next) {
                Some(id) => *id,
                None => {
                    let id = u32::try_from(queue.len()).ok()?;
                    ids.insert(next.clone(), id);
                    queue.push(next);
                    id
                }
            };
            table.push(id);
        }
    }
    Some(Dfa { classes, class_count, table, accept })
}

/// Endet das Stueck offen — `str<N>` oder `{_}`?
fn is_open(p: &PatternPiece) -> bool {
    matches!(p, PatternPiece::Any | PatternPiece::Capture { kind: CaptureKind::Str(_), .. })
}

/// Die Alphabetklassen: Bytes, die alle Muster gleich behandeln, teilen
/// eine Spalte (11.2).
///
/// Zwei Bytes sind gleich, wenn sie in jedem Literal dieselben Stellen
/// treffen und zu denselben Zeichenklassen gehoeren. Die Signatur eines
/// Bytes ist damit sein Verhalten, und gleiche Signaturen bekommen
/// dieselbe Klasse.
fn alphabet(patterns: &[&[PatternPiece]]) -> Vec<u8> {
    let mut signatures: BTreeMap<Vec<u8>, u8> = BTreeMap::new();
    let mut classes = vec![0u8; 256];
    for (b, slot) in classes.iter_mut().enumerate() {
        let byte = b as u8;
        let mut sig = Vec::new();
        for (pi, pieces) in patterns.iter().enumerate() {
            for (si, piece) in pieces.iter().enumerate() {
                match piece {
                    PatternPiece::Text(t) => {
                        for (oi, lb) in t.bytes().enumerate() {
                            if lb == byte {
                                sig.extend_from_slice(&[pi as u8, si as u8, oi as u8]);
                            }
                        }
                    }
                    PatternPiece::Capture { kind, .. } if in_class(kind, byte) => {
                        sig.extend_from_slice(&[pi as u8, si as u8, 0xFF]);
                    }
                    _ => {}
                }
            }
        }
        let next = u8::try_from(signatures.len()).unwrap_or(u8::MAX);
        *slot = *signatures.entry(sig).or_insert(next);
    }
    classes
}

/// Gehoert das Byte zur Zeichenklasse (8.7, Tabelle)?
fn in_class(kind: &CaptureKind, b: u8) -> bool {
    match kind {
        CaptureKind::Int => b.is_ascii_digit() || b == b'+' || b == b'-',
        CaptureKind::Hex => b.is_ascii_hexdigit() || b == b'x' || b == b'X',
        CaptureKind::Float => b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'),
        CaptureKind::Word => b.is_ascii_alphanumeric() || b == b'_',
        // Ein offenes Ende kommt hier nicht an: `build` lehnt solche
        // Muster vorher ab.
        CaptureKind::Str(_) => false,
    }
}

/// Ein Schritt des Automaten: die Folgemenge unter einer Klasse.
fn step(patterns: &[&[PatternPiece]], set: &Set, class: u32, classes: &[u8]) -> Set {
    let mut out = Set::new();
    for pos in set {
        let Some(pieces) = patterns.get(pos.pattern as usize) else { continue };
        let Some(piece) = pieces.get(pos.piece as usize) else { continue };
        match piece {
            PatternPiece::Text(t) => {
                let bytes = t.as_bytes();
                let Some(&want) = bytes.get(pos.offset as usize) else { continue };
                if u32::from(classes[want as usize]) != class {
                    continue;
                }
                let next = pos.offset + 1;
                if next as usize == bytes.len() {
                    advance(patterns, *pos, &mut out);
                } else {
                    out.insert(Position { offset: next, ..*pos });
                }
            }
            PatternPiece::Capture { kind, .. } => {
                // Ein Zeichen der Klasse bleibt im Platzhalter; das erste
                // ausserhalb beendet ihn. Die Mehrdeutigkeitsregel (8.7)
                // garantiert, dass das die gesuchte Grenze ist.
                let fits = (0..=255u8).any(|b| u32::from(classes[b as usize]) == class && in_class(kind, b));
                if fits {
                    // `offset = 1` merkt: mindestens ein Zeichen gelesen.
                    out.insert(Position { offset: 1, ..*pos });
                    advance(patterns, *pos, &mut out);
                }
            }
            PatternPiece::Any => {}
        }
    }
    out
}

/// Nach einem abgeschlossenen Stueck: die Position im naechsten.
fn advance(patterns: &[&[PatternPiece]], pos: Position, out: &mut Set) {
    let next = Position { piece: pos.piece + 1, offset: 0, ..pos };
    let Some(pieces) = patterns.get(pos.pattern as usize) else { return };
    if (next.piece as usize) <= pieces.len() {
        out.insert(next);
    }
}

/// Ist die Position am Ende ihres Musters?
fn accepts(patterns: &[&[PatternPiece]], pos: Position) -> bool {
    patterns.get(pos.pattern as usize).is_some_and(|p| pos.piece as usize >= p.len())
}
