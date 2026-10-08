//! Musterabgleich als Automat (M11 Schritt 27c-2, Referenz 8.7). Der
//! Durchlauf des Interpreters (`takt_interp::pattern::walk`) ist
//! deterministisch: Von einer Stelle aus haengt sein Weg nur vom Baustein,
//! dem Stand darin und dem restlichen Text ab. Darum laufen alle Starts
//! zugleich als Faeden ueber die Stellen des Texts; treffen zwei Faeden
//! denselben Zustand, gilt der frueher gestartete, denn `has` nimmt den
//! fruehesten Start, ab dem der Durchlauf gelingt. Literale und offene Enden
//! (`str<N>`, `{_}`) gehen Byte fuer Byte, eine Klasse (`int`, `hex`,
//! `word`) springt von jeder Stelle aus an jedes moegliche Ende ihres Laufs.
//! So waechst die Kodierung mit der Laenge des Texts, nicht mit ihrem
//! Quadrat.

use std::collections::BTreeMap;
use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::MatchKind;
use takt_mir::pattern::{CaptureKind, PatternPiece};

use super::text::{Pos, Text, lead};
use super::value::V;
use super::{R, no};
use crate::term::{Op, Term};

/// Ein Baustein, wie der Automat ihn fuehrt.
enum Seg {
    Lit(Vec<u8>),
    Int,
    Hex,
    Word,
    /// `str<N>` mit Hoechstlaenge und Kapazitaet, `{_}` ohne; `until` ist das
    /// folgende Literal, an dem es endet.
    Open {
        limit: Option<i64>,
        until: Option<Vec<u8>>,
    },
}

/// Was ein Faden gebunden hat: eine Zahl oder einen Ausschnitt aus Start,
/// Laenge und Kapazitaet.
#[derive(Clone)]
enum Cap {
    Int(Term),
    Span(Term, Term, u32),
}

/// Ein Faden: wann er lebt, wo er begann, was er band und wo sein
/// laufender Baustein begann.
#[derive(Clone)]
struct Thread {
    alive: Term,
    start: Term,
    caps: Vec<Cap>,
    from: Term,
}

/// Baustein und Stand darin: bei einem Literal die gelesenen Bytes, bei
/// `str<N>` die gelesenen Bytes, sonst null.
type Key = (usize, i64);
type Threads = BTreeMap<Key, Thread>;

fn int(i: i64) -> Term {
    Term::int(i)
}

fn add(a: Term, b: Term) -> Term {
    Term::bin(Op::Add, a, b)
}

fn sub(a: Term, b: Term) -> Term {
    Term::bin(Op::Sub, a, b)
}

fn within(x: &Term, lo: u8, hi: u8) -> Term {
    Term::and(vec![Term::bin(Op::Ge, x.clone(), int(i64::from(lo))), Term::bin(Op::Le, x.clone(), int(i64::from(hi)))])
}

fn is(x: &Term, c: u8) -> Term {
    Term::eq(x.clone(), int(i64::from(c)))
}

fn digit(c: &Term) -> Term {
    within(c, b'0', b'9')
}

fn hex_digit(c: &Term) -> Term {
    Term::or(vec![digit(c), within(c, b'a', b'f'), within(c, b'A', b'F')])
}

fn word_char(c: &Term) -> Term {
    Term::or(vec![digit(c), within(c, b'a', b'z'), within(c, b'A', b'Z'), is(c, b'_')])
}

/// Der Wert einer Ziffer zur Basis 16, die Dezimalziffern eingeschlossen.
fn digit_value(c: &Term) -> Term {
    Term::ite(
        digit(c),
        sub(c.clone(), int(i64::from(b'0'))),
        Term::ite(
            within(c, b'a', b'f'),
            sub(c.clone(), int(i64::from(b'a') - 10)),
            sub(c.clone(), int(i64::from(b'A') - 10)),
        ),
    )
}

