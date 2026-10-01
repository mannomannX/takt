//! Der Durchlaufautomat eines Musters fuer `has` (8.7, 11.2; FB-351).
//!
//! **Wozu.** `has P` gilt ab der fruehesten Stelle, an der der
//! Vorwaertsdurchlauf aus 8.7 gelingt. Den Durchlauf an jeder Stelle neu
//! anzusetzen kostet mit `{_}` das Quadrat der Zeilenlaenge; 8.7 verspricht
//! „Matching ist total und O(Zeilenlaenge)". Ohne offenes Ende liest jeder
//! Ansatz hoechstens die Reichweite des Musters, und die Suche ab jeder
//! Stelle bleibt linear; den Automaten bekommt nur ein Muster, dessen
//! Ansatz bis zum Zeilenende lesen kann ([`Scan::for_has`]).
//!
//! **Wie.** Ab jeder Startstelle ist der Durchlauf deterministisch, und
//! seine Zustaende sind endlich: die Stelle im Literal, Vorzeichen und Zahl
//! der Ziffern eines `int`, die Phase von `0x` und die Ziffern eines `hex`,
//! die Laenge eines `word`, der Suchstand nach dem Folgeliteral eines
//! offenen Endes (Knuth-Morris-Pratt) und die Laenge eines `str<N>`.
//! Zwei Durchlaeufe im selben Zustand an derselben Textstelle laufen gleich
//! weiter. Alle Startstellen laufen darum zugleich, und je Zustand bleibt
//! nur der frueheste Start; der frueheste Start, der das Muster vollendet,
//! ist das Ergebnis. Eine Liste der lebenden Zustaende haelt die Arbeit je
//! Byte bei den Faeden, die es gibt — auf einer gewoehnlichen Zeile sterben
//! sie am ersten falschen Zeichen.
//!
//! Anders als der Produkt-DFA (`dfa.rs`) kennt dieser Automat Hoechstlaengen
//! und Wertebereich, und er bildet keine Teilmengen: Er hat so viele
//! Zustaende wie der Durchlauf eines Musters, nicht wie ihr Produkt.
//!
//! **Ueberlauf.** Ein `int` mit 19 Ziffern und ein `hex` mit 16 koennen
//! `i64` verlassen; 8.7 nennt das „kein Treffer". Der Uebergang, der eine
//! solche Zahl beendet, traegt eine Pruefung ([`Check`]), die der Lauf an
//! den Ziffern im Text rechnet ([`fits`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::pattern::{CaptureKind, PatternPiece};

/// Mehr Zustaende fasst ein Eintrag nicht (14 Bit ohne die Marken); die
/// Fundstelle sucht dann der Durchlauf ab jeder Stelle. Gewoehnliche Muster
/// bleiben weit darunter (`{_}:{n:int}` hat 42).
pub const MAX_STATES: usize = 0x3FF0;

/// Mehr Mengen lebender Zustaende verfolgt [`Scan::worst_step`] nicht.
pub const MAX_SETS: usize = 4096;

/// Ein Eintrag der Tabellen: das Ziel in den unteren 14 Bit, die Pruefung
/// in den oberen zwei.
pub const FAIL: u16 = 0x3FFF;
/// Das Muster ist vollendet; der Start des Fadens ist ein Treffer.
pub const ACCEPT: u16 = 0x3FFE;
const TARGET: u16 = 0x3FFF;
const CHECK_SHIFT: u16 = 14;

/// Was ein Uebergang am Text pruefen muss, bevor er gilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// Nichts.
    None = 0,
    /// Ein `int` mit 19 Ziffern ohne `-` endet hier.
    IntPositive = 1,
    /// Ein `int` mit 19 Ziffern und `-` endet hier.
    IntNegative = 2,
    /// Ein `hex` mit 16 Ziffern endet hier.
    Hex = 3,
}

impl Check {
    fn of(bits: u16) -> Check {
        match bits {
            1 => Check::IntPositive,
            2 => Check::IntNegative,
            3 => Check::Hex,
            _ => Check::None,
        }
    }
}

