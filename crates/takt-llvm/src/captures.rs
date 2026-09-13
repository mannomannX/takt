//! Die Werte hinter den Platzhaltern eines Musters (8.7).
//!
//! **Was der Automat nicht kann.** Der DFA (`dfa.rs`) entscheidet, *ob*
//! ein Muster trifft — er laeuft ueber Zeichenklassen und kennt darum
//! die Grenzen eines Platzhalters, nicht seinen Wert. Fuer `{n:int}`
//! braucht es einen zweiten Durchlauf, der die Spannen misst und die
//! Ziffern in eine Zahl verwandelt.
//!
//! **Warum als IR und nicht als Aufruf in die Runtime.** Die Regeln
//! stehen in `takt-match`, und der naheliegende Weg waere, sie von dort
//! zu rufen. Er scheitert an der Naht: Ein Aufruf muesste die Bausteine
//! als Feld von Deskriptoren uebergeben, also ueber Rohzeiger — und
//! `unsafe_code = "forbid"` gilt workspace-weit (13.4 nennt das Verbot
//! einen Zertifizierungsgrund). Die Naht zu ziehen hiesse, sie an genau
//! der Stelle zu ziehen, an der die Sprache sie verbietet.
//!
//! 11.2 zeigt zugleich, dass hier die richtige Seite ist: Der
//! Handler-Dispatch laeuft „mit vorkompilierten DFA-Tabellen", die in
//! `takt size` erscheinen (11.5) und auf XIP-Zielen im RAM liegen
//! (12.3). Der Vergleich ist damit Sache des erzeugten Codes. Die
//! Extraktion gehoert zu demselben Muster; sie ueber eine Naht zu
//! schieben hiesse, ein Muster auf zwei Seiten zu bearbeiten.
//!
//! **`takt-match` bleibt die Quelle der Regeln.** Nicht als geteilter
//! Code, sondern als Orakel: `crates/takt-llvm/tests/captures.rs` misst
//! den erzeugten Code gegen es. Das ist die schaerfere Pruefung — zwei
//! Implementierungen, die uebereinstimmen muessen, decken auf, was eine
//! gemeinsame stillschweigend teilt (so fand FB-114 seinen Fehler).
//!
//! **Ein Durchlauf, kein Backtracking.** 9.7 nennt `match` „total,
//! O(len)". Jeder Baustein verbraucht ein Praefix des Rests: Ein
//! Literal vergleicht, eine Klasse nimmt ihr laengstes Praefix, ein
//! offenes Ende sucht das naechste Literal. Weil jede Klasse dort endet,
//! wo ein Zeichen nicht mehr zu ihr gehoert, ist jede Grenze eindeutig.

use takt_mir::pattern::{CaptureKind, PatternPiece};

use crate::emit::{Module, Reg};
use crate::expr::NotYet;

/// Wohin die Werte eines Musters gehen (8.7, Wrapper-Regel).
///
/// Die Bindung ist ein Record, dessen erste Felder die Platzhalter sind.
/// Ohne Bindung laeuft der Vergleich trotzdem — er sagt dann nur, *ob*
/// das Muster trifft.
pub struct Target<'a> {
    /// Der Record im Zustand der Maschine.
    pub slot: Reg,
    /// Sein Typ, fuer die Adressrechnung: Die Ausrichtung kennt erst das
    /// Datenlayout des Targets, also adressiert `getelementptr` ueber den
    /// Feldindex statt ueber einen gerechneten Versatz (11.2).
    pub record: &'a crate::ty::LlvmType,
    /// Index und Typ je Capture, in Musterreihenfolge.
    pub fields: &'a [(u32, crate::ty::LlvmType)],
}

/// Die Hoechstlaenge eines `word` (8.7).
const WORD_MAX: u32 = 64;

/// Die Hoechstzahl der Ziffern eines `int`: 19, wie `i64` sie traegt
/// (3.1, 8.7).
const INT_MAX_DIGITS: u32 = 19;

