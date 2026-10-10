//! Erzeugte Eingaben (M11 Schritt 29a): je Programm Stimuli aus seinen
//! Deklarationen, damit der Vergleich der drei Ausfuehrer die Wege rechnet,
//! die erst eine Eingabe oeffnet (FB-376).
//!
//! **Was erzeugt wird.** Drei Laeufe je Programm:
//!
//! - `erzeugt`: Abschnitte zu [`PHASE`] Ticks. Jeder skalare Input bekommt
//!   wechselnde Werte in seiner Range, dann ihre Grenzen, knapp jenseits
//!   davon (der Rand macht sie `Bad`/`OutOfRange`, 12.6), schlechte und
//!   veraltete Qualitaeten, eine Luecke ueber `max_age` und Spruenge ueber
//!   `max_slew`. Jedes Command kommt in jedem Abschnitt einmal, alle zusammen
//!   einmal; Tunables stehen auf ihren Grenzen und einmal jenseits (die
//!   Zeile wird verworfen, 8.4). Ein Eingabestrom bekommt Elemente, einen
//!   Schwall bis `MAXPT` und, wo sein Element eine Byteform hat, eines, das
//!   der Rand verwirft (8.6) — so kommen auch NaN- und Inf-Bitmuster an den
//!   Rand, die es als Wert nicht gibt (4.1).
//! - `erzeugt abbruch`: dieselben Werte in der Range und Commands, in der
//!   Mitte ein Operator-Abort (5.4).
//! - `erzeugt rand`: Verstoesse gegen den Treibervertrag (12.6 Zeile 2): eine
//!   Luecke in der Folgenummer, ein Schwall ueber `MAXPT`, ein Zeitstempel
//!   ausserhalb seines Fensters, `bad` mit Wert. Das Modell nimmt einen Rand an, der den
//!   Vertrag haelt (13.3); diesen Lauf rechnen Interpreter und erzeugter
//!   Code ([`EDGE`]).
//!
//! Geliefert wird an Inputs mit `hw`-Bindung oder ohne; einen `sim`-Input
//! speist das Modell des Programms (8.3). Alles ist deterministisch: Derselbe
//! Korpus ergibt dieselben Laeufe.

use std::fmt::Write as _;

use takt_interp::Value;
use takt_interp::trace::value_text;
use takt_mir::TypeId;
use takt_mir::program::{Binding, Channel, Direction, Program};
use takt_mir::types::{Const, FloatWidth, Type};

use crate::cases::Case;

/// Ticks je Abschnitt des Laufs `erzeugt`.
pub const PHASE: u64 = 6;

/// Die Laeufe, die das Modell nicht rechnet, mit Grund.
pub const EDGE: &str = "erzeugt rand";

/// Wie ein Wert aus seinem Typ gewaehlt wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pick {
    /// Ein wechselnder Wert im Innern der Range.
    Mid(u64),
    /// Die Untergrenze.
    Lo,
    /// Die Obergrenze.
    Hi,
    /// Knapp unter der Untergrenze.
    BelowLo,
    /// Knapp ueber der Obergrenze.
    AboveHi,
}

impl Pick {
    /// Die Wahl fuer die Teile eines zusammengesetzten Werts, die keine
    /// Grenze ueberschreiten koennen.
    fn inside(self) -> Pick {
        match self {
            Pick::BelowLo => Pick::Lo,
            Pick::AboveHi => Pick::Hi,
            other => other,
        }
    }
}

/// Die erzeugten Laeufe des Programms; ohne Eingaben, Commands und
/// Tunables keine.
pub fn cases(p: &Program) -> Vec<Case> {
    let inputs = inputs(p);
    let commands = !p.commands.is_empty();
    let tunables = p.params.iter().any(|t| t.tunable);
    if inputs.is_empty() && !commands && !tunables {
        return Vec::new();
    }
    let mut out = vec![sweep(p, &inputs), abort(p, &inputs)];
    if !inputs.is_empty() {
        out.push(edge(p, &inputs));
    }
    out
}

/// Die Inputs, an die der Stimulus liefert: `hw`-gebunden oder ohne Bindung,
/// ohne `sys/outer_fault` — den stellt der Kern (13.3).
fn inputs(p: &Program) -> Vec<(usize, &Channel)> {
    let outer = takt_mir::sys::outer_fault(p);
    p.channels
        .iter()
        .enumerate()
        .filter(|(i, c)| c.dir == Direction::Input && !matches!(c.binding, Binding::Sim(_)) && outer != Some(*i))
        .collect()
}