/// Das Ziel eines Eintrags: ein Zustand, [`FAIL`] oder [`ACCEPT`].
pub fn target(entry: u16) -> u16 {
    entry & TARGET
}

/// Die Pruefung eines Eintrags.
pub fn check(entry: u16) -> Check {
    Check::of(entry >> CHECK_SHIFT)
}

/// Passt die Zahl, die vor `end` endet, in `i64` (8.7)? Fuer `int` die 19
/// Ziffern davor gegen `9223372036854775807` (mit `-` gegen `…808`), fuer
/// `hex` die erste von 16 Ziffern unter `8`.
pub fn fits(check: Check, text: &[u8], end: usize) -> bool {
    let digits = |n: usize| text.get(end.saturating_sub(n)..end).unwrap_or(&[]);
    match check {
        Check::None => true,
        Check::IntPositive => digits(19) <= b"9223372036854775807".as_slice(),
        Check::IntNegative => digits(19) <= b"9223372036854775808".as_slice(),
        Check::Hex => digits(16).first().is_some_and(|d| d.is_ascii_digit() && *d < b'8'),
    }
}

/// Die meiste Arbeit des Laufs an einer Stelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Worst {
    /// Lebende Faeden, der neue eingeschlossen.
    pub threads: usize,
    /// Pruefungen, ob eine Zahl in `i64` passt ([`fits`]).
    pub checks: usize,
}

/// Der Automat eines Musters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scan {
    /// Alphabetklasse je Byte.
    pub classes: Vec<u8>,
    /// Zahl der Klassen.
    pub class_count: u32,
    /// Uebergaenge `states × class_count`.
    pub table: Vec<u16>,
    /// Je Zustand: was das Ende der Zeile bedeutet ([`ACCEPT`] oder
    /// [`FAIL`], mit Pruefung).
    pub eot: Vec<u16>,
    /// Der Zustand eines neuen Fadens; [`ACCEPT`] fuer das leere Muster.
    pub start: u16,
}

impl Scan {
    /// Der Automat fuer `has` auf Zeilen bis `max_len` Bytes, wo er noetig
    /// ist: Nur hinter einem offenen Ende ohne wirksame Grenze — `{_}`, ein
    /// `str<N>` mit `N >= max_len` — kann ein Ansatz bis zum Zeilenende
    /// lesen, und nur dort waere die Suche ab jeder Stelle quadratisch.
    /// Jedes andere Muster liest je Ansatz hoechstens seine Reichweite; die
    /// Suche ab jeder Stelle ist dort schon linear, braucht keinen Rahmen
    /// und im schlimmsten Fall weniger Arbeit je Byte als die Faeden.
    pub fn for_has(pieces: &[PatternPiece], max_len: u32) -> Option<Scan> {
        let unbounded = pieces.iter().any(|p| match p {
            PatternPiece::Any => true,
            PatternPiece::Capture { kind: CaptureKind::Str(n), .. } => *n >= max_len,
            _ => false,
        });
        if unbounded { Scan::of(pieces, max_len) } else { None }
    }