/// Laeuft die Bausteine ab und legt die Werte in die Bindung (8.7).
///
/// `text` zeigt auf `{ i32 len, [N x i8] bytes, … }`, `binding` auf den
/// Record der Bindung, dessen erste Felder die Captures sind (8.7,
/// Wrapper-Regel). `from` ist die Startposition — null fuer `matches`,
/// der Laufindex fuer `has`.
///
/// Das Ergebnis ist ein `i1`: Traegt der Durchlauf? Bei `matches` prueft
/// der Aufrufer zusaetzlich, dass der ganze Text verbraucht ist.
pub fn walk(
    pieces: &[PatternPiece],
    text: Reg,
    target: Option<&Target<'_>>,
    from: Reg,
    m: &mut Module,
) -> Result<(Reg, Reg), NotYet> {
    let k = m.next_label();
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 4"));

    // `at` ist die Position im Text, `ok` das Urteil. Beide stehen im
    // Speicher, weil die Bausteine Verzweigungen erzeugen und ein
    // SSA-Wert ueber sie hinweg eine Phi-Kette braeuchte.
    let at_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 {from}, ptr {at_ptr}"));
    let ok_ptr = m.inst("alloca i1");
    m.void_inst(&format!("store i1 true, ptr {ok_ptr}"));

    let end_at = format!("cap{k}_ende");
    let mut cap_index = 0usize;
    for (i, piece) in pieces.iter().enumerate() {
        let go_on = format!("cap{k}_{i}");
        // Ein gescheiterter Baustein ueberspringt den Rest: Der
        // Durchlauf bricht ab, wie `walk` in `takt-match`.
        let ok = m.inst(&format!("load i1, ptr {ok_ptr}"));
        m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{end_at}"));
        m.label(&go_on);
        match piece {
            PatternPiece::Text(lit) => literal(lit.as_bytes(), bytes, len, at_ptr, ok_ptr, m),
            PatternPiece::Any => {
                let upto = open_end(&pieces[i + 1..], bytes, len, at_ptr, ok_ptr, None, m)?;
                let _ = upto;
            }
            PatternPiece::Capture { kind, .. } => {
                let before = m.inst(&format!("load i32, ptr {at_ptr}"));
                // Die Ziffern beginnen hinter dem Vorzeichen und hinter
                // `0x`; `before` zeigt davor. Der Wert entsteht aus den
                // Ziffern, das Vorzeichen kommt getrennt — sonst liefe
                // `-` in die Stellenrechnung.
                let (start, negative) = match kind {
                    CaptureKind::Int => signed(bytes, len, at_ptr, ok_ptr, m),
                    CaptureKind::Hex => (hex(bytes, len, at_ptr, ok_ptr, m), None),
                    CaptureKind::Word => {
                        bounded(bytes, len, at_ptr, ok_ptr, Class::Word, WORD_MAX, m);
                        (before, None)
                    }
                    CaptureKind::Str(n) => {
                        open_end(&pieces[i + 1..], bytes, len, at_ptr, ok_ptr, Some(*n), m)?;
                        (before, None)
                    }
                    // 4.2 verlangt bitgleiche Fliesskommaergebnisse. Eine
                    // eigene Dezimal-nach-Binaer-Konversion waere eine
                    // zweite Rundungsquelle neben `libtaktm` — dieselbe
                    // Entscheidung wie in `takt-match`.
                    CaptureKind::Float => return Err(NotYet { what: "`{x:float}` im Handler-Muster" }),
                };
                // `{_}` bindet nicht (8.7) und belegt kein Feld.
                if let Some((z, (idx, ty))) = target.zip(target.and_then(|z| z.fields.get(cap_index))) {
                    let end_pos = m.inst(&format!("load i32, ptr {at_ptr}"));
                    let where_to = Slot { target: z, index: *idx, ty, start, end_at: end_pos, negative };
                    store_capture(kind, &where_to, bytes, m)?;
                }
                cap_index += 1;
            }
        }
    }
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&end_at);
    let ok = m.inst(&format!("load i1, ptr {ok_ptr}"));
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    Ok((ok, at))
}

