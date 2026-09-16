//! `grammar/takt-native.md` ist normativ: Jede Vektorzeile wird hier
//! ausgefuehrt.
//!
//! Das ist das Aufnahmekriterium aus 13.8 — eine Funktion gehoert in die
//! kuratierte Menge, *nachdem* ihre Vektoren gruen sind. Drei Tests
//! halten die Spezifikation und die Menge im Gleichschritt, darunter der
//! umgekehrte: kein Vektor fuer eine Funktion, die es nicht gibt.

use takt_native::{Native, Output};

/// Eine Vektorzeile.
struct Vector {
    line: usize,
    text: String,
    fun: String,
    inputs: Vec<Vec<u8>>,
    want: String,
}

/// Liest die Vektoren aus dem Block ```` ```takt-native ```` der
/// Spezifikation.
fn load() -> Vec<Vector> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/takt-native.md");
    let text = std::fs::read_to_string(path).expect("grammar/takt-native.md lesbar");
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("```") {
            inside = line == "```takt-native";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(parse(i + 1, line));
    }
    assert!(out.len() >= 32, "die Spezifikation traegt ihre Vektoren: {}", out.len());
    out
}

/// Eine Hexfolge; ein Bindestrich ist die leere Eingabe — auch sie hat
/// ein definiertes Ergebnis, und gerade dort sitzen Fehler.
fn hex_bytes(line: usize, hex: &str) -> Vec<u8> {
    if hex == "-" {
        return Vec::new();
    }
    hex.as_bytes()
        .chunks(2)
        .map(|p| {
            u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16)
                .unwrap_or_else(|_| panic!("Zeile {line}: `{hex}` ist kein Hex"))
        })
        .collect()
}

fn parse(line: usize, text: &str) -> Vector {
    let (head, want) = text.split_once(':').unwrap_or_else(|| panic!("Zeile {line}: kein `:` in `{text}`"));
    let mut w = head.split_whitespace();
    let fun = w.next().unwrap_or_else(|| panic!("Zeile {line}: keine Funktion")).to_string();
    let inputs: Vec<Vec<u8>> = w.map(|t| hex_bytes(line, t)).collect();
    assert!(!inputs.is_empty(), "Zeile {line}: keine Eingabe");
    Vector { line, text: text.to_string(), fun, inputs, want: want.trim().to_lowercase() }
}

/// Das Ergebnis in der Schreibweise der Spezifikation: eine Pruefsumme in
/// der Breite ihres Vektors, ein Digest Byte fuer Byte.
fn render(out: Output, width: usize) -> String {
    match out {
        Output::Scalar(v) => format!("{v:0width$x}"),
        Output::Digest(d) => d.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

#[test]
fn every_vector_holds() {
    let mut wrong = Vec::new();
    for v in load() {
        let Some(f) = Native::by_name(&v.fun) else {
            wrong.push(format!("Zeile {}: `{}` ist keine native Funktion", v.line, v.fun));
            continue;
        };
        let inputs: Vec<&[u8]> = v.inputs.iter().map(Vec::as_slice).collect();
        let Some(got) = takt_native::call(f, &inputs) else {
            wrong.push(format!("Zeile {}: `{}` nimmt nicht {} Eingaben", v.line, v.fun, inputs.len()));
            continue;
        };
        let got = render(got, v.want.len());
        if got != v.want {
            wrong.push(format!("Zeile {}: `{}`\n  erhalten {got}", v.line, v.text));
        }
    }
    assert!(wrong.is_empty(), "{} Vektoren weichen ab:\n{}", wrong.len(), wrong.join("\n"));
}

/// Die Spezifikation deckt jede Funktion der Menge ab.
///
/// Ohne diesen Test koennte eine Funktion in die kuratierte Menge
/// geraten, ohne je einen Vektor gesehen zu haben — genau das, was 13.8
/// ausschliesst.
#[test]
fn every_curated_function_has_vectors() {
    let vectors = load();
    let missing: Vec<&str> = Native::ALL
        .into_iter()
        .filter(|f| !f.external() && !vectors.iter().any(|v| v.fun == f.vector_name()))
        .map(Native::name)
        .collect();
    assert!(missing.is_empty(), "kuratiert, aber ohne Vektor: {}", missing.join(", "));
}

/// Und umgekehrt: kein Vektor fuer eine Funktion, die es nicht gibt.
/// Sonst behauptete die Spezifikation eine Zusage, die der Compiler nicht
/// gibt.
#[test]
fn no_vector_names_an_unknown_function() {
    let unknown: Vec<String> =
        load().into_iter().filter(|v| Native::by_name(&v.fun).is_none()).map(|v| v.fun).collect();
    assert!(unknown.is_empty(), "Vektoren ohne Funktion: {}", unknown.join(", "));
}