/// Die Abschnitte von `erzeugt`, in dieser Folge.
const PHASES: [&str; 9] = ["mitte", "unten", "oben", "unter", "ueber", "qualitaet", "luecke", "spruenge", "zurueck"];

/// Der Lauf `erzeugt`.
fn sweep(p: &Program, inputs: &[(usize, &Channel)]) -> Case {
    let mut s = String::new();
    let end = PHASE * PHASES.len() as u64;
    for k in 0..end {
        let phase = PHASES[(k / PHASE) as usize];
        let first = k % PHASE == 0;
        for (_, c) in inputs {
            match p.types.get(c.ty) {
                Type::Stream(elem) => stream_line(&mut s, p, c, *elem, k, phase, first),
                _ => scalar_line(&mut s, p, c, k, phase),
            }
        }
        commands(&mut s, p, k);
        if first {
            tunes(&mut s, p, k, phase);
        }
    }
    Case { label: "erzeugt".into(), stimulus: s, ticks: end + 2 * PHASE }
}

/// Der Lauf `erzeugt abbruch`: Werte in der Range, Commands, in der Mitte
/// ein Operator-Abort.
fn abort(p: &Program, inputs: &[(usize, &Channel)]) -> Case {
    let mut s = String::new();
    let end = 4 * PHASE;
    for k in 0..end {
        for (_, c) in inputs {
            match p.types.get(c.ty) {
                Type::Stream(elem) => stream_line(&mut s, p, c, *elem, k, "mitte", false),
                _ => scalar_line(&mut s, p, c, k, "mitte"),
            }
        }
        commands(&mut s, p, k);
        if k == end / 2 {
            s.push_str(&format!("t={k} abort\n"));
        }
    }
    Case { label: "erzeugt abbruch".into(), stimulus: s, ticks: end + PHASE }
}

/// Der Lauf `erzeugt rand`: Verstoesse gegen den Treibervertrag.
fn edge(p: &Program, inputs: &[(usize, &Channel)]) -> Case {
    let mut s = String::new();
    let end = 3 * PHASE;
    let tick = p.config.tick.max(1);
    for k in 0..end {
        for (_, c) in inputs {
            let Type::Stream(elem) = p.types.get(c.ty) else {
                // Ein Zeitstempel hinter dem Fenster seines Ticks, dann `bad`
                // mit Wert.
                if let Some(text) = scalar_text(p, c.ty, Pick::Mid(k)) {
                    let odd = match k {
                        _ if k == PHASE => format!(" t={}", (k as i64 + 1) * tick + 1),
                        _ if k == 2 * PHASE => " bad reason=Implausible".to_string(),
                        _ => String::new(),
                    };
                    let _ = writeln!(s, "t={k} in {} {text}{odd}", c.name);
                }
                continue;
            };
            let Some(text) = element_text(p, *elem, k) else { continue };
            match k {
                // Eine Luecke in der Folgenummer.
                _ if k == PHASE => {
                    let _ = writeln!(s, "t={k} in {} {text} seq={}", c.name, k + 3);
                }
                // Ein Schwall ueber `MAXPT`.
                _ if k == 2 * PHASE => {
                    let n = takt_hal::edge::maxpt_of(c, tick).map_or(2, |m| m + 1);
                    for j in 0..n {
                        let text = element_text(p, *elem, k + u64::from(j)).unwrap_or_else(|| text.clone());
                        let _ = writeln!(s, "t={k} in {} {text}", c.name);
                    }
                }
                _ => {
                    let _ = writeln!(s, "t={k} in {} {text}", c.name);
                }
            }
        }
    }
    Case { label: EDGE.into(), stimulus: s, ticks: end + PHASE }
}

