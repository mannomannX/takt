//! Der Produkt-DFA der Textmuster eines Handler-Blocks (8.7, 11.2).
//!
//! **Welche Sprache er erkennt.** 8.7 definiert ein Muster ueber seinen
//! Vorwaertsdurchlauf (FB-350): Literale Zeichen fuer Zeichen, `int`,
//! `hex` und `word` gierig bis zur Hoechstlaenge, `str` und `_` bis zum
//! ersten Vorkommen des folgenden Literals, ohne Zurueckgehen. Der Automat
//! geht denselben Weg ohne die Hoechstlaengen (19, 16, 64, `N` von
//! `str<N>`) und ohne den Wertebereich: Ein Muster ohne Platzhalter
//! entscheidet er genau, eines mit Platzhaltern als Obermenge — jede
//! Zeile, auf der der Durchlauf gelingt, nimmt er an, denn hinter einer
//! Klasse steht nach der Mehrdeutigkeitsregel ein Zeichen ausserhalb der
//! Klasse. Den Treffer eines solchen Musters bestaetigt die Extraktion,
//! die ohnehin laeuft, um die Werte zu holen (`extracts`). Zaehler bis
//! 19 oder 64 vervielfachten sich im Produkt; ohne sie bleiben die
//! Tabellen klein genug fuer den RAM eines XIP-Ziels (12.3).
//!
//! **Wie er entsteht.** Jedes Muster ist zuerst ein deterministischer
//! Automat seines Durchlaufs (`At`): ein Literal zaehlt gelesene Zeichen,
//! eine Klasse merkt Vorzeichen und `0x`, ein `str` oder `_` sucht sein
//! Folgeliteral mit einer Kette wie bei Knuth-Morris-Pratt. `matches`
//! startet einen Faden am Anfang, `has` an jeder Stelle einen neuen. Die
//! Teilmengenkonstruktion ueber alle Faeden aller Muster ergibt den
//! Produkt-DFA; je Zustand steht, welche Muster treffen, wenn die Zeile
//! dort endet.
//!
//! **Alphabetklassen.** Bytes, die alle Muster gleich behandeln, teilen
//! eine Spalte (11.2: „typisch 10-25 statt 256"). Die Klassenabbildung
//! ist 256 Byte je Block, die Tabelle `states × classes` — beides geht
//! in `takt size` ein (11.5).

use std::collections::BTreeMap;

use crate::expr::MatchKind;
use crate::machine::Handler;
use crate::pattern::{CaptureKind, Pattern, PatternPiece};

/// Mehr Zustaende baut der Uebersetzer nicht; der Dispatch pruefte dann
/// jeden Handler einzeln. 8.7 sagt, die Groesse sei durch die
/// Musterlaenge beschraenkt — die Schranke schuetzt vor einem
/// Entwurfsfehler, nicht vor einem gueltigen Muster.
pub const MAX_STATES: usize = 4096;

/// Ein Textmuster des Blocks und wie es gefragt wird.
#[derive(Clone, Copy, Debug)]
pub struct Entry<'a> {
    /// Die Bausteine.
    pub pieces: &'a [PatternPiece],
    /// `has` statt `matches`.
    pub has: bool,
}

/// Der Automat eines Handler-Blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dfa {
    /// Alphabetklasse je Byte.
    pub classes: Vec<u8>,
    /// Zahl der Klassen.
    pub class_count: u32,
    /// Uebergangstabelle `states × class_count`; Zustand 0 ist der Start.
    pub table: Vec<u16>,
    /// Je Zustand: Bit `i` gesetzt, wenn Muster `i` trifft, sobald die
    /// Zeile hier endet.
    pub accept: Vec<u64>,
    /// Zahl der Muster.
    pub patterns: u32,
}

impl Dfa {
    /// Zahl der Zustaende.
    pub fn states(&self) -> usize {
        self.accept.len()
    }

    /// Byte je Uebergang: eines bis 256 Zustaende, sonst zwei.
    pub fn state_width(&self) -> u32 {
        if self.states() <= 256 { 1 } else { 2 }
    }