    /// Baut den Automaten. `max_len` ist die Kapazitaet der Zeile: Ein
    /// `str<N>` mit `N >= max_len` kann seine Grenze nicht ueberschreiten
    /// und braucht keinen Zaehler. `None` fuer `float` — seine Umwandlung
    /// fehlt dem Codegen ohnehin (4.2) — und ueber [`MAX_STATES`].
    pub fn of(pieces: &[PatternPiece], max_len: u32) -> Option<Scan> {
        let pass = Pass::of(pieces, max_len)?;
        let mut states: Vec<St> = Vec::new();
        let mut ids: BTreeMap<St, u16> = BTreeMap::new();
        let mut code = |out: Out, states: &mut Vec<St>| -> Option<u16> {
            Some(match out {
                Out::Fail => FAIL,
                Out::Accept => ACCEPT,
                Out::Go(st) => match ids.get(&st) {
                    Some(id) => *id,
                    None => {
                        if states.len() >= MAX_STATES {
                            return None;
                        }
                        let id = states.len() as u16;
                        ids.insert(st, id);
                        states.push(st);
                        id
                    }
                },
            })
        };
        let start = code(pass.enter(0), &mut states)?;
        // Je Zustand und Byte der Eintrag; Bytes mit gleicher Spalte werden
        // eine Klasse.
        let mut rows: Vec<[u16; 256]> = Vec::new();
        let mut eot = Vec::new();
        let mut i = 0;
        while i < states.len() {
            let st = states[i];
            let mut row = [FAIL; 256];
            for b in 0..=255u8 {
                let (out, chk) = pass.step(st, b);
                row[usize::from(b)] = code(out, &mut states)? | ((chk as u16) << CHECK_SHIFT);
            }
            rows.push(row);
            let (out, chk) = pass.eot(st);
            eot.push(code(out, &mut states)? | ((chk as u16) << CHECK_SHIFT));
            i += 1;
        }
        let mut columns: BTreeMap<Vec<u16>, u8> = BTreeMap::new();
        let mut classes = vec![0u8; 256];
        let mut representatives = Vec::new();
        for b in 0..256usize {
            let column: Vec<u16> = rows.iter().map(|r| r[b]).collect();
            let next = columns.len();
            let class = *columns.entry(column).or_insert_with(|| {
                representatives.push(b);
                next as u8
            });
            classes[b] = class;
        }
        let table = rows.iter().flat_map(|r| representatives.iter().map(move |b| r[*b])).collect();
        Some(Scan { classes, class_count: representatives.len() as u32, table, eot, start })
    }

    /// Zahl der Zustaende.
    pub fn states(&self) -> usize {
        self.eot.len()
    }

    /// Was die Tabellen im Objekt belegen: Klassenabbildung, Uebergaenge und
    /// Zeilenende (11.5).
    pub fn bytes(&self) -> u64 {
        256 + 2 * (self.table.len() + self.eot.len()) as u64
    }