/// Eine Lieferung an einen skalaren Input im Abschnitt `phase`.
fn scalar_line(s: &mut String, p: &Program, c: &Channel, k: u64, phase: &str) {
    let mid = Pick::Mid(k);
    let line = match phase {
        "unten" => scalar_text(p, c.ty, Pick::Lo),
        "oben" => scalar_text(p, c.ty, Pick::Hi),
        "unter" => scalar_text(p, c.ty, Pick::BelowLo).or_else(|| scalar_text(p, c.ty, mid)),
        "ueber" => scalar_text(p, c.ty, Pick::AboveHi).or_else(|| scalar_text(p, c.ty, mid)),
        // Wechselnd schlecht mit Grund (ohne Wert: `bad` mit Wert bricht den
        // Treibervertrag, 12.6 Zeile 2), veraltet mit Alter, verdaechtig.
        "qualitaet" => scalar_text(p, c.ty, mid).map(|text| match k % 3 {
            0 => "bad reason=Implausible".to_string(),
            1 => {
                let age = c.attrs.max_age.map_or(10_000_000, |a| a.saturating_add(p.config.tick));
                format!("{text} stale age={}", takt_mir::dump::duration(age))
            }
            _ => format!("{text} suspect"),
        }),
        // Keine Lieferung: Der Input veraltet ueber `max_age` (3.5).
        "luecke" => None,
        // Abwechselnd die Grenzen: Spruenge ueber `max_slew` (3.5).
        "spruenge" if c.attrs.max_slew.is_some() => scalar_text(p, c.ty, if k % 2 == 0 { Pick::Lo } else { Pick::Hi }),
        _ => scalar_text(p, c.ty, mid),
    };
    if let Some(text) = line {
        let _ = writeln!(s, "t={k} in {} {text}", c.name);
    }
}

/// Lieferungen an einen Eingabestrom im Abschnitt `phase`; `first` ist der
/// erste Tick des Abschnitts.
fn stream_line(s: &mut String, p: &Program, c: &Channel, elem: TypeId, k: u64, phase: &str, first: bool) {
    let Some(text) = element_text(p, elem, k) else { return };
    match phase {
        // Ein Schwall bis `MAXPT`, ohne Hoechstrate bis ueber die Kapazitaet.
        "oben" if first => {
            let n = takt_hal::edge::maxpt_of(c, p.config.tick)
                .unwrap_or_else(|| c.attrs.capacity.unwrap_or(16).saturating_add(1));
            for j in 0..n.min(64) {
                let text = element_text(p, elem, k + u64::from(j)).unwrap_or_else(|| text.clone());
                let _ = writeln!(s, "t={k} in {} {text}", c.name);
            }
        }
        // Ein Element, das der Rand verwirft (8.6), wo das Element eine
        // Byteform hat; eine Zeile ueber ihrer Kapazitaet, die er kuerzt.
        "ueber" if first => {
            let bad = match p.types.get(elem) {
                Type::Record(_) | Type::Capture { .. } => Some("0x".to_string()),
                Type::Line { cap } => Some(format!("{:?}", "x".repeat(*cap as usize + 5))),
                _ => None,
            };
            let _ = writeln!(s, "t={k} in {} {}", c.name, bad.unwrap_or(text));
        }
        "luecke" | "qualitaet" => {}
        _ => {
            let _ = writeln!(s, "t={k} in {} {text}", c.name);
        }
    }
}

/// Je Command eines in jedem Abschnitt, versetzt nach seiner Nummer; alle
/// zusammen im dritten Tick des zweiten.
fn commands(s: &mut String, p: &Program, k: u64) {
    for (i, c) in p.commands.iter().enumerate() {
        let at = (i as u64 % PHASE).saturating_add(1);
        if k % PHASE == at || k == PHASE + 2 {
            let _ = writeln!(s, "t={k} cmd {}", c.name);
        }
    }
}

/// Tunables auf ihren Grenzen und einmal jenseits; die Zeile jenseits
/// verwirft der Lauf (8.4).
fn tunes(s: &mut String, p: &Program, k: u64, phase: &str) {
    let how = match phase {
        "unten" => Pick::Lo,
        "oben" => Pick::Hi,
        "ueber" => Pick::AboveHi,
        "zurueck" => Pick::Mid(k),
        _ => return,
    };
    for t in p.params.iter().filter(|t| t.tunable) {
        if let Some(text) = scalar_text(p, t.ty, how) {
            let _ = writeln!(s, "t={k} tune {} {text}", t.name);
        }
    }
}

/// Ein Wert als Text des Stimulus.
fn scalar_text(p: &Program, ty: TypeId, how: Pick) -> Option<String> {
    pick(p, ty, how).map(|v| value_text(&v, ty, p))
}

