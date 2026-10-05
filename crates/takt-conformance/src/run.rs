//! Der Vergleich: Interpreter gegen erzeugten Code (13.8).
//!
//! **Was verglichen wird.** Die Outputs je Tick, in kanonischer Ordnung.
//! 9.4.4 spricht von Outputs, und plan/m4.md 2.5 hat daraus die Abnahme
//! gemacht: Der Trace ist die Zusage, der Zustand ein Diagnosewerkzeug.
//! Dazu die Faults je Tick nach Maschine und Art (FB-330): Zwei Wege, die
//! im selben Tick aus verschiedenen Gruenden faulten, fielen an den
//! Outputs nicht auf — beide stehen danach auf `safe`. Ebenso die Alerts,
//! die Zaehler der Stroeme (8.6, FB-361) und die Beobachtungen `log`,
//! `measure`, `verify`, `verdict`, `property` und `end` (13.5, 13.3, 12.7).
//!
//! **Was nicht verglichen wird, und warum das dasteht.** Der Testrahmen
//! ist keine Runtime (siehe `harness`): Er hat keine Treiber, also keine
//! Eingaben; keine Uhr, also keine Zeitquelle; keine Fault-Behandlung,
//! also keine Abort-Phase. Ein Lauf ohne Eingaben prueft damit den
//! *Anfangszustand und seine Fortschreibung* — das ist weniger, als die
//! Abnahme am Ende verlangt, und es steht hier, damit niemand die Zahl
//! fuer mehr haelt, als sie ist.
//!
//! Was er schon prueft, ist nicht wenig: Jede Zuweisung, jeder `check`,
//! jeder Uebergang und jede Kennlinie eines Programms ohne Eingaben laeuft
//! durch beide Implementierungen, und ihre Outputs muessen Zeichen fuer
//! Zeichen gleich sein.

use std::collections::BTreeMap;

/// Eine Abweichung zwischen den beiden Implementierungen.
#[derive(Clone, Debug, PartialEq)]
pub struct Difference {
    /// Der Tick, in dem sie auftritt.
    pub tick: u64,
    /// Name des Outputs.
    pub output: String,
    /// Was der Interpreter sagt.
    pub interpreter: String,
    /// Was der erzeugte Code sagt.
    pub native: String,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "t={} {}: Interpreter `{}`, nativ `{}`", self.tick, self.output, self.interpreter, self.native)
    }
}

/// Die Zeilen `t=<tick> out <name> <wert>`, je Tick und Output in der
/// Reihenfolge des Traces.
fn outputs(text: &str) -> BTreeMap<(u64, String), Vec<String>> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let Some(t) = w.next().and_then(|s| s.strip_prefix("t=")).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        if w.next() != Some("out") {
            continue;
        }
        let Some(name) = w.next() else { continue };
        let value: Vec<&str> = w.collect();
        out.entry((t, name.to_string())).or_insert_with(Vec::new).push(value.join(" "));
    }
    out
}

/// Die Outputs eines Programms, deren Werte `f32` tragen (4.2): ein `f32`,
/// ein Feld, Vektor oder Record mit `f32`-Elementen, eine Matrix in einem
/// Programm mit `float = f32`.
pub fn f32_outputs(p: &takt_mir::Program) -> std::collections::BTreeSet<String> {
    use takt_mir::types::{FloatWidth, Type};
    fn holds(p: &takt_mir::Program, ty: takt_mir::TypeId, depth: u32) -> bool {
        if depth > 8 {
            return false;
        }
        match p.types.get(ty) {
            Type::Float { width, .. } => *width == FloatWidth::F32,
            Type::Mat { .. } => p.config.float_width == FloatWidth::F32,
            Type::Array { elem, .. } | Type::Vec { elem, .. } | Type::Samples { elem, .. } => {
                holds(p, *elem, depth + 1)
            }
            Type::Record(r) => p.records[r.index()].fields.iter().any(|f| holds(p, f.ty, depth + 1)),
            _ => false,
        }
    }
    p.channels
        .iter()
        .filter(|c| c.dir == takt_mir::program::Direction::Output && holds(p, c.ty, 0))
        .map(|c| c.name.clone())
        .collect()
}

