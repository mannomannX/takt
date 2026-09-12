//! Mustervergleich und Werteextraktion (Referenz 8.7).
//!
//! **Warum ein eigenes Crate.** Interpreter und erzeugter Code brauchen
//! dieselbe Antwort auf dieselbe Frage: Trifft das Muster, und was steht
//! in den Platzhaltern? Zwei Implementierungen waeren zwei Gelegenheiten,
//! `int` seine 19 Stellen verschieden zu geben — und Satz 9.4.4 verlangt
//! Bit-Gleichheit. Prinzip 4 des Plans nennt das Vorbild: `libtaktm` und
//! `takt-native` sind `no_std`-Crates, die Host und Targets teilen.
//!
//! **Was hier steht und was nicht.** Der Vorwaertsdurchlauf mit
//! Extraktion — 8.7: „Matching plus Extraktion ist ein einziger
//! Vorwaertsdurchlauf". Der Produkt-DFA steht daneben
//! (`takt-mir::dfa`): Er beantwortet schneller, *welches* Muster trifft,
//! wenn ein Zustand mehrere Handler hat, und ist eine Kostenfrage
//! (11.2). Wer nur ein Muster hat, braucht ihn nicht.
//!
//! **Warum Bytes und nicht `&str`.** Ein `line<N>` im erzeugten Code ist
//! `{ i32 len, [N x i8], i1 truncated }` (11.2) — ein Puffer, kein
//! Rust-String. Die Zeichenklassen aus 8.7 sind alle ASCII, und ein
//! Nicht-ASCII-Byte gehoert zu keiner; der Vergleich bleibt damit
//! byteweise korrekt.

#![no_std]

/// Die Art eines Platzhalters (8.7, Tabelle der Zeichenklassen).
///
/// Sie spiegelt `takt_mir::pattern::CaptureKind`; dieses Crate kennt die
/// MIR nicht, weil es auf dem Target laeuft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `[+-]?[0-9]{1,19}` → `int`.
    Int,
    /// `(0x)?[0-9a-fA-F]{1,16}` → `int`.
    Hex,
    /// `[+-]?[0-9]+(.[0-9]+)?([eE][+-]?[0-9]+)?` → `float`.
    Float,
    /// `[A-Za-z0-9_]{1,64}` → `str<64>`.
    Word,
    /// Beliebige Zeichen, hoechstens N → `str<N>`.
    Str(u32),
    /// `{_}`: wie `Str`, ohne Bindung.
    Any,
}

/// Ein Stueck eines Musters (8.7).
#[derive(Clone, Copy, Debug)]
pub enum Piece<'a> {
    /// Literaler Text.
    Text(&'a [u8]),
    /// Platzhalter.
    Capture(Kind),
}

/// Was ein Platzhalter gefunden hat: der Bereich im Text.
///
/// Der Wert selbst entsteht daraus mit `parse_int`, `parse_hex` oder
/// `parse_float`; ein `Word` oder `Str` ist der Bereich selbst.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    /// Erstes Byte.
    pub start: u32,
    /// Hinter dem letzten Byte.
    pub end: u32,
}

impl Span {
    /// Der Ausschnitt eines Texts.
    pub fn of<'a>(&self, text: &'a [u8]) -> &'a [u8] {
        let (a, b) = (self.start as usize, self.end as usize);
        if b <= text.len() && a <= b { &text[a..b] } else { &[] }
    }
}

/// Hoechstzahl der Platzhalter je Muster.
///
/// 8.7 begrenzt sie nicht, aber ein Muster ist ein Literal im Quelltext
/// — sechzehn ist grosszuegig, und eine feste Zahl erspart dem Target
/// eine Allokation (4.1: keine dynamische Groesse).
pub const MAX_CAPTURES: usize = 16;

/// Das Ergebnis eines Abgleichs.
#[derive(Clone, Copy, Debug)]
pub struct Match {
    /// Die Bereiche der Platzhalter, in Musterreihenfolge.
    pub spans: [Span; MAX_CAPTURES],
    /// Wie viele davon belegt sind.
    pub count: u32,
}

impl Default for Match {
    fn default() -> Self {
        Match { spans: [Span::default(); MAX_CAPTURES], count: 0 }
    }
}