/// Ein Literal: Es muss Byte fuer Byte stehen (8.7).
///
/// Die Laenge ist zur Uebersetzungszeit bekannt, also wird der Vergleich
/// abgerollt — bei den typischen zwei bis zehn Zeichen ist das kuerzer
/// als eine Schleife mit Zaehler und drei Marken.
fn literal(lit: &[u8], bytes: Reg, len: Reg, at_ptr: Reg, ok_ptr: Reg, m: &mut Module) {
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let n = lit.len() as u32;
    let upto = m.inst(&format!("add i32 {at}, {n}"));
    // Passt das Literal ueberhaupt noch in den Rest?
    let fits = m.inst(&format!("icmp sle i32 {upto}, {len}"));
    let mut same = fits;
    for (j, b) in lit.iter().enumerate() {
        // Ausserhalb des Texts wird nicht gelesen: Der Index wird auf
        // eine gueltige Stelle geklemmt, und `gleich` ist dann ohnehin
        // falsch. Ein Sprung je Byte waere die Alternative und braeuchte
        // eine Marke je Zeichen.
        let idx = m.inst(&format!("add i32 {at}, {j}"));
        let in_range = m.inst(&format!("icmp slt i32 {idx}, {len}"));
        let safe_idx = m.inst(&format!("select i1 {in_range}, i32 {idx}, i32 0"));
        let p = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {safe_idx}"));
        let got = m.inst(&format!("load i8, ptr {p}"));
        let eq = m.inst(&format!("icmp eq i8 {got}, {b}"));
        same = m.inst(&format!("and i1 {same}, {eq}"));
    }
    let old = m.inst(&format!("load i1, ptr {ok_ptr}"));
    let new_val = m.inst(&format!("and i1 {old}, {same}"));
    m.void_inst(&format!("store i1 {new_val}, ptr {ok_ptr}"));
    // Nur bei Erfolg ruecken; sonst bliebe die Position unbestimmt.
    let go_on = m.inst(&format!("select i1 {same}, i32 {upto}, i32 {at}"));
    m.void_inst(&format!("store i32 {go_on}, ptr {at_ptr}"));
}

/// Die Zeichenklassen, die ein Platzhalter verbraucht (8.7).
#[derive(Clone, Copy)]
enum Class {
    /// `0`–`9`.
    Digit,
    /// `0`–`9`, `a`–`f`, `A`–`F`.
    Hex,
    /// Buchstaben, Ziffern und `_`.
    Word,
}

impl Class {
    /// Der Test als IR: Gehoert das Byte zur Klasse?
    fn test(self, b: Reg, m: &mut Module) -> Reg {
        let ziffer = |m: &mut Module| {
            let ge = m.inst(&format!("icmp uge i8 {b}, 48"));
            let le = m.inst(&format!("icmp ule i8 {b}, 57"));
            m.inst(&format!("and i1 {ge}, {le}"))
        };
        match self {
            Class::Digit => ziffer(m),
            Class::Hex => {
                let d = ziffer(m);
                let a_ge = m.inst(&format!("icmp uge i8 {b}, 97"));
                let f_le = m.inst(&format!("icmp ule i8 {b}, 102"));
                let klein = m.inst(&format!("and i1 {a_ge}, {f_le}"));
                let a_ge2 = m.inst(&format!("icmp uge i8 {b}, 65"));
                let f_le2 = m.inst(&format!("icmp ule i8 {b}, 70"));
                let gross = m.inst(&format!("and i1 {a_ge2}, {f_le2}"));
                let letter = m.inst(&format!("or i1 {klein}, {gross}"));
                m.inst(&format!("or i1 {d}, {letter}"))
            }
            Class::Word => {
                let d = ziffer(m);
                let a_ge = m.inst(&format!("icmp uge i8 {b}, 97"));
                let z_le = m.inst(&format!("icmp ule i8 {b}, 122"));
                let klein = m.inst(&format!("and i1 {a_ge}, {z_le}"));
                let a_ge2 = m.inst(&format!("icmp uge i8 {b}, 65"));
                let z_le2 = m.inst(&format!("icmp ule i8 {b}, 90"));
                let gross = m.inst(&format!("and i1 {a_ge2}, {z_le2}"));
                let unter = m.inst(&format!("icmp eq i8 {b}, 95"));
                let a = m.inst(&format!("or i1 {klein}, {gross}"));
                let b2 = m.inst(&format!("or i1 {a}, {unter}"));
                m.inst(&format!("or i1 {d}, {b2}"))
            }
        }
    }
}