    /// Byte je Eintrag der Trefferliste: so wenige, wie die Muster brauchen.
    pub fn accept_width(&self) -> u32 {
        match self.patterns {
            0..=8 => 1,
            9..=16 => 2,
            17..=32 => 4,
            _ => 8,
        }
    }

    /// Was die Tabellen im Objekt belegen: Klassenabbildung, Uebergaenge,
    /// Trefferliste (11.5).
    pub fn bytes(&self) -> u64 {
        self.classes.len() as u64
            + self.table.len() as u64 * u64::from(self.state_width())
            + self.accept.len() as u64 * u64::from(self.accept_width())
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
        feed(&self.patterns.to_le_bytes());
        self.table.iter().for_each(|t| feed(&t.to_le_bytes()));
        self.accept.iter().for_each(|a| feed(&a.to_le_bytes()));
        h
    }

    /// Liest eine Zeile; die Muster, die auf ihr treffen.
    pub fn run(&self, text: &[u8]) -> u64 {
        let mut state = 0usize;
        for b in text {
            let class = self.classes[usize::from(*b)] as usize;
            state = usize::from(self.table[state * self.class_count as usize + class]);
        }
        self.accept[state]
    }
}

/// Die Handler eines Stroms auf einer Ebene (8.7) als ein Automat ueber
/// ihre Textmuster: je Handler das Bit seines Musters, `None` fuer ein
/// Record-Muster, einen Catch-all und jeden Handler hinter einem
/// Catch-all ohne Guard, der nie laeuft. `None` insgesamt, wenn es kein
/// Textmuster gibt oder `build` keinen Automaten baut. Codegen und
/// `takt size` fragen beide hier.
pub fn of_handlers(handlers: &[&Handler]) -> Option<(Dfa, Vec<Option<usize>>)> {
    let reach = handlers.iter().position(|h| h.pattern.is_none() && h.guard.is_none()).unwrap_or(handlers.len());
    let mut entries = Vec::new();
    let bits = handlers
        .iter()
        .enumerate()
        .map(|(i, h)| match &h.pattern {
            Some((kind, Pattern::Text { pieces })) if i < reach => {
                entries.push(Entry { pieces, has: *kind == MatchKind::Has });
                Some(entries.len() - 1)
            }
            _ => None,
        })
        .collect();
    Some((build(&entries)?, bits))
}

/// Braucht ein Treffer des Automaten noch den Durchlauf? Bei jedem
/// Platzhalter: Seine Hoechstlaenge und seinen Wertebereich kennt der
/// Automat nicht, und die Werte holt ohnehin der Durchlauf (8.7).
pub fn extracts(pieces: &[PatternPiece]) -> bool {
    pieces.iter().any(|p| matches!(p, PatternPiece::Capture { .. }))
}

/// Die Handler einer Ebene, nach Strom gruppiert in der Reihenfolge
/// ihres ersten Auftretens — die Gruppen des Dispatch (8.7).
pub fn by_stream(handlers: &[Handler]) -> Vec<Vec<&Handler>> {
    let mut groups: Vec<Vec<&Handler>> = Vec::new();
    for h in handlers {
        match groups.iter_mut().find(|g| g[0].stream == h.stream) {
            Some(g) => g.push(h),
            None => groups.push(vec![h]),
        }
    }
    groups
}

