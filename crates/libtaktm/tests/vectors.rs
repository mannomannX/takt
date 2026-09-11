//! `grammar/libtaktm.md` ist normativ: Jede Vektorzeile wird hier
//! ausgefuehrt.
//!
//! Das ist zugleich das Aufnahmekriterium aus 13.8 — eine Funktion
//! gehoert in die kuratierte Menge, *nachdem* ihre Bit-Gleichheit belegt
//! ist. Der Test vergleicht Bitmuster, nicht Zahlen: `0.1 + 0.2 == 0.3`
//! ist falsch, aber `a.to_bits() == b.to_bits()` ist die Frage, die
//! Satz 9.4.4 stellt.

use libtaktm::{Curated, Fun, curated};

/// Eine Vektorzeile.
struct Vector {
    line: usize,
    text: String,
    width: Width,
    fun: String,
    args: Vec<u64>,
    want: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    F32,
    F64,
}

/// Liest die Vektoren aus dem Block ```` ```libtaktm ```` der Spezifikation.
fn load() -> Vec<Vector> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/libtaktm.md");
    let text = std::fs::read_to_string(path).expect("grammar/libtaktm.md lesbar");
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("```") {
            inside = line == "```libtaktm";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        out.push(parse(i + 1, line));
    }
    assert!(out.len() > 60, "die Spezifikation traegt ihre Vektoren: {}", out.len());
    out
}

fn parse(line: usize, text: &str) -> Vector {
    let (head, tail) = text.split_once(':').unwrap_or_else(|| panic!("Zeile {line}: kein `:` in `{text}`"));
    let (w, fun) = head.split_once(' ').unwrap_or_else(|| panic!("Zeile {line}: Breite und Funktion erwartet"));
    let width = match w {
        "f64" => Width::F64,
        "f32" => Width::F32,
        other => panic!("Zeile {line}: unbekannte Breite `{other}`"),
    };
    let (args, want) = tail.split_once("->").unwrap_or_else(|| panic!("Zeile {line}: kein `->`"));
    let hex =
        |s: &str| u64::from_str_radix(s.trim(), 16).unwrap_or_else(|_| panic!("Zeile {line}: `{s}` ist kein Hex"));
    Vector {
        line,
        text: text.to_string(),
        width,
        fun: fun.trim().to_string(),
        args: args.split_whitespace().map(hex).collect(),
        want: hex(want),
    }
}

/// Rechnet einen Vektor nach und liefert das Ergebnis als Bitmuster.
fn apply(v: &Vector) -> u64 {
    let f64s: Vec<f64> = v.args.iter().map(|b| f64::from_bits(*b)).collect();
    let f32s: Vec<f32> = v.args.iter().map(|b| f32::from_bits(*b as u32)).collect();
    let a64 = |i: usize| f64s[i];
    let a32 = |i: usize| f32s[i];
    match (v.width, v.fun.as_str()) {
        (Width::F64, "sqrt") => libtaktm::sqrt_f64(a64(0)).to_bits(),
        (Width::F64, "fma") => libtaktm::fma_f64(a64(0), a64(1), a64(2)).to_bits(),
        (Width::F64, "round") => libtaktm::round_f64(a64(0)).to_bits(),
        (Width::F64, "floor") => libtaktm::floor_f64(a64(0)).to_bits(),
        (Width::F64, "ceil") => libtaktm::ceil_f64(a64(0)).to_bits(),
        (Width::F64, "trunc") => libtaktm::trunc_f64(a64(0)).to_bits(),
        (Width::F64, "abs") => libtaktm::fabs_f64(a64(0)).to_bits(),
        (Width::F64, "copysign") => libtaktm::copysign_f64(a64(0), a64(1)).to_bits(),
        (Width::F32, "sqrt") => u64::from(libtaktm::sqrt_f32(a32(0)).to_bits()),
        (Width::F32, "fma") => u64::from(libtaktm::fma_f32(a32(0), a32(1), a32(2)).to_bits()),
        (Width::F32, "round") => u64::from(libtaktm::round_f32(a32(0)).to_bits()),
        (Width::F32, "floor") => u64::from(libtaktm::floor_f32(a32(0)).to_bits()),
        (Width::F32, "ceil") => u64::from(libtaktm::ceil_f32(a32(0)).to_bits()),
        (Width::F32, "trunc") => u64::from(libtaktm::trunc_f32(a32(0)).to_bits()),
        (Width::F32, "abs") => u64::from(libtaktm::fabs_f32(a32(0)).to_bits()),
        (Width::F32, "copysign") => u64::from(libtaktm::copysign_f32(a32(0), a32(1)).to_bits()),
        (_, other) => panic!("Zeile {}: unbekannte Funktion `{other}`", v.line),
    }
}

#[test]
fn every_vector_holds() {
    let mut wrong = Vec::new();
    for v in load() {
        let got = apply(&v);
        if got != v.want {
            let w = if v.width == Width::F32 { 8 } else { 16 };
            wrong.push(format!("Zeile {}: `{}`\n  erhalten {:0w$x}", v.line, v.text, got));
        }
    }
    assert!(wrong.is_empty(), "{} Vektoren weichen ab:\n{}", wrong.len(), wrong.join("\n"));
}

/// Die Spezifikation deckt jede kuratierte Funktion ab.
///
/// Ohne diesen Test koennte eine Funktion in die kuratierte Menge
/// geraten, ohne je einen Vektor gesehen zu haben — genau das, was 13.8
/// ausschliesst.
#[test]
fn every_curated_function_has_vectors() {
    let vectors = load();
    let mut missing = Vec::new();
    for f in Fun::ALL {
        if curated(f) != Curated::Yes {
            continue;
        }
        for width in ["f64", "f32"] {
            let found = vectors
                .iter()
                .any(|v| v.fun == f.name() && (if v.width == Width::F64 { "f64" } else { "f32" }) == width);
            if !found {
                missing.push(format!("{} {}", width, f.name()));
            }
        }
    }
    assert!(missing.is_empty(), "kuratiert, aber ohne Vektor: {}", missing.join(", "));
}

/// Umgekehrt: Kein Vektor steht fuer eine Funktion, die nicht kuratiert
/// ist. Sonst behauptete die Spezifikation eine Zusage, die der Compiler
/// nicht gibt.
#[test]
fn no_vector_for_an_uncurated_function() {
    let mut stray = Vec::new();
    for v in load() {
        let Some(f) = Fun::ALL.into_iter().find(|f| f.name() == v.fun) else {
            stray.push(format!("Zeile {}: `{}` ist keine Funktion der Bibliothek", v.line, v.fun));
            continue;
        };
        if curated(f) != Curated::Yes {
            stray.push(format!("Zeile {}: `{}` ist nicht kuratiert", v.line, v.fun));
        }
    }
    assert!(stray.is_empty(), "{}", stray.join("\n"));
}
