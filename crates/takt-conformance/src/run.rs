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
/// Verglichen werden nur Outputs, die *beide* Seiten melden. Der
/// Interpreter schreibt eine Zeile nur bei Aenderung (9.3), der
/// Testrahmen in jedem Tick — ein Output, den nur eine Seite nennt, ist
/// darum kein Unterschied, sondern eine andere Schreibweise.
pub fn compare(interpreter: &str, native: &str) -> Vec<Difference> {
    let a = outputs(interpreter);
    let b = outputs(native);
    let mut out = Vec::new();
    // Der Interpreter schreibt nur Aenderungen; sein letzter Wert gilt
    // fort, bis ein neuer kommt (9.3).
    let mut zuletzt: BTreeMap<String, String> = BTreeMap::new();
    let mut ticks: Vec<u64> = b.keys().map(|(t, _)| *t).collect();
    ticks.dedup();
    for tick in ticks {
        for ((t, name), value) in &a {
            if *t == tick {
                zuletzt.insert(name.clone(), value.clone());
            }
        }
        for ((t, name), native_value) in &b {
            if *t != tick {
                continue;
            }
            let Some(want) = zuletzt.get(name) else { continue };
            if !same_number(want, native_value) {
                out.push(Difference {
                    tick,
                    output: name.clone(),
                    interpreter: want.clone(),
                    native: native_value.clone(),
                });
            }
        }
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