/// Gleicht ein Muster gegen den ganzen Text ab (`matches`, 8.7).
///
/// Ein einziger Vorwaertsdurchlauf: Die Mehrdeutigkeitsregel — auf
/// `int`, `hex`, `float` und `word` folgt ein Literal, dessen erstes
/// Zeichen nicht zur Klasse gehoert — macht jede Grenze eindeutig, und
/// darum braucht es kein Backtracking.
pub fn matches(pieces: &[Piece<'_>], text: &[u8]) -> Option<Match> {
    let (m, rest) = walk(pieces, text, 0)?;
    if rest as usize == text.len() { Some(m) } else { None }
}

/// Sucht ein Vorkommen des Musters (`has`, 8.7).
///
/// Die Startpositionen werden der Reihe nach probiert; die erste, an der
/// der Durchlauf traegt, gewinnt (leftmost). Die Schleife ist durch die
/// Textlaenge beschraenkt, die ihrerseits durch `N` beschraenkt ist
/// (3.9) — 4.1 verlangt genau das.
pub fn has(pieces: &[Piece<'_>], text: &[u8]) -> Option<Match> {
    for start in 0..=text.len() {
        if let Some((m, _)) = walk(pieces, text, start as u32) {
            return Some(m);
        }
    }
    None
}

/// Der Durchlauf ab einer Startposition; liefert die Treffer und das
/// Ende des verbrauchten Bereichs.
fn walk(pieces: &[Piece<'_>], text: &[u8], from: u32) -> Option<(Match, u32)> {
    let mut out = Match::default();
    let mut at = from as usize;
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Text(lit) => {
                if text.len() < at + lit.len() || &text[at..at + lit.len()] != *lit {
                    return None;
                }
                at += lit.len();
            }
            Piece::Capture(kind) => {
                let end = take(*kind, &pieces[i + 1..], text, at)?;
                if out.count as usize >= MAX_CAPTURES {
                    return None;
                }
                // `{_}` bindet nicht (8.7), belegt also keinen Platz.
                if !matches!(kind, Kind::Any) {
                    out.spans[out.count as usize] = Span { start: at as u32, end: end as u32 };
                    out.count += 1;
                }
                at = end;
            }
        }
    }
    Some((out, at as u32))
}

/// Verbraucht den Text eines Platzhalters; liefert seine Endposition.
fn take(kind: Kind, after: &[Piece<'_>], text: &[u8], at: usize) -> Option<usize> {
    match kind {
        Kind::Int => signed(text, at, is_digit, 19),
        Kind::Hex => hex(text, at),
        Kind::Float => float(text, at),
        Kind::Word => bounded(text, at, is_word, 64),
        // Ein offenes Ende endet beim ersten Vorkommen des naechsten
        // Literals (leftmost-shortest, 8.7).
        Kind::Str(n) => open_end(after, text, at, Some(n as usize)),
        Kind::Any => open_end(after, text, at, None),
    }
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Laengstes Praefix aus Zeichen der Klasse, hoechstens `max` Zeichen.
/// Ein leeres Praefix ist kein Treffer — jede Klasse verlangt `{1,…}`.
fn bounded(text: &[u8], at: usize, class: fn(u8) -> bool, max: usize) -> Option<usize> {
    let mut end = at;
    while end < text.len() && end - at < max && class(text[end]) {
        end += 1;
    }
    if end == at { None } else { Some(end) }
}

/// `[+-]?[0-9]{1,19}` (8.7): Das Vorzeichen zaehlt nicht zu den Ziffern.
fn signed(text: &[u8], at: usize, class: fn(u8) -> bool, max: usize) -> Option<usize> {
    let sign = usize::from(text.get(at).is_some_and(|b| *b == b'+' || *b == b'-'));
    bounded(text, at + sign, class, max)
}

/// `(0x)?[0-9a-fA-F]{1,16}` (8.7).
fn hex(text: &[u8], at: usize) -> Option<usize> {
    let prefix = usize::from(text.len() > at + 1 && text[at] == b'0' && text[at + 1] == b'x') * 2;
    bounded(text, at + prefix, |b| b.is_ascii_hexdigit(), 16)
}

/// `[+-]?[0-9]+(.[0-9]+)?([eE][+-]?[0-9]+)?` (8.7).
fn float(text: &[u8], at: usize) -> Option<usize> {
    let mut end = signed(text, at, is_digit, usize::MAX)?;
    if text.get(end) == Some(&b'.')
        && let Some(frac) = bounded(text, end + 1, is_digit, usize::MAX)
    {
        end = frac;
    }
    if text.get(end).is_some_and(|b| *b == b'e' || *b == b'E')
        && let Some(exp) = signed(text, end + 1, is_digit, usize::MAX)
    {
        end = exp;
    }
    Some(end)
}

/// Offenes Ende: bis zum ersten Vorkommen des naechsten Literals, sonst
/// bis zum Textende (leftmost-shortest, 8.7).
fn open_end(after: &[Piece<'_>], text: &[u8], at: usize, max: Option<usize>) -> Option<usize> {
    let grenze = |end: usize| max.is_none_or(|n| end - at <= n);
    let Some(Piece::Text(lit)) = after.iter().find(|p| matches!(p, Piece::Text(_))) else {
        // Kein Folgeliteral: bis zum Ende, soweit die Grenze reicht.
        return grenze(text.len()).then_some(text.len());
    };
    let mut end = at;
    while end + lit.len() <= text.len() {
        if &text[end..end + lit.len()] == *lit {
            return grenze(end).then_some(end);
        }
        end += 1;
    }
    None
}

/// `int` aus dem Bereich (8.7: Ueberlauf ist kein Treffer).
pub fn parse_int(bytes: &[u8]) -> Option<i64> {
    let (neg, digits) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    if digits.is_empty() {
        return None;
    }
    // Der Betrag waechst negativ, weil der Zweierkomplementbereich
    // asymmetrisch ist: `-9223372036854775808` ist gueltig, sein Betrag
    // nicht. Wer positiv rechnet und am Ende negiert, verliert genau
    // diese eine Zahl — derselbe Fehler wie FB-95, an anderer Stelle.
    let mut n: i64 = 0;
    for b in digits {
        let d = i64::from(b.checked_sub(b'0')?);
        if d > 9 {
            return None;
        }
        // 8.7: „Ueberlauf → kein Match". Der Fault gehoert nicht hierher
        // — ein Muster, das nicht trifft, ist kein Fehler.
        n = n.checked_mul(10)?.checked_sub(d)?;
    }
    if neg { Some(n) } else { n.checked_neg() }
}

// Warum es hier kein `parse_float` gibt.
//
// 8.7 verlangt `float` als Capture-Art, und der Bereich wird auch
// erkannt (`Kind::Float`) — nur die *Umwandlung* in eine Zahl fehlt.
// Sie waere eine zweite Rundungsquelle neben `libtaktm`, und 4.2
// verlangt bitgleiche Ergebnisse ueber alle Targets: Ein eigener
// Dezimal-nach-Binaer-Umsetzer muesste korrekt gerundet sein, sonst
// weichen Interpreter und erzeugter Code in der letzten Stelle ab.
//
// Der Interpreter kommt heute mit `str::parse::<f64>` aus, weil er auf
// dem Host laeuft; ein Target hat das nicht. Die saubere Loesung ist
// eine korrekt gerundete Konversion in `libtaktm`, neben `sqrt` und
// `fma` — mit Konformitaetsvektoren wie diese (13.8). Bis dahin lehnt
// der Codegen ein `float`-Capture ab, statt still zu runden.

/// `hex` aus dem Bereich; das Praefix `0x` ist optional (8.7).
pub fn parse_hex(bytes: &[u8]) -> Option<i64> {
    let digits = if bytes.len() > 1 && bytes[0] == b'0' && bytes[1] == b'x' { &bytes[2..] } else { bytes };
    if digits.is_empty() {
        return None;
    }
    let mut n: i64 = 0;
    for b in digits {
        let d = match b {
            b'0'..=b'9' => i64::from(b - b'0'),
            b'a'..=b'f' => i64::from(b - b'a') + 10,
            b'A'..=b'F' => i64::from(b - b'A') + 10,
            _ => return None,
        };
        n = n.checked_mul(16)?.checked_add(d)?;
    }
    Some(n)
}