    /// Ein Schluessel ueber den Inhalt (FNV-1a): gleiche Automaten, gleiche
    /// Tabellen im Objekt.
    pub fn key(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut feed = |bytes: &[u8]| {
            for b in bytes {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        feed(&self.classes);
        feed(&self.start.to_le_bytes());
        self.table.iter().for_each(|t| feed(&t.to_le_bytes()));
        self.eot.iter().for_each(|t| feed(&t.to_le_bytes()));
        h
    }

    /// Die meiste Arbeit des Laufs an einer Stelle, ueber alle Zeilen.
    /// Gerechnet ueber die Mengen lebender Zustaende, die eine Zeile
    /// erreichen kann (wie in der Teilmengenkonstruktion), mit einem neuen
    /// Faden an jeder Stelle und jeder Pruefung als bestanden — eher zu
    /// viel, nie zu wenig. `None` ueber [`MAX_SETS`] Mengen.
    pub fn worst_step(&self) -> Option<Worst> {
        let c = self.class_count as usize;
        let entry = |s: u16, col: usize| {
            let s = usize::from(s);
            if col == c { self.eot[s] } else { self.table[s * c + col] }
        };
        let mut seen: BTreeSet<Vec<u16>> = BTreeSet::from([Vec::new()]);
        let mut queue = vec![Vec::new()];
        let mut worst = Worst { threads: 0, checks: 0 };
        while let Some(mut live) = queue.pop() {
            if self.start != ACCEPT && !live.contains(&self.start) {
                live.push(self.start);
            }
            worst.threads = worst.threads.max(live.len());
            for col in 0..=c {
                let checks = live
                    .iter()
                    .filter(|s| {
                        let e = entry(**s, col);
                        target(e) != FAIL && check(e) != Check::None
                    })
                    .count();
                worst.checks = worst.checks.max(checks);
                if col == c {
                    continue;
                }
                let mut next: Vec<u16> =
                    live.iter().map(|s| target(entry(*s, col))).filter(|t| *t != FAIL && *t != ACCEPT).collect();
                next.sort_unstable();
                next.dedup();
                if seen.insert(next.clone()) {
                    if seen.len() > MAX_SETS {
                        return None;
                    }
                    queue.push(next);
                }
            }
        }
        Some(worst)
    }

    /// Die frueheste Byteposition, ab der das Muster gelingt — so, wie der
    /// erzeugte Code sie rechnet (`takt-llvm/src/scan.rs`): Faeden beginnen
    /// an jedem Zeichenanfang und hinter dem letzten Zeichen, je Zustand
    /// zaehlt der frueheste Start, und ein Faden, der spaeter als ein
    /// gefundener Treffer begann, faellt weg.
    pub fn first(&self, text: &[u8]) -> Option<usize> {
        self.trace(text, |_| {})
    }

    /// Wie [`Scan::first`]; meldet je Stelle die Arbeit des Laufs: lebende
    /// Faeden und Pruefungen, die er rechnet.
    pub fn trace(&self, text: &[u8], mut each: impl FnMut(Worst)) -> Option<usize> {
        if self.start == ACCEPT {
            return Some(0);
        }
        let n = self.states();
        let (mut cur_start, mut next_start) = (vec![-1i64; n], vec![-1i64; n]);
        let (mut cur, mut next): (Vec<u16>, Vec<u16>) = (Vec::new(), Vec::new());
        let mut best: Option<i64> = None;
        let c = self.class_count as usize;
        for pos in 0..=text.len() {
            let at_end = pos == text.len();
            if best.is_none() && (at_end || text[pos] & 0xC0 != 0x80) {
                let s = usize::from(self.start);
                if cur_start[s] < 0 {
                    cur_start[s] = pos as i64;
                    cur.push(self.start);
                }
            }
            let class = if at_end { 0 } else { usize::from(self.classes[usize::from(text[pos])]) };
            let mut work = Worst { threads: cur.len(), checks: 0 };
            for &s in &cur {
                let s = usize::from(s);
                let st = std::mem::replace(&mut cur_start[s], -1);
                if best.is_some_and(|b| st > b) {
                    continue;
                }
                let entry = if at_end { self.eot[s] } else { self.table[s * c + class] };
                let t = target(entry);
                if t == FAIL {
                    continue;
                }
                work.checks += usize::from(check(entry) != Check::None);
                if !fits(check(entry), text, pos) {
                    continue;
                }
                if t == ACCEPT {
                    best = Some(best.map_or(st, |b| b.min(st)));
                } else if next_start[usize::from(t)] < 0 {
                    next_start[usize::from(t)] = st;
                    next.push(t);
                } else {
                    next_start[usize::from(t)] = next_start[usize::from(t)].min(st);
                }
            }
            each(work);
            cur.clear();
            std::mem::swap(&mut cur, &mut next);
            std::mem::swap(&mut cur_start, &mut next_start);
            if best.is_some() && cur.is_empty() {
                break;
            }
        }
        best.map(|b| b as usize)
    }
}

/// Ein Zustand eines Fadens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum St {
    /// Im Literal `part`, `k` Bytes gelesen.
    Lit { part: u16, k: u16 },
    /// Im `int`: Vorzeichen gelesen, negativ, Ziffern bisher.
    Int { part: u16, signed: bool, neg: bool, digits: u8 },
    /// Im `hex`.
    Hex { part: u16, phase: HexPhase, digits: u8 },
    /// Im `word`, `len` Zeichen gelesen.
    Word { part: u16, len: u8 },
    /// Im offenen Ende: Suchstand im Folgeliteral und gelesene Bytes (nur
    /// bei `str<N>` mit wirksamer Grenze, sonst null).
    Open { part: u16, kmp: u16, read: u32 },
}

/// Wo ein `hex` steht (8.7: `(0x)?[0-9a-fA-F]{1,16}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum HexPhase {
    /// Noch nichts.
    Start,
    /// Eine `0`; folgt `x`, war sie das Praefix, sonst die erste Ziffer.
    Zero,
    /// Nach `0x`, noch keine Ziffer.
    Prefix,
    /// `digits` Ziffern.
    Digits,
}

/// Was ein Zeichen oder das Zeilenende aus einem Faden macht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Out {
    Fail,
    Accept,
    Go(St),
}