/// Baut den Produkt-DFA ueber die Textmuster eines Blocks (11.2), in
/// Quelltextreihenfolge; Bit `i` der Treffer ist Muster `i`.
///
/// `None`, wenn ein Muster keinen Automaten bekommt — `float` (seine
/// Umwandlung fehlt dem Codegen ohnehin, 4.2) oder ein Platzhalter direkt
/// hinter `str`/`_`, den Pruefung 18 abweist — oder wenn die Muster zu
/// viele Zustaende ergaeben. Der Dispatch prueft dann jeden Handler mit
/// seinem Durchlauf; das Urteil ist dasselbe.
pub fn build(entries: &[Entry<'_>]) -> Option<Dfa> {
    if entries.is_empty() || entries.len() > 64 {
        return None;
    }
    let patterns: Vec<Compiled> = entries.iter().map(|e| Compiled::of(e.pieces, e.has)).collect::<Option<_>>()?;
    let (classes, representatives) = alphabet(&patterns);
    let class_count = representatives.len() as u32;

    let start = seed(&patterns, Vec::new(), true);
    let mut ids: BTreeMap<Set, u16> = BTreeMap::new();
    let mut queue = vec![start.clone()];
    ids.insert(start, 0);
    let mut table: Vec<u16> = Vec::new();
    let mut accept: Vec<u64> = Vec::new();
    let mut i = 0;
    while i < queue.len() {
        let set = queue[i].clone();
        i += 1;
        let hits = set.iter().filter(|(p, at)| patterns[usize::from(*p)].final_at_eof(*at));
        accept.push(hits.fold(0u64, |mask, (p, _)| mask | 1u64 << *p));
        for &b in &representatives {
            let moved = set.iter().filter_map(|(p, at)| Some((*p, patterns[*p as usize].step(*at, b)?))).collect();
            let next = seed(&patterns, moved, false);
            let id = match ids.get(&next) {
                Some(id) => *id,
                None => {
                    if queue.len() >= MAX_STATES {
                        return None;
                    }
                    let id = u16::try_from(queue.len()).ok()?;
                    ids.insert(next.clone(), id);
                    queue.push(next);
                    id
                }
            };
            table.push(id);
        }
    }
    Some(Dfa { classes, class_count, table, accept, patterns: entries.len() as u32 })
}

/// Die Faeden eines DFA-Zustands, sortiert: Muster und Stelle darin.
type Set = Vec<(u8, At)>;

/// Macht aus den weitergerueckten Faeden einen Zustand: `has` beginnt an
/// der naechsten Stelle einen neuen Faden, ein Faden, dessen Treffer
/// feststeht, wird `Hit`, und ein Muster mit `Hit` braucht keine weiteren.
/// Gleiche Faeden fallen zusammen.
fn seed(patterns: &[Compiled], mut set: Set, first: bool) -> Set {
    for (p, c) in patterns.iter().enumerate() {
        if first || c.has {
            set.push((p as u8, c.enter(0)));
        }
    }
    for (p, at) in &mut set {
        if patterns[*p as usize].has && patterns[*p as usize].complete(*at) {
            *at = At::Hit;
        }
    }
    let hit: Vec<u8> = set.iter().filter(|(_, at)| *at == At::Hit).map(|(p, _)| *p).collect();
    set.retain(|(p, at)| *at == At::Hit || !hit.contains(p));
    set.sort_unstable();
    set.dedup();
    set
}

/// Eine Stelle im Durchlauf eines Musters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum At {
    /// Im Literal `part`, `k` Zeichen gelesen.
    Lit { part: u16, k: u16 },
    /// In einer Klasse; `phase` fuer Vorzeichen und `0x`.
    Run { part: u16, phase: Phase },
    /// In `str`/`_`: `kmp` Zeichen des Folgeliterals erkannt.
    Open { part: u16, kmp: u16 },
    /// Alle Bausteine gelesen.
    Done,
    /// `has`: Ein Start hat getroffen, der Rest der Zeile ist gleich.
    Hit,
}

/// Wo ein Klassenlauf steht.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    /// Noch kein Zeichen.
    Start,
    /// `int`: nach `+` oder `-`, noch keine Ziffer.
    Sign,
    /// `hex`: eine `0` gelesen; folgt `x`, war sie das Praefix.
    Zero,
    /// `hex`: nach `0x`, noch keine Ziffer.
    Prefix,
    /// Im Lauf, mindestens ein Zeichen.
    Digits,
}

/// Ein Baustein, wie der Automat ihn braucht.
#[derive(Clone, Debug)]
enum Part {
    Lit(Vec<u8>),
    Class(Class),
    /// `str<N>` und `_`; das Folgeliteral mit seiner Fehlerfunktion, wenn
    /// eines folgt.
    Open {
        next: Option<Kmp>,
    },
}

