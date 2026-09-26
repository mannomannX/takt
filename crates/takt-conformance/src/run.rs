//! Der Vergleich: Interpreter gegen erzeugten Code (13.8).
//!
//! **Was verglichen wird.** Die Outputs je Tick, in kanonischer Ordnung.
//! 9.4.4 spricht von Outputs, und plan/m4.md 2.5 hat daraus die Abnahme
//! gemacht: Der Trace ist die Zusage, der Zustand ein Diagnosewerkzeug.
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

/// Vergleicht zwei Traces.
///
/// Beide Seiten duerfen eine Zeile nur bei Aenderung schreiben (9.3); der
/// letzte Wert gilt fort. Verglichen wird an jedem Tick, an dem eine Seite
/// schreibt, jeder Output, den beide schon gemeldet haben. Schreiben beide
/// einen Output mehrmals im selben Tick — den Wert vor und nach dem Ende
/// eines Laufs (12.7) —, zaehlt die Folge ihrer Aenderungen und nicht nur
/// ihr letzter Wert; ein Wert gleich dem geltenden ist keine Aenderung,
/// weil der Wirtsrahmen jeden Output je Abzug schreibt.
pub fn compare(interpreter: &str, native: &str) -> Vec<Difference> {
    let a = outputs(interpreter);
    let b = outputs(native);
    let mut ticks: Vec<u64> = a.keys().chain(b.keys()).map(|(t, _)| *t).collect();
    ticks.sort_unstable();
    ticks.dedup();
    let (mut want, mut have) = (BTreeMap::new(), BTreeMap::new());
    let mut out = Vec::new();
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
    out
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

    /// Eine Dauer traegt ihre Einheit auf beiden Seiten; eine Groesse nur
    /// im Interpreter.
    #[test]
    fn a_duration_keeps_its_unit() {
        assert_eq!(compare("t=1 out d 10 min\n", "t=1 out d 10 s\n").len(), 1);
        assert!(compare("t=1 out d 10 min\n", "t=1 out d 10 min\n").is_empty());
        assert!(compare("t=1 out u 3.75 V\n", "t=1 out u 3.75\n").is_empty());
    }
}