/// Ein Baustein des Durchlaufs.
#[derive(Clone, Debug)]
enum Part {
    Lit(Vec<u8>),
    Int,
    Hex,
    Word,
    /// `str<N>` oder `{_}` mit seinem Folgeliteral, das es zugleich
    /// verbraucht; `limit` ist die Grenze in Bytes, wenn sie wirkt.
    Open {
        next: Option<Kmp>,
        limit: Option<u32>,
    },
}

/// Ein Literal mit seiner Fehlerfunktion (Knuth-Morris-Pratt).
#[derive(Clone, Debug)]
struct Kmp {
    lit: Vec<u8>,
    fail: Vec<u16>,
}

impl Kmp {
    fn of(lit: &[u8]) -> Kmp {
        let mut fail = vec![0u16; lit.len()];
        let mut k = 0usize;
        for i in 1..lit.len() {
            while k > 0 && lit[i] != lit[k] {
                k = usize::from(fail[k - 1]);
            }
            if lit[i] == lit[k] {
                k += 1;
            }
            fail[i] = k as u16;
        }
        Kmp { lit: lit.to_vec(), fail }
    }

    /// Wie viele Bytes des Literals nach `b` als Suffix stehen.
    fn step(&self, mut k: usize, b: u8) -> usize {
        while k > 0 && self.lit[k] != b {
            k = usize::from(self.fail[k - 1]);
        }
        if self.lit[k] == b { k + 1 } else { 0 }
    }
}

/// Der Durchlauf eines Musters ab einer Startstelle, als Automat.
struct Pass {
    parts: Vec<Part>,
}

impl Pass {
    fn of(pieces: &[PatternPiece], max_len: u32) -> Option<Pass> {
        let mut parts = Vec::with_capacity(pieces.len());
        let mut i = 0;
        while i < pieces.len() {
            let open = |limit: Option<u32>| match pieces.get(i + 1) {
                None => Some((Part::Open { next: None, limit }, 1)),
                Some(PatternPiece::Text(t)) if !t.is_empty() => {
                    Some((Part::Open { next: Some(Kmp::of(t.as_bytes())), limit }, 2))
                }
                // Pruefung 18 laesst keinen Platzhalter direkt folgen.
                Some(_) => None,
            };
            let (part, used) = match &pieces[i] {
                PatternPiece::Text(t) if t.is_empty() => {
                    i += 1;
                    continue;
                }
                PatternPiece::Text(t) => (Part::Lit(t.as_bytes().to_vec()), 1),
                PatternPiece::Any => open(None)?,
                PatternPiece::Capture { kind, .. } => match kind {
                    CaptureKind::Int => (Part::Int, 1),
                    CaptureKind::Hex => (Part::Hex, 1),
                    CaptureKind::Word => (Part::Word, 1),
                    CaptureKind::Str(n) => open((*n < max_len).then_some(*n))?,
                    CaptureKind::Float => return None,
                },
            };
            parts.push(part);
            i += used;
        }
        Some(Pass { parts })
    }

    /// Der Beginn von Baustein `i`; hinter dem letzten ist das Muster
    /// vollendet.
    fn enter(&self, i: usize) -> Out {
        let part = i as u16;
        Out::Go(match self.parts.get(i) {
            None => return Out::Accept,
            Some(Part::Lit(_)) => St::Lit { part, k: 0 },
            Some(Part::Int) => St::Int { part, signed: false, neg: false, digits: 0 },
            Some(Part::Hex) => St::Hex { part, phase: HexPhase::Start, digits: 0 },
            Some(Part::Word) => St::Word { part, len: 0 },
            Some(Part::Open { .. }) => St::Open { part, kmp: 0, read: 0 },
        })
    }

    /// Ein Zeichen, das ein Baustein nicht mehr nimmt, gehoert dem naechsten.
    fn hand_on(&self, part: u16, b: u8) -> (Out, Check) {
        match self.enter(usize::from(part) + 1) {
            Out::Go(st) => self.step(st, b),
            other => (other, Check::None),
        }
    }

    /// Das Zeilenende nach dem Baustein `part`.
    fn end_after(&self, part: u16) -> (Out, Check) {
        match self.enter(usize::from(part) + 1) {
            Out::Go(st) => self.eot(st),
            other => (other, Check::None),
        }
    }