/// Die Klassen mit gierigem Lauf (8.7, Tabelle); die Hoechstlaenge prueft
/// die Extraktion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Int,
    Hex,
    Word,
}

impl Class {
    fn contains(self, b: u8) -> bool {
        match self {
            Class::Int => b.is_ascii_digit(),
            Class::Hex => b.is_ascii_hexdigit(),
            Class::Word => b.is_ascii_alphanumeric() || b == b'_',
        }
    }
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

    /// Wie viele Zeichen des Literals nach `b` als Suffix stehen.
    fn step(&self, mut k: usize, b: u8) -> usize {
        while k > 0 && self.lit[k] != b {
            k = usize::from(self.fail[k - 1]);
        }
        if self.lit[k] == b { k + 1 } else { 0 }
    }
}

/// Ein Muster als Automat seines Durchlaufs.
#[derive(Clone, Debug)]
struct Compiled {
    parts: Vec<Part>,
    has: bool,
}

impl Compiled {
    fn of(pieces: &[PatternPiece], has: bool) -> Option<Compiled> {
        let mut parts = Vec::with_capacity(pieces.len());
        for (i, piece) in pieces.iter().enumerate() {
            let open = || {
                let next = match pieces.get(i + 1) {
                    None => None,
                    Some(PatternPiece::Text(t)) if !t.is_empty() => Some(Kmp::of(t.as_bytes())),
                    // Pruefung 18 laesst keinen Platzhalter direkt folgen.
                    Some(_) => return None,
                };
                Some(Part::Open { next })
            };
            parts.push(match piece {
                PatternPiece::Text(t) if t.is_empty() => continue,
                PatternPiece::Text(t) => Part::Lit(t.as_bytes().to_vec()),
                PatternPiece::Any => open()?,
                PatternPiece::Capture { kind, .. } => match kind {
                    CaptureKind::Int => Part::Class(Class::Int),
                    CaptureKind::Hex => Part::Class(Class::Hex),
                    CaptureKind::Word => Part::Class(Class::Word),
                    CaptureKind::Str(_) => open()?,
                    CaptureKind::Float => return None,
                },
            });
        }
        Some(Compiled { parts, has })
    }

    /// Der Beginn von Baustein `i`.
    fn enter(&self, i: usize) -> At {
        let part = i as u16;
        match self.parts.get(i) {
            None => At::Done,
            Some(Part::Lit(_)) => At::Lit { part, k: 0 },
            Some(Part::Class(_)) => At::Run { part, phase: Phase::Start },
            Some(Part::Open { .. }) => At::Open { part, kmp: 0 },
        }
    }

    /// Ein Zeichen weiter; `None`, wenn der Durchlauf scheitert.
    fn step(&self, at: At, b: u8) -> Option<At> {
        match at {
            At::Hit => Some(At::Hit),
            // `matches` verlangt die ganze Zeile, `has` nimmt jeden Rest.
            At::Done => self.has.then_some(At::Hit),
            At::Lit { part, k } => {
                let Some(Part::Lit(lit)) = self.parts.get(usize::from(part)) else { return None };
                if lit[usize::from(k)] != b {
                    return None;
                }
                let k = k + 1;
                if usize::from(k) == lit.len() {
                    Some(self.enter(usize::from(part) + 1))
                } else {
                    Some(At::Lit { part, k })
                }
            }
            At::Run { part, phase } => {
                let Some(Part::Class(class)) = self.parts.get(usize::from(part)) else { return None };
                let run = |phase| Some(At::Run { part, phase });
                match (class, phase) {
                    (Class::Int, Phase::Start) if b == b'+' || b == b'-' => run(Phase::Sign),
                    (Class::Hex, Phase::Start) if b == b'0' => run(Phase::Zero),
                    (Class::Hex, Phase::Zero) if b == b'x' => run(Phase::Prefix),
                    (_, _) if class.contains(b) => run(Phase::Digits),
                    (_, Phase::Start | Phase::Sign | Phase::Prefix) => None,
                    // Das erste Zeichen ausserhalb der Klasse gehoert dem
                    // naechsten Baustein.
                    (_, Phase::Zero | Phase::Digits) => self.step(self.enter(usize::from(part) + 1), b),
                }
            }
            At::Open { part, kmp } => {
                let Some(Part::Open { next }) = self.parts.get(usize::from(part)) else { return None };
                let Some(kmp_lit) = next else { return Some(At::Open { part, kmp: 0 }) };
                let k = kmp_lit.step(usize::from(kmp), b);
                // Das erste Vorkommen beendet den Platzhalter.
                if k == kmp_lit.lit.len() {
                    Some(self.enter(usize::from(part) + 2))
                } else {
                    Some(At::Open { part, kmp: k as u16 })
                }
            }
        }
    }