/// Die Bausteine eines Musters.
fn segments(pieces: &[PatternPiece], span: Span) -> R<Vec<Seg>> {
    let mut out = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        let until = pieces[i + 1..].iter().find_map(|p| match p {
            PatternPiece::Text(t) => Some(t.as_bytes().to_vec()),
            _ => None,
        });
        out.push(match piece {
            PatternPiece::Text(t) => Seg::Lit(t.as_bytes().to_vec()),
            PatternPiece::Any => Seg::Open { limit: None, until },
            PatternPiece::Capture { kind, .. } => match kind {
                CaptureKind::Int => Seg::Int,
                CaptureKind::Hex => Seg::Hex,
                CaptureKind::Word => Seg::Word,
                CaptureKind::Str(n) => Seg::Open { limit: Some(i64::from(*n)), until },
                CaptureKind::Float => {
                    return no("`{x:float}` im Muster: das Modell rechnet keine Dezimalzahl um", span);
                }
            },
        });
    }
    Ok(out)
}

/// Zwei Faeden im selben Zustand: Der frueher gestartete gilt.
fn merge(a: Thread, b: Thread) -> Thread {
    let first = Term::and(vec![
        a.alive.clone(),
        Term::or(vec![b.alive.clone().not(), Term::bin(Op::Le, a.start.clone(), b.start.clone())]),
    ]);
    let pick = |x: Term, y: Term| Term::ite(first.clone(), x, y);
    let caps = a
        .caps
        .into_iter()
        .zip(b.caps)
        .map(|(x, y)| match (x, y) {
            (Cap::Int(x), Cap::Int(y)) => Cap::Int(pick(x, y)),
            (Cap::Span(s, l, w), Cap::Span(t, m, _)) => Cap::Span(pick(s, t), pick(l, m), w),
            (x, _) => x,
        })
        .collect();
    Thread { alive: Term::or(vec![a.alive, b.alive]), start: pick(a.start, b.start), caps, from: pick(a.from, b.from) }
}

fn put(ts: &mut Threads, key: Key, t: Thread) {
    if t.alive.is_bool(false) {
        return;
    }
    let merged = match ts.remove(&key) {
        None => t,
        Some(old) => merge(old, t),
    };
    ts.insert(key, merged);
}

/// Ein Faden mit anderer Lebensbedingung.
fn under(t: &Thread, alive: Term) -> Thread {
    Thread { alive: Term::and(vec![t.alive.clone(), alive]), ..t.clone() }
}

/// Ein Faden am Anfang des naechsten Bausteins, mit einer Bindung mehr.
fn advance(t: &Thread, alive: Term, cap: Option<Cap>, at: i64) -> Thread {
    let mut caps = t.caps.clone();
    caps.extend(cap);
    Thread { alive: Term::and(vec![t.alive.clone(), alive]), start: t.start.clone(), caps, from: int(at) }
}