/// Das laengste Praefix aus Zeichen einer Klasse, hoechstens `max`.
///
/// Ein leeres Praefix ist kein Treffer: Jede Klasse verlangt mindestens
/// ein Zeichen (8.7, `{1,…}`).
fn bounded(bytes: Reg, len: Reg, at_ptr: Reg, ok_ptr: Reg, class_of: Class, max: u32, m: &mut Module) {
    let k = m.next_label();
    let (head, body, done) = (format!("kl{k}"), format!("kl{k}_rumpf"), format!("kl{k}_fertig"));
    let start = m.inst(&format!("load i32, ptr {at_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let in_text = m.inst(&format!("icmp slt i32 {at}, {len}"));
    let taken = m.inst(&format!("sub i32 {at}, {start}"));
    let under_max = m.inst(&format!("icmp slt i32 {taken}, {max}"));
    let may_take = m.inst(&format!("and i1 {in_text}, {under_max}"));
    m.void_inst(&format!("br i1 {may_take}, label %{body}, label %{done}"));

    m.label(&body);
    let p = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {at}"));
    let b = m.inst(&format!("load i8, ptr {p}"));
    let fits = class_of.test(b, m);
    let next = m.inst(&format!("add i32 {at}, 1"));
    let new_val = m.inst(&format!("select i1 {fits}, i32 {next}, i32 {at}"));
    m.void_inst(&format!("store i32 {new_val}, ptr {at_ptr}"));
    m.void_inst(&format!("br i1 {fits}, label %{head}, label %{done}"));

    m.label(&done);
    let end_at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let empty = m.inst(&format!("icmp eq i32 {end_at}, {start}"));
    let has_any = m.inst(&format!("xor i1 {empty}, true"));
    let old = m.inst(&format!("load i1, ptr {ok_ptr}"));
    let new_ok = m.inst(&format!("and i1 {old}, {has_any}"));
    m.void_inst(&format!("store i1 {new_ok}, ptr {ok_ptr}"));
}

/// `{n:int}`: ein optionales `-`, dann Ziffern (8.7).
///
/// Liefert den Beginn der Ziffern und das Vorzeichen. Beides getrennt,
/// weil der Wert nur aus den Ziffern entsteht: Ein `-` in der
/// Stellenrechnung waere eine Ziffer mit dem Wert -3.
fn signed(bytes: Reg, len: Reg, at_ptr: Reg, ok_ptr: Reg, m: &mut Module) -> (Reg, Option<Reg>) {
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let in_text = m.inst(&format!("icmp slt i32 {at}, {len}"));
    let safe = m.inst(&format!("select i1 {in_text}, i32 {at}, i32 0"));
    let p = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {safe}"));
    let b = m.inst(&format!("load i8, ptr {p}"));
    let is_minus = m.inst(&format!("icmp eq i8 {b}, 45"));
    let minus = m.inst(&format!("and i1 {is_minus}, {in_text}"));
    let after = m.inst(&format!("add i32 {at}, 1"));
    let start = m.inst(&format!("select i1 {minus}, i32 {after}, i32 {at}"));
    m.void_inst(&format!("store i32 {start}, ptr {at_ptr}"));
    bounded(bytes, len, at_ptr, ok_ptr, Class::Digit, INT_MAX_DIGITS, m);
    (start, Some(minus))
}

/// `{x:hex}`: ein optionales `0x`, dann Hexziffern (8.7).
///
/// Liefert den Beginn der Ziffern — hinter dem Praefix, wenn eines
/// steht. `takt-match` kennt nur die Kleinschreibung `0x`; `0X` ist
/// darum kein Praefix, sondern eine Null gefolgt von etwas, das keine
/// Hexziffer ist.
fn hex(bytes: Reg, len: Reg, at_ptr: Reg, ok_ptr: Reg, m: &mut Module) -> Reg {
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let two = m.inst(&format!("add i32 {at}, 2"));
    let fits = m.inst(&format!("icmp sle i32 {two}, {len}"));
    let safe0 = m.inst(&format!("select i1 {fits}, i32 {at}, i32 0"));
    let p0 = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {safe0}"));
    let b0 = m.inst(&format!("load i8, ptr {p0}"));
    let one = m.inst(&format!("add i32 {safe0}, 1"));
    let p1 = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {one}"));
    let b1 = m.inst(&format!("load i8, ptr {p1}"));
    let zero = m.inst(&format!("icmp eq i8 {b0}, 48"));
    let x = m.inst(&format!("icmp eq i8 {b1}, 120"));
    let prefix0 = m.inst(&format!("and i1 {zero}, {x}"));
    let prefix = m.inst(&format!("and i1 {prefix0}, {fits}"));
    let start = m.inst(&format!("select i1 {prefix}, i32 {two}, i32 {at}"));
    m.void_inst(&format!("store i32 {start}, ptr {at_ptr}"));
    // 16 Hexziffern sind 64 Bit; mehr traegt `int` nicht (3.1).
    bounded(bytes, len, at_ptr, ok_ptr, Class::Hex, 16, m);
    start
}

/// Ein offenes Ende (`{_}`, `str<N>`): Es endet vor dem naechsten
/// Literal, sonst am Textende (8.7, leftmost-shortest).
fn open_end(
    after: &[PatternPiece],
    bytes: Reg,
    len: Reg,
    at_ptr: Reg,
    ok_ptr: Reg,
    max: Option<u32>,
    m: &mut Module,
) -> Result<Reg, NotYet> {
    // Das naechste Literal begrenzt; ohne eines reicht das offene Ende
    // bis zum Textende.
    let Some(PatternPiece::Text(lit)) = after.first() else {
        let at = m.inst(&format!("load i32, ptr {at_ptr}"));
        let rest = m.inst(&format!("sub i32 {len}, {at}"));
        let target = match max {
            // `str<N>` nimmt hoechstens `N` Zeichen (3.9).
            Some(n) => {
                let too_long = m.inst(&format!("icmp sgt i32 {rest}, {n}"));
                let old = m.inst(&format!("load i1, ptr {ok_ptr}"));
                let fits = m.inst(&format!("xor i1 {too_long}, true"));
                let new_val = m.inst(&format!("and i1 {old}, {fits}"));
                m.void_inst(&format!("store i1 {new_val}, ptr {ok_ptr}"));
                len
            }
            None => len,
        };
        m.void_inst(&format!("store i32 {target}, ptr {at_ptr}"));
        return Ok(target);
    };
    let lit = lit.clone();
    let lit_len = lit.len() as u32;
    let k = m.next_label();
    let (head, body, done) = (format!("oe{k}"), format!("oe{k}_rumpf"), format!("oe{k}_fertig"));
    let start = m.inst(&format!("load i32, ptr {at_ptr}"));
    // `gefunden` merkt die Fundstelle; `-1` heisst „noch nichts".
    let found_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 -1, ptr {found_ptr}"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 {start}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let upto = m.inst(&format!("add i32 {i}, {lit_len}"));
    let in_text = m.inst(&format!("icmp sle i32 {upto}, {len}"));
    let found_at = m.inst(&format!("load i32, ptr {found_ptr}"));
    let open_still = m.inst(&format!("icmp eq i32 {found_at}, -1"));
    let searching = m.inst(&format!("and i1 {in_text}, {open_still}"));
    m.void_inst(&format!("br i1 {searching}, label %{body}, label %{done}"));

    m.label(&body);
    let mut same = m.inst("and i1 true, true");
    for (j, b) in lit.as_bytes().iter().enumerate() {
        let idx = m.inst(&format!("add i32 {i}, {j}"));
        let p = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {idx}"));
        let got = m.inst(&format!("load i8, ptr {p}"));
        let eq = m.inst(&format!("icmp eq i8 {got}, {b}"));
        same = m.inst(&format!("and i1 {same}, {eq}"));
    }
    let here = m.inst(&format!("select i1 {same}, i32 {i}, i32 -1"));
    m.void_inst(&format!("store i32 {here}, ptr {found_ptr}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    let found = m.inst(&format!("load i32, ptr {found_ptr}"));
    let has_any = m.inst(&format!("icmp sge i32 {found}, 0"));
    let old = m.inst(&format!("load i1, ptr {ok_ptr}"));
    let mut new_val = m.inst(&format!("and i1 {old}, {has_any}"));
    // `str<N>`: Die Spanne darf `N` nicht ueberschreiten (3.9).
    if let Some(n) = max {
        let end_at = m.inst(&format!("select i1 {has_any}, i32 {found}, i32 {start}"));
        let width = m.inst(&format!("sub i32 {end_at}, {start}"));
        let fits = m.inst(&format!("icmp sle i32 {width}, {n}"));
        new_val = m.inst(&format!("and i1 {new_val}, {fits}"));
    }
    m.void_inst(&format!("store i1 {new_val}, ptr {ok_ptr}"));
    let target = m.inst(&format!("select i1 {has_any}, i32 {found}, i32 {start}"));
    m.void_inst(&format!("store i32 {target}, ptr {at_ptr}"));
    Ok(target)
}

/// Ein Platzhalter und sein Platz: was `store_capture` braucht.
struct Slot<'a> {
    target: &'a Target<'a>,
    index: u32,
    ty: &'a crate::ty::LlvmType,
    /// Beginn der Ziffern, hinter Vorzeichen und `0x`.
    start: Reg,
    /// Ende der Spanne.
    end_at: Reg,
    /// Das Vorzeichen, falls `signed` eines verbraucht hat.
    negative: Option<Reg>,
}

/// Legt den Wert eines Platzhalters in sein Feld der Bindung (8.7).
///
/// Das Feld wird ueber seinen *Index* adressiert, nicht ueber einen
/// gerechneten Versatz: Die Ausrichtung kennt erst das Datenlayout des
/// Targets, und `LlvmType::size()` laesst sie bewusst weg (11.2).
fn store_capture(kind: &CaptureKind, where_to: &Slot<'_>, bytes: Reg, m: &mut Module) -> Result<(), NotYet> {
    let (field, ty, start, end_at, negative) =
        (where_to.index, where_to.ty, where_to.start, where_to.end_at, where_to.negative);
    let target = m.inst(&format!(
        "getelementptr inbounds {}, ptr {}, i32 0, i32 {field}",
        where_to.target.record, where_to.target.slot
    ));
    match kind {
        CaptureKind::Int => {
            let v = parse_int(bytes, start, end_at, negative, m);
            m.void_inst(&format!("store i64 {v}, ptr {target}"));
        }
        CaptureKind::Hex => {
            let v = parse_hex(bytes, start, end_at, m);
            m.void_inst(&format!("store i64 {v}, ptr {target}"));
        }
        // `word` und `str<N>` liefern Text; er wandert als
        // `{ i32 len, [N x i8] }` in das Feld (3.9).
        CaptureKind::Word | CaptureKind::Str(_) => {
            let crate::ty::LlvmType::Struct(f) = ty else {
                return Err(NotYet { what: "Textcapture ohne Textfeld" });
            };
            let Some(crate::ty::LlvmType::Array(_, cap)) = f.get(1) else {
                return Err(NotYet { what: "Textcapture ohne Puffer" });
            };
            copy_text(bytes, start, end_at, target, ty, *cap, m);
        }
        CaptureKind::Float => return Err(NotYet { what: "`{x:float}` im Handler-Muster" }),
    }
    Ok(())
}

/// Die Ziffern zwischen `start` und `ende` als `i64` (8.7).
///
/// Negativ aufgebaut, wie `takt-match::parse_int`: Der
/// Zweierkomplementbereich ist asymmetrisch, und
/// `-9223372036854775808` waere positiv nicht darstellbar (FB-114,
/// derselbe Fall wie FB-95).
fn parse_int(bytes: Reg, start: Reg, end_at: Reg, negative: Option<Reg>, m: &mut Module) -> Reg {
    let k = m.next_label();
    let (head, body, done) = (format!("pi{k}"), format!("pi{k}_rumpf"), format!("pi{k}_fertig"));
    // Das Vorzeichen kommt von `signed`, das es verbraucht hat; hier
    // zurueckzulesen hiesse, dieselbe Stelle zweimal zu deuten.
    let negative = match negative {
        Some(r) => r,
        None => m.inst("and i1 false, false"),
    };
    let acc_ptr = m.inst("alloca i64");
    m.void_inst(&format!("store i64 0, ptr {acc_ptr}"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 {start}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {end_at}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let at = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {i}"));
    let byte = m.inst(&format!("load i8, ptr {at}"));
    let d8 = m.inst(&format!("sub i8 {byte}, 48"));
    let d = m.inst(&format!("zext i8 {d8} to i64"));
    let acc = m.inst(&format!("load i64, ptr {acc_ptr}"));
    let scaled = m.inst(&format!("mul i64 {acc}, 10"));
    // Negativ aufbauen; am Ende wird bei Bedarf negiert.
    let minus = m.inst(&format!("sub i64 {scaled}, {d}"));
    m.void_inst(&format!("store i64 {minus}, ptr {acc_ptr}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    let acc = m.inst(&format!("load i64, ptr {acc_ptr}"));
    let positive = m.inst(&format!("sub i64 0, {acc}"));
    m.inst(&format!("select i1 {negative}, i64 {acc}, i64 {positive}"))
}

/// Die Hexziffern zwischen `start` und `ende` als `i64` (8.7).
fn parse_hex(bytes: Reg, start: Reg, end_at: Reg, m: &mut Module) -> Reg {
    let k = m.next_label();
    let (head, body, done) = (format!("ph{k}"), format!("ph{k}_rumpf"), format!("ph{k}_fertig"));
    let acc_ptr = m.inst("alloca i64");
    m.void_inst(&format!("store i64 0, ptr {acc_ptr}"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 {start}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {end_at}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let at = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {i}"));
    let byte = m.inst(&format!("load i8, ptr {at}"));
    // Ziffer, Klein- oder Grossbuchstabe: drei Faelle, zwei Auswahlen.
    let is_digit_char = m.inst(&format!("icmp ule i8 {byte}, 57"));
    let is_lower = m.inst(&format!("icmp uge i8 {byte}, 97"));
    let from_digit = m.inst(&format!("sub i8 {byte}, 48"));
    let from_lower = m.inst(&format!("sub i8 {byte}, 87"));
    let from_upper = m.inst(&format!("sub i8 {byte}, 55"));
    let letter = m.inst(&format!("select i1 {is_lower}, i8 {from_lower}, i8 {from_upper}"));
    let d8 = m.inst(&format!("select i1 {is_digit_char}, i8 {from_digit}, i8 {letter}"));
    let d = m.inst(&format!("zext i8 {d8} to i64"));
    let acc = m.inst(&format!("load i64, ptr {acc_ptr}"));
    let scaled = m.inst(&format!("mul i64 {acc}, 16"));
    let sum_val = m.inst(&format!("add i64 {scaled}, {d}"));
    m.void_inst(&format!("store i64 {sum_val}, ptr {acc_ptr}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    m.inst(&format!("load i64, ptr {acc_ptr}"))
}

/// Kopiert die Spanne in ein Textfeld `{ i32 len, [cap x i8] }` (3.9).
fn copy_text(bytes: Reg, start: Reg, end_at: Reg, target: Reg, ty: &crate::ty::LlvmType, cap: u32, m: &mut Module) {
    let k = m.next_label();
    let (head, body, done) = (format!("ct{k}"), format!("ct{k}_rumpf"), format!("ct{k}_fertig"));
    let width = m.inst(&format!("sub i32 {end_at}, {start}"));
    // Der Rand begrenzt; laenger als der Puffer wird nicht kopiert.
    let too_long = m.inst(&format!("icmp sgt i32 {width}, {cap}"));
    let n = m.inst(&format!("select i1 {too_long}, i32 {cap}, i32 {width}"));
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {target}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {n}, ptr {len_ptr}"));
    let buffer = m.inst(&format!("getelementptr inbounds {ty}, ptr {target}, i32 0, i32 1"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {n}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let from_at = m.inst(&format!("add i32 {start}, {i}"));
    let src = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {from_at}"));
    let b = m.inst(&format!("load i8, ptr {src}"));
    let dst = m.inst(&format!("getelementptr inbounds i8, ptr {buffer}, i32 {i}"));
    m.void_inst(&format!("store i8 {b}, ptr {dst}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
}