    /// Traegt der Durchlauf, wenn die Zeile hier endet?
    fn final_at_eof(&self, at: At) -> bool {
        match at {
            At::Done | At::Hit => true,
            At::Lit { .. } => false,
            At::Run { part, phase, .. } => {
                matches!(phase, Phase::Zero | Phase::Digits) && self.final_at_eof(self.enter(usize::from(part) + 1))
            }
            At::Open { part, .. } => {
                matches!(self.parts.get(usize::from(part)), Some(Part::Open { next: None, .. }))
                    && self.final_at_eof(self.enter(usize::from(part) + 1))
            }
        }
    }

    /// `has`: Steht der Treffer dieses Fadens fest, gleich was folgt?
    fn complete(&self, at: At) -> bool {
        match at {
            At::Done | At::Hit => true,
            At::Lit { .. } => false,
            // Eine `0` in `hex` steht noch nicht fest: Folgt `x`, war sie
            // das Praefix, und eine Ziffer muss noch kommen.
            At::Run { part, phase, .. } => phase == Phase::Digits && self.complete(self.enter(usize::from(part) + 1)),
            // `_` und `str` am Ende nehmen jeden Rest.
            At::Open { part, .. } => matches!(self.parts.get(usize::from(part)), Some(Part::Open { next: None })),
        }
    }
}

/// Die Alphabetklassen: Bytes, die alle Muster gleich behandeln, teilen
/// eine Spalte (11.2). Liefert die Abbildung und je Klasse ein Byte, an dem
/// die Uebergaenge gerechnet werden.
fn alphabet(patterns: &[Compiled]) -> (Vec<u8>, Vec<u8>) {
    let mut literal = [false; 256];
    let mut kinds: Vec<Class> = Vec::new();
    for c in patterns {
        for part in &c.parts {
            match part {
                Part::Lit(lit) => lit.iter().for_each(|b| literal[usize::from(*b)] = true),
                Part::Open { next: Some(k), .. } => k.lit.iter().for_each(|b| literal[usize::from(*b)] = true),
                Part::Open { next: None, .. } => {}
                Part::Class(class) => {
                    if !kinds.contains(class) {
                        kinds.push(*class);
                    }
                }
            }
        }
    }
    // Vorzeichen, `0` und `x` sind eigene Faelle der Klassen.
    let special = |b: u8| match b {
        b'+' | b'-' => kinds.contains(&Class::Int),
        b'0' | b'x' => kinds.contains(&Class::Hex),
        _ => false,
    };
    let mut signatures: BTreeMap<(Option<u8>, Vec<bool>), u8> = BTreeMap::new();
    let mut classes = vec![0u8; 256];
    let mut representatives = Vec::new();
    for b in 0..=255u8 {
        let own = (literal[usize::from(b)] || special(b)).then_some(b);
        let sig = (own, kinds.iter().map(|k| k.contains(b)).collect());
        let next = signatures.len();
        let class = *signatures.entry(sig).or_insert_with(|| {
            representatives.push(b);
            next as u8
        });
        classes[usize::from(b)] = class;
    }
    (classes, representatives)
}