/// Gleicht ein Textmuster ab (`pattern::match_text`, `match_has`): ob es
/// trifft und was es bindet.
pub(super) fn text_match(pieces: &[PatternPiece], kind: MatchKind, text: &Text, span: Span) -> R<(Term, Vec<V>)> {
    let segs = segments(pieces, span)?;
    let done = segs.len();
    let cap = text.bytes.len() as i64;
    let byte = |i: i64| text.at(&Pos::At(i));
    let mut ts = Threads::new();
    // Was ein Sprung an einer spaeteren Stelle abliefert.
    let mut ahead: BTreeMap<i64, Threads> = BTreeMap::new();
    let mut best: Option<Thread> = None;
    for i in 0..=cap {
        for (key, t) in ahead.remove(&i).unwrap_or_default() {
            put(&mut ts, key, t);
        }
        // Ein Start: bei `matches` am Anfang, bei `has` an jeder
        // Zeichengrenze bis zum Ende.
        let spawn = match kind {
            MatchKind::Matches => (i == 0).then(|| Term::bool(true)),
            MatchKind::Has => {
                let boundary = match text.bytes.get(i as usize) {
                    Some(b) => Term::or(vec![Term::eq(int(i), text.len.clone()), lead(b)]),
                    None => Term::bool(true),
                };
                Some(Term::and(vec![Term::bin(Op::Le, int(i), text.len.clone()), boundary]))
            }
        };
        if let Some(alive) = spawn {
            put(&mut ts, (0, 0), Thread { alive, start: int(i), caps: Vec::new(), from: int(i) });
        }
        // Ohne Verbrauch: Ein offenes Ende endet vor seinem Literal oder am
        // Textende, ein fertiger Faden trifft. Die Bausteine der Reihe nach,
        // damit ein Faden mehrere Schritte zugleich tun kann.
        for s in 0..=done {
            let keys: Vec<Key> = ts.keys().filter(|(seg, _)| *seg == s).copied().collect();
            for key in keys {
                let Some(t) = ts.remove(&key) else { continue };
                match segs.get(s) {
                    // Hinter dem letzten Baustein: Der Faden trifft.
                    None => {
                        let ok = match kind {
                            MatchKind::Matches => Term::eq(int(i), text.len.clone()),
                            MatchKind::Has => Term::bool(true),
                        };
                        let won = under(&t, ok);
                        best = Some(match best {
                            None => won,
                            Some(old) => merge(old, won),
                        });
                    }
                    Some(Seg::Open { limit, until }) => {
                        let ends = match until {
                            Some(lit) => {
                                Term::and(lit.iter().enumerate().map(|(k, c)| is(&byte(i + k as i64), *c)).collect())
                            }
                            None => Term::eq(int(i), text.len.clone()),
                        };
                        let captured = limit.map(|n| Cap::Span(t.from.clone(), int(key.1), n as u32));
                        put(&mut ts, (s + 1, 0), advance(&t, ends.clone(), captured, i));
                        put(&mut ts, key, under(&t, ends.not()));
                    }
                    _ => put(&mut ts, key, t),
                }
            }
        }
        // Mit Verbrauch des Bytes an `i`.
        let b = byte(i);
        let mut next = Threads::new();
        for (key, t) in std::mem::take(&mut ts) {
            let (s, k) = key;
            match &segs[s] {
                Seg::Lit(lit) => {
                    let hit = is(&b, lit[k as usize]);
                    if k as usize + 1 == lit.len() {
                        put(&mut next, (s + 1, 0), advance(&t, hit, None, i + 1));
                    } else {
                        put(&mut next, (s, k + 1), under(&t, hit));
                    }
                }
                Seg::Open { limit, .. } => {
                    let inside = Term::bin(Op::Lt, int(i), text.len.clone());
                    match limit {
                        Some(n) if k >= *n => {}
                        Some(_) => put(&mut next, (s, k + 1), under(&t, inside)),
                        None => put(&mut next, key, under(&t, inside)),
                    }
                }
                Seg::Int | Seg::Hex | Seg::Word => {
                    for (end, alive, captured) in run(&segs[s], text, i) {
                        if end <= cap {
                            put(ahead.entry(end).or_default(), (s + 1, 0), advance(&t, alive, Some(captured), end));
                        }
                    }
                }
            }
        }
        ts = next;
    }
    let Some(t) = best else { return Ok((Term::bool(false), Vec::new())) };
    let caps = t
        .caps
        .iter()
        .map(|c| match c {
            Cap::Int(x) => V::Leaf(x.clone()),
            Cap::Span(start, len, width) => {
                text.slice(&Pos::Sym(start.clone()), len, i64::from(*width)).value(*width, None)
            }
        })
        .collect();
    Ok((t.alive, caps))
}

