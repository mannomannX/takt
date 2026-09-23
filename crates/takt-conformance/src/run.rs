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

/// Eine Zeile `t=<tick> out <name> <wert>`.
fn outputs(text: &str) -> BTreeMap<(u64, String), String> {
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
        out.insert((t, name.to_string()), value.join(" "));
    }
    out
}

/// Vergleicht zwei Traces.
///
/// Beide Seiten duerfen eine Zeile nur bei Aenderung schreiben (9.3); der
/// letzte Wert gilt fort. Verglichen wird an jedem Tick, an dem eine Seite
/// schreibt, jeder Output, den beide schon gemeldet haben.
pub fn compare(interpreter: &str, native: &str) -> Vec<Difference> {
    let a = outputs(interpreter);
    let b = outputs(native);
    let mut ticks: Vec<u64> = a.keys().chain(b.keys()).map(|(t, _)| *t).collect();
    ticks.sort_unstable();
    ticks.dedup();
    let (mut want, mut have) = (BTreeMap::new(), BTreeMap::new());
    let mut out = Vec::new();
    for tick in ticks {
        want.extend(at(&a, tick));
        have.extend(at(&b, tick));
        for (name, w) in &want {
            let Some(h) = have.get(name) else { continue };
            if !same_number(w, h) {
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

/// Die Outputs eines Ticks.
fn at(m: &BTreeMap<(u64, String), String>, tick: u64) -> impl Iterator<Item = (&str, &str)> {
    m.range((tick, String::new())..(tick.saturating_add(1), String::new())).map(|((_, n), v)| (n.as_str(), v.as_str()))
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
    let a = interpreter.split_whitespace().next().unwrap_or(interpreter);
    let b = native.split_whitespace().next().unwrap_or(native);
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