    /// Ein Byte weiter.
    fn step(&self, st: St, b: u8) -> (Out, Check) {
        let go = |st| (Out::Go(st), Check::None);
        let fail = (Out::Fail, Check::None);
        match st {
            St::Lit { part, k } => {
                let Some(Part::Lit(lit)) = self.parts.get(usize::from(part)) else { return fail };
                if lit.get(usize::from(k)) != Some(&b) {
                    return fail;
                }
                if usize::from(k) + 1 == lit.len() {
                    (self.enter(usize::from(part) + 1), Check::None)
                } else {
                    go(St::Lit { part, k: k + 1 })
                }
            }
            St::Int { part, signed, neg, digits: 0 } => match b {
                b'+' | b'-' if !signed => go(St::Int { part, signed: true, neg: b == b'-', digits: 0 }),
                b'0'..=b'9' => go(St::Int { part, signed: false, neg, digits: 1 }),
                _ => fail,
            },
            St::Int { part, neg, digits, .. } => {
                if b.is_ascii_digit() && digits < 19 {
                    return go(St::Int { part, signed: false, neg, digits: digits + 1 });
                }
                let check = match (digits, neg) {
                    (19, false) => Check::IntPositive,
                    (19, true) => Check::IntNegative,
                    _ => Check::None,
                };
                let (out, _) = self.hand_on(part, b);
                (out, check)
            }
            St::Hex { part, phase, digits } => match phase {
                HexPhase::Start if b == b'0' => go(St::Hex { part, phase: HexPhase::Zero, digits: 1 }),
                HexPhase::Start | HexPhase::Prefix if b.is_ascii_hexdigit() => {
                    go(St::Hex { part, phase: HexPhase::Digits, digits: 1 })
                }
                HexPhase::Start | HexPhase::Prefix => fail,
                HexPhase::Zero if b == b'x' => go(St::Hex { part, phase: HexPhase::Prefix, digits: 0 }),
                HexPhase::Zero | HexPhase::Digits if b.is_ascii_hexdigit() && digits < 16 => {
                    go(St::Hex { part, phase: HexPhase::Digits, digits: digits + 1 })
                }
                HexPhase::Zero | HexPhase::Digits => {
                    let check = if digits == 16 { Check::Hex } else { Check::None };
                    let (out, _) = self.hand_on(part, b);
                    (out, check)
                }
            },
            St::Word { part, len } => {
                if (b.is_ascii_alphanumeric() || b == b'_') && len < 64 {
                    go(St::Word { part, len: len + 1 })
                } else if len == 0 {
                    fail
                } else {
                    self.hand_on(part, b)
                }
            }
            St::Open { part, kmp, read } => {
                let Some(Part::Open { next, limit }) = self.parts.get(usize::from(part)) else { return fail };
                let read = if limit.is_some() { read + 1 } else { 0 };
                let Some(lit) = next else {
                    return if limit.is_some_and(|n| read > n) { fail } else { go(St::Open { part, kmp: 0, read }) };
                };
                let k = lit.step(usize::from(kmp), b);
                // Vor den `k` Bytes des Literals liegt die Spanne; sie darf
                // `N` Bytes nicht ueberschreiten, auch nicht spaeter.
                if limit.is_some_and(|n| read - k as u32 > n) {
                    return fail;
                }
                if k == lit.lit.len() {
                    (self.enter(usize::from(part) + 1), Check::None)
                } else {
                    go(St::Open { part, kmp: k as u16, read })
                }
            }
        }
    }