/// Der Lauf einer Klasse ab `p` (`bounded`, `signed`, `hex`): je moegliches
/// Ende, wann der Lauf genau dort endet, und was er bindet. Ein Lauf ueber
/// seine Hoechstlaenge trifft nicht, eine Zahl ausserhalb von `i64` auch nicht.
fn run(seg: &Seg, text: &Text, p: i64) -> Vec<(i64, Term, Cap)> {
    let byte = |i: i64| text.at(&Pos::At(i));
    match seg {
        Seg::Int => {
            let c = byte(p);
            let signed = Term::or(vec![is(&c, b'+'), is(&c, b'-')]);
            let minus = is(&c, b'-');
            let mut out = digits(text, p, 19, 10, &digit, false)
                .into_iter()
                .map(|(end, ok, v)| (end, Term::and(vec![signed.clone().not(), ok]), Cap::Int(v)))
                .collect::<Vec<_>>();
            let plus = digits(text, p + 1, 19, 10, &digit, false);
            let neg = digits(text, p + 1, 19, 10, &digit, true);
            for ((end, ok_p, v_p), (_, ok_n, v_n)) in plus.into_iter().zip(neg) {
                let ok = Term::ite(minus.clone(), ok_n, ok_p);
                out.push((end, Term::and(vec![signed.clone(), ok]), Cap::Int(Term::ite(minus.clone(), v_n, v_p))));
            }
            out
        }
        Seg::Hex => {
            let prefix = Term::and(vec![is(&byte(p), b'0'), is(&byte(p + 1), b'x')]);
            let mut out: Vec<(i64, Term, Cap)> = digits(text, p + 2, 16, 16, &hex_digit, false)
                .into_iter()
                .map(|(end, ok, v)| (end, Term::and(vec![prefix.clone(), ok]), Cap::Int(v)))
                .collect();
            out.extend(
                digits(text, p, 16, 16, &hex_digit, false)
                    .into_iter()
                    .map(|(end, ok, v)| (end, Term::and(vec![prefix.clone().not(), ok]), Cap::Int(v))),
            );
            out
        }
        Seg::Word => {
            let mut prefix = Term::bool(true);
            let mut out = Vec::new();
            for r in 1..=64i64 {
                prefix = Term::and(vec![prefix, word_char(&byte(p + r - 1))]);
                let exact = Term::and(vec![prefix.clone(), word_char(&byte(p + r)).not()]);
                out.push((p + r, exact, Cap::Span(int(p), int(r), 64)));
            }
            out
        }
        Seg::Lit(_) | Seg::Open { .. } => Vec::new(),
    }
}

/// Die Ziffernlaeufe ab `p`: je Laenge `r` das Ende, wann der Lauf genau so
/// lang ist und `i64` haelt, und sein Wert; `negative` zaehlt nach unten,
/// damit `i64::MIN` passt.
fn digits(
    text: &Text,
    p: i64,
    max: i64,
    base: i64,
    class: &dyn Fn(&Term) -> Term,
    negative: bool,
) -> Vec<(i64, Term, Term)> {
    let byte = |i: i64| text.at(&Pos::At(i));
    let mut prefix = Term::bool(true);
    let mut v = int(0);
    let mut over = Term::bool(false);
    let mut out = Vec::new();
    for r in 1..=max {
        let c = byte(p + r - 1);
        prefix = Term::and(vec![prefix, class(&c)]);
        let d = digit_value(&c);
        let scaled = Term::bin(Op::Mul, v.clone(), int(base));
        let (next, wraps) = if negative {
            (sub(scaled.clone(), d.clone()), Term::bin(Op::SubOverflows, scaled, d))
        } else {
            (add(scaled.clone(), d.clone()), Term::bin(Op::AddOverflows, scaled, d))
        };
        over = Term::or(vec![over, Term::bin(Op::MulOverflows, v.clone(), int(base)), wraps]);
        v = next;
        let exact = Term::and(vec![prefix.clone(), class(&byte(p + r)).not(), over.clone().not()]);
        out.push((p + r, exact, v.clone()));
    }
    out
}