/// Ein Stromelement als Text: Records, Captures und Bytes in ihrer
/// Drahtform, Text in Anfuehrungszeichen, alles andere in seiner
/// Schreibweise.
fn element_text(p: &Program, elem: TypeId, k: u64) -> Option<String> {
    let v = pick(p, elem, Pick::Mid(k))?;
    Some(match p.types.get(elem) {
        Type::Record(_) | Type::Capture { .. } | Type::Bytes { .. } => {
            let bytes = takt_interp::bytes::encode(p, &v, elem).ok()?;
            format!("0x{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
        }
        _ => value_text(&v, elem, p),
    })
}

/// Ein Wert des Typs `ty` nach `how`; `None`, wo der Typ keinen solchen hat
/// — keine Grenze jenseits ohne Range, keinen Wert eines Typs, den der
/// Stimulus nicht schreibt.
fn pick(p: &Program, ty: TypeId, how: Pick) -> Option<Value> {
    Some(match p.types.get(ty) {
        Type::Bool => Value::Bool(match how {
            Pick::Mid(k) => k % 2 == 0,
            Pick::Lo | Pick::BelowLo => false,
            Pick::Hi | Pick::AboveHi => true,
        }),
        Type::Int { width, range, .. } => {
            let (wlo, whi) = takt_interp::arith::bounds(*width);
            let bounds = range.as_ref().and_then(|r| Some((int_of(&r.lo)?, int_of(&r.hi)?)));
            let (lo, hi) = bounds.unwrap_or((wlo.max(-1000), whi.min(1000)));
            let x = match how {
                Pick::Mid(k) => lo + (hi - lo) * i128::from(k % 5 + 1) / 6,
                Pick::Lo => lo,
                Pick::Hi => hi,
                Pick::BelowLo => bounds.map(|_| lo - 1)?,
                Pick::AboveHi => bounds.map(|_| hi + 1)?,
            };
            if !(wlo..=whi).contains(&x) {
                return None;
            }
            Value::int(*width, x)
        }
        Type::Float { width, range, .. } => {
            let bounds = range.as_ref().and_then(|r| Some((float_of(&r.lo)?, float_of(&r.hi)?)));
            let (lo, hi) = bounds.unwrap_or((-100.0, 100.0));
            let step = ((hi - lo).abs() * 0.01).max(1e-3);
            let x = match how {
                Pick::Mid(k) => lo + (hi - lo) * (k % 5 + 1) as f64 / 6.0,
                Pick::Lo => lo,
                Pick::Hi => hi,
                Pick::BelowLo => bounds.map(|_| lo - step)?,
                Pick::AboveHi => bounds.map(|_| hi + step)?,
            };
            match width {
                FloatWidth::F32 => Value::F32(x as f32),
                FloatWidth::F64 => Value::F64(x),
            }
        }
        Type::Duration { range } => {
            let bounds = range.as_ref().and_then(|r| Some((int_of(&r.lo)?, int_of(&r.hi)?)));
            let (lo, hi) = bounds.unwrap_or((0, 1_000_000_000));
            let x = match how {
                Pick::Mid(k) => lo + (hi - lo) * i128::from(k % 5 + 1) / 6,
                Pick::Lo => lo,
                Pick::Hi => hi,
                Pick::BelowLo => bounds.map(|_| lo - 1)?,
                Pick::AboveHi => bounds.map(|_| hi + 1)?,
            };
            Value::Duration(i64::try_from(x).ok()?)
        }
        Type::Enum(id) => {
            if matches!(how, Pick::BelowLo | Pick::AboveHi) {
                return None;
            }
            let e = &p.enums[id.index()];
            let n = e.variants.len().max(1);
            let variant = match how {
                Pick::Mid(k) => (k % n as u64) as usize,
                Pick::Lo | Pick::BelowLo => 0,
                Pick::Hi | Pick::AboveHi => n - 1,
            };
            let def = e.variants.get(variant)?;
            let fields = def.fields.iter().map(|f| pick(p, f.ty, how)).collect::<Option<Vec<_>>>()?;
            Value::Enum { variant: variant as u32, fields }
        }
        Type::Record(id) => {
            // Jenseits der Grenzen: das erste Feld, das eine hat; die
            // anderen auf der Grenze.
            let fields = &p.records[id.index()].fields;
            let beyond = fields.iter().position(|f| pick(p, f.ty, how).is_some()).filter(|_| how != how.inside());
            if how != how.inside() && beyond.is_none() {
                return None;
            }
            let mut out = Vec::with_capacity(fields.len());
            for (i, f) in fields.iter().enumerate() {
                let here = if Some(i) == beyond { how } else { how.inside() };
                out.push(pick(p, f.ty, here).or_else(|| Some(Value::default_for(f.ty, p)))?);
            }
            Value::Record(out)
        }
        Type::Array { elem, len } => Value::Array(sequence(p, *elem, *len, how)?),
        Type::Samples { elem, len } => Value::Samples(sequence(p, *elem, *len, how)?),
        Type::Capture { elem, len } => {
            if how != how.inside() {
                return None;
            }
            let k = match how {
                Pick::Mid(k) => k,
                _ => 0,
            };
            Value::Record(vec![
                Value::Duration(0),
                Value::Int(i64::from(k % 3 == 0) + 1),
                Value::Int(2),
                Value::F64(1000.0),
                Value::Array(sequence(p, *elem, *len, how)?),
            ])
        }
        Type::Str { cap } => Value::Str(text_of(how, *cap)),
        Type::Line { cap } => Value::Line { text: text_of(how, *cap), truncated: false },
        Type::Bytes { cap } => {
            if how != how.inside() {
                return None;
            }
            let n = match how {
                Pick::Lo => 0,
                Pick::Hi => *cap as usize,
                _ => (*cap as usize).min(4),
            };
            Value::Bytes((0..n).map(|i| (i as u8).wrapping_mul(37)).collect())
        }
        _ => return None,
    })
}

/// `len` Werte des Elementtyps: das erste nach `how`, die anderen im
/// Innern.
fn sequence(p: &Program, elem: TypeId, len: u32, how: Pick) -> Option<Vec<Value>> {
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len {
        let here = match how {
            _ if i == 0 => how,
            Pick::Mid(k) => Pick::Mid(k + u64::from(i)),
            other => other.inside(),
        };
        out.push(pick(p, elem, here)?);
    }
    Some(out)
}

/// Ein Text fuer `str<cap>` und `line<cap>`: leer, voll, sonst kurz.
fn text_of(how: Pick, cap: u32) -> String {
    match how {
        Pick::Lo | Pick::BelowLo => String::new(),
        Pick::Hi | Pick::AboveHi => "x".repeat(cap as usize),
        Pick::Mid(k) => format!("w{k}").chars().take(cap as usize).collect(),
    }
}

fn int_of(c: &Const) -> Option<i128> {
    match c {
        Const::Int(i) | Const::Duration(i) => Some(i128::from(*i)),
        _ => None,
    }
}

fn float_of(c: &Const) -> Option<f64> {
    match c {
        Const::Float(f) => Some(*f),
        Const::Int(i) => Some(*i as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(src: &str) -> Program {
        let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
        let out = takt_sema::compile(src, &options);
        assert!(!out.has_errors(), "{:?}", out.diagnostics);
        out.program.expect("Programm")
    }

    const SRC: &str = "system:
    language = 1
    tick     = 10 ms

command go

input  p : float[bar] in 0..250 bar @ hw(\"i/p\") with max_age = 30 ms, max_slew = 1000 bar/s
input  n : int in -5..5 @ hw(\"i/n\")
output o : float[bar] @ hw(\"o/o\") with safe = 0 bar

machine m:
    initial RUN

    state RUN:
        loop:
            o = p
";

    /// Jeder erzeugte Lauf liest sich als Stimulus des Programms, und
    /// `erzeugt` traegt beide Grenzen, ein Jenseits, eine Qualitaet und das
    /// Command.
    #[test]
    fn the_generated_runs_are_stimuli_of_the_program() {
        let p = program(SRC);
        let runs = cases(&p);
        let labels: Vec<&str> = runs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["erzeugt", "erzeugt abbruch", EDGE]);
        for c in &runs {
            let parsed = takt_interp::read_stimulus(&p, &c.stimulus, None);
            assert!(parsed.is_ok(), "{}: {:?}\n{}", c.label, parsed.err(), c.stimulus);
        }
        let sweep = &runs[0].stimulus;
        for line in
            [" in p 0.0 bar\n", " in p 250.0 bar\n", " in p 252.5 bar\n", " in n -6\n", " bad reason=", " cmd go\n"]
        {
            assert!(sweep.contains(line), "`{line}` fehlt:\n{sweep}");
        }
        assert!(runs[1].stimulus.contains(" abort\n"), "{}", runs[1].stimulus);
    }

    /// Ohne Eingaben, Commands und Tunables gibt es nichts zu erzeugen.
    #[test]
    fn a_program_without_inputs_gets_no_generated_run() {
        let p = program(
            "system:
    language = 1
    tick     = 10 ms

output o : int in 0..9 @ hw(\"o/o\") with safe = 0

machine m:
    initial RUN

    state RUN:
        loop:
            o = 1
",
        );
        assert!(cases(&p).is_empty());
    }
}