/// Schreibt die `f32`-Werte eines Interpreter-Traces so, wie der erzeugte
/// Code sie schreibt: als ihren Wert in `f64`.
///
/// Der Interpreter schreibt ein `f32` in seiner kuerzesten Form, der
/// Wirtsrahmen nach `(double)` mit 17 Stellen und die Boards in der
/// kuerzesten Form des `f64`; [`compare`] liest alle als `f64` und
/// vergliche sonst Schreibweisen statt Bits. Nur die genannten Outputs: Fuer
/// einen `f64`-Output waere dieselbe Lesart eine Lockerung.
pub fn widen_f32(trace: &str, outputs: &std::collections::BTreeSet<String>) -> String {
    let widen = |value: &str| -> String {
        let mut out = String::with_capacity(value.len());
        let mut token = String::new();
        let flush = |token: &mut String, out: &mut String| {
            match token.parse::<f32>() {
                Ok(x) if token.chars().any(|c| c.is_ascii_digit()) => out.push_str(&format!("{}", f64::from(x))),
                _ => out.push_str(token),
            }
            token.clear();
        };
        for c in value.chars() {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+') {
                token.push(c);
            } else {
                flush(&mut token, &mut out);
                out.push(c);
            }
        }
        flush(&mut token, &mut out);
        out
    };
    let mut out = String::with_capacity(trace.len());
    for line in trace.lines() {
        let mut parts = line.splitn(4, ' ');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(t), Some("out"), Some(name), Some(value)) if t.starts_with("t=") && outputs.contains(name) => {
                out.push_str(&format!("{t} out {name} {}", widen(value)));
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Vergleicht zwei Traces.
///
/// Beide Seiten duerfen eine Zeile nur bei Aenderung schreiben (9.3); der
/// letzte Wert gilt fort. Verglichen wird an jedem Tick, an dem eine Seite
/// schreibt, jeder Output, den beide schon gemeldet haben. Schreiben beide
/// einen Output mehrmals im selben Tick — den Wert vor und nach dem Ende
/// eines Laufs (12.7) —, zaehlt die Folge ihrer Aenderungen und nicht nur
/// ihr letzter Wert; ein Wert gleich dem geltenden ist keine Aenderung,
/// weil der Wirtsrahmen jeden Output je Abzug schreibt.
///
/// **Ein Vergleich besteht nicht leer** (FB-391). Zeigt nur eine Seite
/// Beobachtbares — ein Lauf, der nicht startete, ein Interpreter, der
/// abbrach —, ist das eine Abweichung, ebenso ein Ausgang, den nur eine
/// Seite schreibt.
pub fn compare(interpreter: &str, native: &str) -> Vec<Difference> {
    if observed(interpreter) != observed(native) {
        return vec![Difference {
            tick: 0,
            output: "trace".into(),
            interpreter: summary(interpreter),
            native: summary(native),
        }];
    }
    let a = outputs(interpreter);
    let b = outputs(native);
    let mut out = one_sided(&a, &b);
    let mut ticks: Vec<u64> = a.keys().chain(b.keys()).map(|(t, _)| *t).collect();
    ticks.sort_unstable();
    ticks.dedup();
    let (mut want, mut have) = (BTreeMap::new(), BTreeMap::new());
    for tick in ticks {
        let now_a: BTreeMap<&str, Vec<&str>> = at(&a, tick).collect();
        let now_b: BTreeMap<&str, Vec<&str>> = at(&b, tick).collect();
        let mut reported = Vec::new();
        for (name, ws) in &now_a {
            let Some(hs) = now_b.get(name) else { continue };
            let (ws, hs) = (changes(ws, want.get(name)), changes(hs, have.get(name)));
            if (ws.len() > 1 || hs.len() > 1)
                && (ws.len() != hs.len() || ws.iter().zip(&hs).any(|(w, h)| !same_number(w, h)))
            {
                out.push(Difference {
                    tick,
                    output: name.to_string(),
                    interpreter: ws.join(" | "),
                    native: hs.join(" | "),
                });
                reported.push(*name);
            }
        }
        want.extend(now_a.iter().filter_map(|(n, v)| Some((*n, *v.last()?))));
        have.extend(now_b.iter().filter_map(|(n, v)| Some((*n, *v.last()?))));
        for (name, w) in &want {
            let Some(h) = have.get(name) else { continue };
            if !reported.contains(name) && !same_number(w, h) {
                out.push(Difference {
                    tick,
                    output: name.to_string(),
                    interpreter: w.to_string(),
                    native: h.to_string(),
                });
            }
        }
    }
    // Faults und Beobachtungen nur, soweit beide Traces reichen: Ein Lauf,
    // der frueher endet, hat die spaeteren nicht verpasst, sondern nicht
    // erreicht.
    let horizon = last_tick(interpreter).min(last_tick(native));
    let (fa, fb) = (faults(interpreter), faults(native));
    let fault_ticks: std::collections::BTreeSet<u64> =
        fa.keys().chain(fb.keys()).copied().filter(|t| Some(*t) <= horizon).collect();
    for tick in fault_ticks {
        let (x, y) = (fa.get(&tick), fb.get(&tick));
        if x != y {
            let render = |v: Option<&Vec<(String, String)>>| {
                v.map_or(String::new(), |v| v.iter().map(|(m, k)| format!("{m} {k}")).collect::<Vec<_>>().join(", "))
            };
            out.push(Difference { tick, output: "fault".into(), interpreter: render(x), native: render(y) });
        }
    }
    let (aa, ab) = (alerts(interpreter), alerts(native));
    let alert_ticks: std::collections::BTreeSet<u64> =
        aa.keys().chain(ab.keys()).copied().filter(|t| Some(*t) <= horizon).collect();
    for tick in alert_ticks {
        let (x, y) = (aa.get(&tick), ab.get(&tick));
        if x != y {
            let render = |v: Option<&Vec<String>>| v.map_or(String::new(), |v| v.join(", "));
            out.push(Difference { tick, output: "alert".into(), interpreter: render(x), native: render(y) });
        }
    }
    for (output, a, b) in [
        ("stream", stream_counters(interpreter), stream_counters(native)),
        ("observation", observations(interpreter), observations(native)),
    ] {
        let ticks: std::collections::BTreeSet<u64> =
            a.keys().chain(b.keys()).copied().filter(|t| Some(*t) <= horizon).collect();
        for tick in ticks {
            let (x, y) = (a.get(&tick), b.get(&tick));
            if x != y {
                let render = |v: Option<&Vec<String>>| v.map_or(String::new(), |v| v.join(", "));
                out.push(Difference { tick, output: output.into(), interpreter: render(x), native: render(y) });
            }
        }
    }
    out
}

/// Zeigt ein Trace etwas, das [`compare`] vergleicht: Ausgaenge, Faults,
/// Alerts, Zaehler der Stroeme, Beobachtungen?
fn observed(trace: &str) -> bool {
    trace.lines().any(|l| {
        let mut w = l.split_whitespace();
        w.next().is_some_and(|t| t.strip_prefix("t=").is_some_and(|k| k.parse::<u64>().is_ok()))
            && matches!(
                w.next(),
                Some(
                    "out"
                        | "fault"
                        | "alert"
                        | "stream"
                        | "log"
                        | "measure"
                        | "verify"
                        | "verdict"
                        | "property"
                        | "assumption"
                        | "end"
                )
            )
    })
}

/// Die Beobachtungszeilen eines Traces (5.6, 12.7, 13.3, 13.5): je Tick
/// Art und Ausgang, sortiert.
///
/// Der Interpreter nennt Maschine und Text, der erzeugte Code Nummer der
/// Maschine und Stelle (`log <m> <stelle>`); gemeinsam ist beiden, was im
/// Tick geschah: wie viele `log`, welche `measure`-Werte, welche `verify`
/// und `verdict` mit welchem Ausgang, welche Verletzung an welcher Position
/// (`property`, `assumption`) und welches Ende eines Laufs (`end`).
///
/// Nicht verglichen werden `end scenario` (das Ende waehlt der Interpreter,
/// der Rahmen laeuft die Ticks zu Ende), `abort` (der Interpreter schreibt
/// die Anweisung nicht, ihre Wirkung zeigen die Faults), `persist` (die
/// Boards schreiben die Zeile nicht, `persist_native.rs` haelt sie
/// gegeneinander), `job`, `signal` und `verdict-final` (der erzeugte Code
/// schreibt sie nicht).
fn observations(trace: &str) -> BTreeMap<u64, Vec<String>> {
    let mut out: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for line in trace.lines() {
        let Some(rest) = line.strip_prefix("t=") else { continue };
        let Some((tick, rest)) = rest.split_once(' ') else { continue };
        let Ok(tick) = tick.parse::<u64>() else { continue };
        let words: Vec<&str> = rest.split_whitespace().collect();
        // Eine Maschine nennt der erzeugte Code mit ihrer Nummer, der
        // Interpreter mit ihrem Namen, und der beginnt nie mit einer Ziffer.
        let native = words.get(1).is_some_and(|m| m.parse::<i64>().is_ok());
        let key = match words.as_slice() {
            ["log", ..] => "log".to_string(),
            ["verify", _, _, ok] if native => format!("verify {}", if *ok == "1" { "ok" } else { "fail" }),
            ["verify", _, ok, ..] => format!("verify {ok}"),
            ["verdict", _, _, pass] if native => format!("verdict {}", if *pass == "1" { "pass" } else { "fail" }),
            ["verdict", _, pass, ..] => format!("verdict {pass}"),
            ["measure", _, _, "bits", bits] if native => {
                format!(
                    "measure {}",
                    bits.parse::<i64>().map_or_else(|_| bits.to_string(), |b| measured(f64::from_bits(b as u64)))
                )
            }
            ["measure", _, _, value, unit] if !native => match nanoseconds(value, unit) {
                Some(ns) => format!("measure {}", measured(ns as f64)),
                None => format!("measure {}", value.parse::<f64>().map_or_else(|_| value.to_string(), measured)),
            },
            ["measure", _, _, value, ..] => {
                format!("measure {}", value.parse::<f64>().map_or_else(|_| value.to_string(), measured))
            }
            ["property", _, at] if native => format!("property {at}"),
            ["property" | "assumption", _, "violated", at] => format!("property {at}"),
            ["end", "scenario"] => continue,
            ["end", word] => format!("end {word}"),
            _ => continue,
        };
        out.entry(tick).or_default().push(key);
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// Eine Dauer, wie der Interpreter sie schreibt (`82 ms`, T2: in ihrer
/// groessten ganzzahligen Einheit), in Nanosekunden — der Groesse, die der
/// erzeugte Code als `double` meldet. Ganzzahlig gerechnet, damit kein
/// zweiter Rundungsweg vor dem bitweisen Vergleich steht; `None` fuer eine
/// andere Einheit, deren Zahl beide Seiten gleich schreiben.
fn nanoseconds(value: &str, unit: &str) -> Option<i64> {
    let factor: i64 = match unit {
        "ns" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "min" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        _ => return None,
    };
    value.parse::<i64>().ok()?.checked_mul(factor)
}

/// Ein gemessener Wert als sein Bitmuster: Der Interpreter schreibt ihn in
/// seiner kuerzesten Form mit Einheit, der Wirtsrahmen mit `%.17g`, die
/// Boards als Bits (Satz 9.4.4 verlangt dasselbe Bit).
fn measured(v: f64) -> String {
    format!("{:#018x}", v.to_bits())
}

/// Die erste Zeile eines Traces, fuer die Meldung einer leeren Seite.
fn summary(trace: &str) -> String {
    trace.lines().find(|l| !l.trim().is_empty()).map_or_else(|| "leer".to_string(), |l| l.chars().take(120).collect())
}

/// Die Ausgaenge, die nur eine Seite schreibt, je mit ihrem ersten Wert.
fn one_sided(a: &BTreeMap<(u64, String), Vec<String>>, b: &BTreeMap<(u64, String), Vec<String>>) -> Vec<Difference> {
    let first = |m: &BTreeMap<(u64, String), Vec<String>>| {
        let mut f: BTreeMap<String, (u64, String)> = BTreeMap::new();
        for ((tick, name), values) in m {
            f.entry(name.clone()).or_insert_with(|| (*tick, values.first().cloned().unwrap_or_default()));
        }
        f
    };
    let (fa, fb) = (first(a), first(b));
    let only = |x: &BTreeMap<String, (u64, String)>, y: &BTreeMap<String, (u64, String)>| {
        x.iter().filter(|(n, _)| !y.contains_key(*n)).map(|(n, v)| (n.clone(), v.clone())).collect::<Vec<_>>()
    };
    let mut out: Vec<Difference> = only(&fa, &fb)
        .into_iter()
        .map(|(output, (tick, value))| Difference { tick, output, interpreter: value, native: "fehlt".into() })
        .collect();
    out.extend(only(&fb, &fa).into_iter().map(|(output, (tick, value))| Difference {
        tick,
        output,
        interpreter: "fehlt".into(),
        native: value,
    }));
    out
}

/// Die Zaehler der Stroeme (`t=<tick> stream <name> dropped=… overflowed=…
/// malformed=…`, 8.6): je Tick die Zeilen, sortiert. Beide Seiten schreiben
/// sie nur, wenn sich ein Zaehler nach Tick 0 aendert; eine Zeile, die nur
/// eine Seite schreibt, ist eine Abweichung.
fn stream_counters(trace: &str) -> BTreeMap<u64, Vec<String>> {
    let mut out: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for line in trace.lines() {
        let Some(rest) = line.strip_prefix("t=") else { continue };
        let Some((tick, rest)) = rest.split_once(' ') else { continue };
        let (Ok(tick), Some(rest)) = (tick.parse::<u64>(), rest.strip_prefix("stream ")) else { continue };
        out.entry(tick).or_default().push(rest.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// Die Alert-Flanken eines Traces (`t=<tick> alert <maschine> on|off …`,
/// 5.6): je Tick Maschine, Flanke und ob ein ungueltiger Wert sie
/// ausloeste, sortiert. Den Text rendert nur der Interpreter; der
/// erzeugte Code kennt die Stelle, nicht die Meldung.
fn alerts(trace: &str) -> BTreeMap<u64, Vec<String>> {
    let mut out: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for line in trace.lines() {
        let Some(rest) = line.strip_prefix("t=") else { continue };
        let Some((tick, rest)) = rest.split_once(' ') else { continue };
        let Some(rest) = rest.strip_prefix("alert ") else { continue };
        let Ok(tick) = tick.parse::<u64>() else { continue };
        let mut words = rest.split(' ');
        let (Some(machine), Some(edge)) = (words.next(), words.next()) else { continue };
        let invalid = rest.ends_with(" invalid") || rest.ends_with("(sensor invalid)\"");
        let key = if invalid { format!("{machine} {edge} invalid") } else { format!("{machine} {edge}") };
        out.entry(tick).or_default().push(key);
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

/// Die Faults eines Traces (`t=<tick> fault <maschine> <art> …`): je Tick
/// Maschine und Art, sortiert — die Reihenfolge der Maschinen in einem
/// Tick ist keine Zusage (Satz 9.4.1).
fn faults(text: &str) -> BTreeMap<u64, Vec<(String, String)>> {
    let mut out: BTreeMap<u64, Vec<(String, String)>> = BTreeMap::new();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let Some(t) = w.next().and_then(|s| s.strip_prefix("t=")).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        if w.next() != Some("fault") {
            continue;
        }
        let (Some(machine), Some(kind)) = (w.next(), w.next()) else { continue };
        out.entry(t).or_default().push((machine.to_string(), kind.to_string()));
    }
    for list in out.values_mut() {
        list.sort();
    }
    out
}

/// Der letzte Tick, den ein Trace nennt.
fn last_tick(text: &str) -> Option<u64> {
    text.lines().filter_map(|l| l.split_whitespace().next()?.strip_prefix("t=")?.parse::<u64>().ok()).max()
}

/// Die Outputs eines Ticks, jeder mit seinen Werten in Reihenfolge.
fn at(m: &BTreeMap<(u64, String), Vec<String>>, tick: u64) -> impl Iterator<Item = (&str, Vec<&str>)> {
    m.range((tick, String::new())..(tick.saturating_add(1), String::new()))
        .map(|((_, n), v)| (n.as_str(), v.iter().map(String::as_str).collect()))
}

/// Die Aenderungen eines Ticks gegenueber dem Wert `current`, der bis
/// dahin galt (9.3): ein Wert, der ihn oder seinen Vorgaenger nur
/// wiederholt, zaehlt nicht.
fn changes<'a>(values: &[&'a str], current: Option<&&str>) -> Vec<&'a str> {
    let mut last = current.copied();
    let mut out = Vec::new();
    for v in values {
        if last.is_none_or(|l| !same_number(l, v)) {
            out.push(*v);
        }
        last = Some(v);
    }
    out
}

/// Sind zwei Ausgaben derselbe Wert?
///
/// Der Interpreter schreibt `true`/`false` und Zahlen mit Einheit, der
/// Rahmen Zahlen. Verglichen wird darum der Zahlenwert — und bei
/// Fliesskomma **bitgenau**, nicht mit Toleranz: Satz 9.4.4 verlangt
/// dasselbe Bit, und eine Toleranz waere genau die Abweichung, die zu
/// finden der Test da ist.
fn same_number(interpreter: &str, native: &str) -> bool {
    // Arrays elementweise (T2): der Interpreter schreibt `[3.75 V, …]`,
    // der Rahmen `[3.75, …]`.
    if let (Some(x), Some(y)) = (interpreter.strip_prefix('['), native.strip_prefix('[')) {
        let items = |s: &str| s.trim_end_matches(']').split(',').map(str::trim).map(String::from).collect::<Vec<_>>();
        let (xs, ys) = (items(x), items(y));
        return xs.len() == ys.len() && xs.iter().zip(&ys).all(|(a, b)| same_number(a, b));
    }
    // Varianten mit Feldern: `RECT(3.0, 1.5)` gegen `RECT(3, 1.5)`.
    if let (Some((na, xa)), Some((nb, xb))) = (interpreter.split_once('('), native.split_once('('))
        && na == nb
        && xa.ends_with(')')
        && xb.ends_with(')')
    {
        let items = |s: &str| s.trim_end_matches(')').split(',').map(str::trim).map(String::from).collect::<Vec<_>>();
        let (xs, ys) = (items(xa), items(xb));
        return xs.len() == ys.len() && xs.iter().zip(&ys).all(|(a, b)| same_number(a, b));
    }
    let mut wa = interpreter.split_whitespace();
    let mut wb = native.split_whitespace();
    let (a, b) = (wa.next().unwrap_or(interpreter), wb.next().unwrap_or(native));
    // Der Interpreter schreibt Groessen mit Einheit, der Rahmen ohne; eine
    // Dauer schreiben beide mit (T2), und dann zaehlt auch sie.
    if let (Some(ua), Some(ub)) = (wa.next(), wb.next())
        && ua != ub
    {
        return false;
    }
    if a == b {
        return true;
    }
    // `-0` ist eine Fliesskommazahl — eine Ganzzahl schreibt so niemand —
    // und gegen `0` ein anderes Bit (4.2); als Ganzzahl gelesen waeren
    // beide gleich.
    let float = |s: &str| s == "-0" || s.contains(['.', 'e', 'E', 'n', 'N', 'i', 'I']);
    if (float(a) || float(b))
        && let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>())
    {
        return x.to_bits() == y.to_bits();
    }
    // `true`/`false` gegen 1/0.
    let as_bool = |s: &str| match s {
        "true" => Some(1i64),
        "false" => Some(0),
        _ => s.parse::<i64>().ok(),
    };
    if let (Some(x), Some(y)) = (as_bool(a), as_bool(b)) {
        return x == y;
    }
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein `f32` des Interpreters gleicht dem Wert, den der Rahmen nach
    /// `(double)` schreibt; ein `f64`-Output bleibt streng.
    #[test]
    fn f32_outputs_compare_by_value() {
        let names: std::collections::BTreeSet<String> = ["s".to_string(), "v".to_string()].into();
        let interp = "t=0 out s 0.9997293\nt=0 out v [0.1, 2.5 V]\nt=0 out d 0.1\n";
        let native =
            "t=0 out s 0.99972927570343018\nt=0 out v [0.10000000149011612, 2.5]\nt=0 out d 0.10000000149011612\n";
        let d = compare(&widen_f32(interp, &names), native);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].output, "d");
    }

    /// **Ein Vergleich besteht nicht leer** (FB-391): Eine leere oder
    /// abgebrochene Seite und ein Ausgang, den nur eine Seite schreibt, sind
    /// Abweichungen; zwei Seiten ohne Beobachtbares gleichen sich.
    #[test]
    fn an_empty_or_one_sided_trace_is_a_difference() {
        let want = "t=0 out a 1\nt=0 out b 2\n";
        assert_eq!(compare(want, "")[0].output, "trace");
        assert_eq!(compare("Trap: Bug\n", want)[0].output, "trace");
        assert_eq!(compare(want, "out a 1\nout b 2\n")[0].output, "trace", "eine Zeile ohne `t=` kommt nicht an");
        assert!(compare("t=0 state m RUN\n", "").is_empty());
        let d = compare(want, "t=0 out a 1\n");
        assert_eq!((d.len(), d[0].output.as_str(), d[0].native.as_str()), (1, "b", "fehlt"), "{d:?}");
        let d = compare("t=0 out a 1\n", want);
        assert_eq!((d.len(), d[0].output.as_str(), d[0].interpreter.as_str()), (1, "b", "fehlt"), "{d:?}");
    }

    /// Das Vorzeichen der Null zaehlt (4.2): `-0` gegen `0` ist ein anderes
    /// Bit, auch wenn `widen_f32` ein `f32` ohne Dezimalpunkt schreibt.
    #[test]
    fn the_sign_of_zero_counts() {
        let names: std::collections::BTreeSet<String> = ["s".to_string()].into();
        assert_eq!(compare(&widen_f32("t=0 out s -0\n", &names), "t=0 out s 0\n").len(), 1);
        assert_eq!(compare("t=0 out d -0.0\n", "t=0 out d 0\n").len(), 1);
        assert!(compare("t=0 out d -0.0\n", "t=0 out d -0\n").is_empty());
        assert!(compare("t=0 out n 0\n", "t=0 out n 0\n").is_empty());
    }

    /// Der Wert vor dem Ende eines Laufs zaehlt, auch wenn im selben Tick
    /// noch der `safe`-Wert folgt (12.7).
    #[test]
    fn a_value_overwritten_within_a_tick_is_compared_too() {
        let want = "t=5 out r DEEP_SLEEP_FOR(10 s)\nt=5 out r NONE\n";
        assert!(compare(want, "t=5 out r DEEP_SLEEP_FOR(10 s)\nt=5 out r NONE\n").is_empty());
        assert_eq!(compare(want, "t=5 out r DEEP_SLEEP_FOR(10 ms)\nt=5 out r NONE\n").len(), 1);
        assert_eq!(compare(want, "t=5 out r NONE\n").len(), 1);
    }

    /// Der Wirtsrahmen schreibt jeden Output je Abzug; ein Wert, der den
    /// geltenden nur wiederholt, ist keine Aenderung (9.3).
    #[test]
    fn a_repeated_value_is_no_change() {
        assert!(compare("t=5 out led false\n", "t=5 out led 0\nt=5 out led 0\n").is_empty());
        let interpreted = "t=0 out led true\nt=2 out led false\n";
        assert!(compare(interpreted, "t=0 out led 1\nt=1 out led 1\nt=2 out led 1\nt=2 out led 0\n").is_empty());
    }

    /// Alerts vergleichen sich an ihren Flanken (5.6): Maschine, Flanke und
    /// ob ein ungueltiger Wert sie ausloeste. Den Text kennt nur der
    /// Interpreter.
    #[test]
    fn alerts_compare_by_edge_not_by_text() {
        let interpreted =
            "t=3 alert m on \"warm 31.5\"\nt=5 alert m off \"warm 29.0\"\nt=6 alert m on \"warm (sensor invalid)\"\n";
        assert!(compare(interpreted, "t=3 alert m on\nt=5 alert m off\nt=6 alert m on invalid\n").is_empty());
        assert_eq!(compare(interpreted, "t=3 alert m on\nt=6 alert m on invalid\n").len(), 1);
        assert_eq!(compare(interpreted, "t=3 alert m on\nt=5 alert m off\nt=6 alert m on\n").len(), 1);
    }

    /// Ein Fault vergleicht sich nach Tick, Maschine und Art (FB-330): Jede
    /// davon anders, oder die Zeile nur auf einer Seite, ist eine Abweichung.
    #[test]
    fn a_fault_differs_by_kind_machine_tick_and_side() {
        let want = "t=0 out a 1\nt=4 fault m Range \"x\" -> SAFE\nt=9 out a 2\n";
        assert!(compare(want, "t=0 out a 1\nt=4 fault m Range\nt=9 out a 2\n").is_empty());
        for native in [
            "t=0 out a 1\nt=4 fault m Overflow\nt=9 out a 2\n",
            "t=0 out a 1\nt=4 fault n Range\nt=9 out a 2\n",
            "t=0 out a 1\nt=5 fault m Range\nt=9 out a 2\n",
            "t=0 out a 1\nt=9 out a 2\n",
            "t=0 out a 1\nt=4 fault m Range\nt=4 fault n Range\nt=9 out a 2\n",
        ] {
            let d = compare(want, native);
            assert!(d.iter().any(|d| d.output == "fault"), "{native:?}: {d:?}");
        }
    }

    /// Die Zaehler eines Stroms vergleichen sich je Tick (8.6, FB-361):
    /// `dropped`, `overflowed` und `malformed` einzeln, und eine Zeile nur
    /// auf einer Seite.
    #[test]
    fn a_stream_counter_differs_in_each_field_and_side() {
        let want = "t=0 out a 1\nt=3 stream rx dropped=1 overflowed=0 malformed=2\nt=9 out a 2\n";
        assert!(compare(want, want).is_empty());
        for counters in [
            "dropped=2 overflowed=0 malformed=2",
            "dropped=1 overflowed=1 malformed=2",
            "dropped=1 overflowed=0 malformed=3",
        ] {
            let native = format!("t=0 out a 1\nt=3 stream rx {counters}\nt=9 out a 2\n");
            assert!(compare(want, &native).iter().any(|d| d.output == "stream"), "{counters}");
        }
        assert!(compare(want, "t=0 out a 1\nt=9 out a 2\n").iter().any(|d| d.output == "stream"));
        assert!(compare("t=0 out a 1\nt=9 out a 2\n", want).iter().any(|d| d.output == "stream"));
    }

    /// Faults, Alerts und Zaehler zaehlen nur, soweit beide Traces reichen:
    /// Ein Lauf, der frueher endet, hat die spaeteren nicht verpasst. Ob er
    /// zu frueh endet, prueft der Rahmen, der die Tickzahl kennt (KON1-010).
    #[test]
    fn faults_count_up_to_the_shorter_horizon() {
        let want = "t=0 out a 1\nt=4 fault m Range\n";
        assert!(compare(want, "t=0 out a 1\nt=3 out a 1\n").is_empty());
        assert_eq!(compare(want, "t=0 out a 1\nt=4 out a 1\n").len(), 1);
    }

    /// Jede Beobachtung vergleicht sich je Tick nach Art und Ausgang
    /// (13.5, 13.3, 12.7): der Interpreter mit Maschine und Text, der
    /// Wirtsrahmen und die Boards mit Nummern und Stelle.
    #[test]
    fn every_observation_compares_by_tick_kind_and_outcome() {
        let base = "t=0 out a 1\nt=9 out a 2\n";
        for (interpreted, native, other) in [
            ("t=2 log m \"hallo welt\"", "t=2 log 0 7", "t=3 log 0 7"),
            ("t=2 verify m ok \"p > 0\"", "t=2 verify 0 3 1", "t=2 verify 0 3 0"),
            ("t=2 verify m fail \"p > 0\"", "t=2 verify 1 3 0", "t=2 verify 1 3 1"),
            ("t=2 verdict s pass", "t=2 verdict 2 0 1", "t=2 verdict 2 0 0"),
            ("t=2 verdict s fail \"zu spaet\"", "t=2 verdict 2 0 0", "t=2 verdict 2 0 1"),
            ("t=2 measure m level 0.5 bar", "t=2 measure 0 1 0.5", "t=2 measure 0 1 0.25"),
            ("t=2 measure m level 0.5 bar", "t=2 measure 0 1 bits 4602678819172646912", "t=2 measure 0 1 bits 0"),
            ("t=2 measure m level <invalid>", "t=2 measure 0 1 <invalid>", "t=2 measure 0 1 0"),
            // Eine Dauer schreibt der Interpreter mit Einheit, der Rahmen in ns.
            ("t=2 measure m wait 82 ms", "t=2 measure 0 1 82000000", "t=2 measure 0 1 82"),
            ("t=2 measure m wait 82 ms", "t=2 measure 0 1 bits 4725275336332279808", "t=2 measure 0 1 bits 0"),
            ("t=2 measure m wait -3 s", "t=2 measure 0 1 -3000000000", "t=2 measure 0 1 -3"),
            ("t=7 property disarms violated 2", "t=7 property 0 2", "t=7 property 0 3"),
            ("t=7 assumption rare violated 7", "t=7 property 1 7", "t=8 property 1 7"),
            ("t=5 end deep_sleep", "t=5 end deep_sleep", "t=5 end restart"),
        ] {
            let want = format!("{base}{interpreted}\n");
            assert!(compare(&want, &format!("{base}{native}\n")).is_empty(), "{interpreted} gegen {native}");
            let d = compare(&want, &format!("{base}{other}\n"));
            assert!(d.iter().any(|d| d.output == "observation"), "{interpreted} gegen {other}: {d:?}");
            let d = compare(&want, base);
            assert!(d.iter().any(|d| d.output == "observation"), "{interpreted} nur im Interpreter: {d:?}");
            let d = compare(base, &format!("{base}{native}\n"));
            assert!(d.iter().any(|d| d.output == "observation"), "{native} nur nativ: {d:?}");
        }
    }

    /// Das Ende eines Szenarios waehlt der Interpreter (13.6); der Rahmen
    /// laeuft weiter, und das ist keine Abweichung.
    #[test]
    fn the_end_of_a_scenario_is_the_interpreters() {
        assert!(compare("t=0 out a 1\nt=4 end scenario\n", "t=0 out a 1\nt=4 out a 1\n").is_empty());
    }

    /// Eine Dauer traegt ihre Einheit auf beiden Seiten; eine Groesse nur
    /// im Interpreter.
    #[test]
    fn a_duration_keeps_its_unit() {
        assert_eq!(compare("t=1 out d 10 min\n", "t=1 out d 10 s\n").len(), 1);
        assert!(compare("t=1 out d 10 min\n", "t=1 out d 10 min\n").is_empty());
        assert!(compare("t=1 out u 3.75 V\n", "t=1 out u 3.75\n").is_empty());
    }
}