    /// Das Ende der Zeile in diesem Zustand.
    fn eot(&self, st: St) -> (Out, Check) {
        let fail = (Out::Fail, Check::None);
        match st {
            St::Lit { .. } => fail,
            St::Int { digits: 0, .. } => fail,
            St::Int { part, neg, digits, .. } => {
                let check = match (digits, neg) {
                    (19, false) => Check::IntPositive,
                    (19, true) => Check::IntNegative,
                    _ => Check::None,
                };
                (self.end_after(part).0, check)
            }
            St::Hex { phase: HexPhase::Start | HexPhase::Prefix, .. } => fail,
            St::Hex { part, digits, .. } => {
                let check = if digits == 16 { Check::Hex } else { Check::None };
                (self.end_after(part).0, check)
            }
            St::Word { len: 0, .. } => fail,
            St::Word { part, .. } => self.end_after(part),
            // Ein offenes Ende ohne Folgeliteral nimmt den Rest; seine
            // Grenze hat jedes Zeichen schon geprueft.
            St::Open { part, .. } => match self.parts.get(usize::from(part)) {
                Some(Part::Open { next: None, .. }) => self.end_after(part),
                _ => fail,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> PatternPiece {
        PatternPiece::Text(t.into())
    }

    fn cap(kind: CaptureKind) -> PatternPiece {
        PatternPiece::Capture { name: "x".into(), kind }
    }

    /// Das leere Muster trifft vorn, ein Literal an seinem ersten Vorkommen.
    #[test]
    fn literals_find_their_first_occurrence() {
        let s = Scan::of(&[], 64).expect("Automat");
        assert_eq!(s.first(b"abc"), Some(0));
        let s = Scan::of(&[text("aab")], 64).expect("Automat");
        assert_eq!(s.first(b"aaab"), Some(1));
        assert_eq!(s.first(b"aaa"), None);
    }

    /// `{_}:` vor einer Zahl: der frueheste Start ist null, gleich wie lang
    /// die Zeile — und der Automat bleibt klein.
    #[test]
    fn an_open_end_keeps_the_earliest_start() {
        let s = Scan::of(&[PatternPiece::Any, text(":"), cap(CaptureKind::Int)], 256).expect("Automat");
        assert!(s.states() < 50, "{} Zustaende", s.states());
        assert_eq!(s.first(b"aaaa:12"), Some(0));
        assert_eq!(s.first(b"aaaa:x"), None);
    }

    /// 8.7: Ein `int` ueber `i64` ist kein Treffer, `-9223372036854775808`
    /// schon; ein `hex` ab `8` mit 16 Ziffern ebenso nicht.
    #[test]
    fn overflow_is_no_hit() {
        let s = Scan::of(&[text("="), cap(CaptureKind::Int)], 64).expect("Automat");
        assert_eq!(s.first(b"=9223372036854775807"), Some(0));
        assert_eq!(s.first(b"=9223372036854775808"), None);
        assert_eq!(s.first(b"=-9223372036854775808"), Some(0));
        assert_eq!(s.first(b"=-9223372036854775809"), None);
        let s = Scan::of(&[text("="), cap(CaptureKind::Hex)], 64).expect("Automat");
        assert_eq!(s.first(b"=7fffffffffffffff"), Some(0));
        assert_eq!(s.first(b"=8000000000000000"), None);
        assert_eq!(s.first(b"=0x8000000000000000"), None);
    }

    /// `str<N>` zaehlt Bytes, wie der Typ sie fasst (FB-358); ein Faden
    /// beginnt nur an einem Zeichenanfang.
    #[test]
    fn a_bounded_string_counts_bytes() {
        let s = Scan::of(&[cap(CaptureKind::Str(3)), text(";")], 64).expect("Automat");
        assert_eq!(s.first("abc;".as_bytes()), Some(0));
        assert_eq!(s.first("äöü;".as_bytes()), Some(4));
        assert_eq!(s.first("äbc;".as_bytes()), Some(2));
    }

    /// Einen Automaten braucht nur ein Muster, dessen Ansatz bis zum
    /// Zeilenende lesen kann.
    #[test]
    fn only_an_unbounded_pattern_gets_an_automaton() {
        let any = [text("go"), PatternPiece::Any, text(":"), cap(CaptureKind::Int)];
        assert!(Scan::for_has(&any, 64).is_some());
        assert!(Scan::for_has(&[cap(CaptureKind::Str(64)), text(";")], 64).is_some());
        assert!(Scan::for_has(&[cap(CaptureKind::Str(63)), text(";")], 64).is_none());
        assert!(Scan::for_has(&[text("id="), cap(CaptureKind::Word), text(";")], 64).is_none());
    }
}
