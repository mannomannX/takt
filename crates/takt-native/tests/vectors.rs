//! `grammar/takt-native.md` ist normativ: Jede Vektorzeile wird hier
//! ausgefuehrt.
//!
//! Das ist das Aufnahmekriterium aus 13.8 — eine Funktion gehoert in die
//! kuratierte Menge, *nachdem* ihre Vektoren gruen sind. Drei Tests
//! halten die Spezifikation und die Menge im Gleichschritt, darunter der
//! umgekehrte: kein Vektor fuer eine Funktion, die es nicht gibt.

use takt_native::Native;

/// Eine Vektorzeile.
struct Vector {
    line: usize,
    text: String,
    fun: String,
    input: Vec<u8>,
    want: u64,
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

fn parse(line: usize, text: &str) -> Vector {
    let (head, want) = text.split_once(':').unwrap_or_else(|| panic!("Zeile {line}: kein `:` in `{text}`"));
    let mut w = head.split_whitespace();
    let fun = w.next().unwrap_or_else(|| panic!("Zeile {line}: keine Funktion")).to_string();
    let hex = w.next().unwrap_or("-");
    // Ein Bindestrich ist die leere Eingabe; sie hat ein definiertes
    // Ergebnis, und gerade dort sitzen Fehler.
    let input = if hex == "-" {
        Vec::new()
    } else {
        hex.as_bytes()
            .chunks(2)
            .map(|p| {
                u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16)
                    .unwrap_or_else(|_| panic!("Zeile {line}: `{hex}` ist kein Hex"))
            })
            .collect()
    };
    let want = u64::from_str_radix(want.trim(), 16).unwrap_or_else(|_| panic!("Zeile {line}: kein Hex-Ergebnis"));
    Vector { line, text: text.to_string(), fun, input, want }
}

#[test]
fn every_vector_holds() {
    let mut wrong = Vec::new();
    for v in load() {
        let Some(f) = Native::by_name(&v.fun) else {
            wrong.push(format!("Zeile {}: `{}` ist keine native Funktion", v.line, v.fun));
            continue;
        };
        let got = takt_native::apply(f, &v.input);
        if got != v.want {
            wrong.push(format!("Zeile {}: `{}`\n  erhalten {:x}", v.line, v.text, got));
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
    let missing: Vec<&str> =
        Native::ALL.into_iter().filter(|f| !vectors.iter().any(|v| v.fun == f.name())).map(Native::name).collect();
    assert!(missing.is_empty(), "kuratiert, aber ohne Vektor: {}", missing.join(", "));
}

/// Und umgekehrt: kein Vektor fuer eine Funktion, die es nicht gibt.
/// Sonst behauptete die Spezifikation eine Zusage, die der Compiler nicht
/// gibt.
#[test]
fn no_vector_for_an_unknown_function() {
    let stray: Vec<String> = load()
        .iter()
        .filter(|v| Native::by_name(&v.fun).is_none())
        .map(|v| format!("Zeile {}: `{}`", v.line, v.fun))
        .collect();
    assert!(stray.is_empty(), "{}", stray.join("\n"));
}

/// Der bekannteste Pruefwert der CRC-Kataloge: `123456789`.
///
/// Er steht hier noch einmal ausdruecklich, weil er die Stelle ist, an
/// der ein falsches Polynom oder ein vergessenes Invertieren sofort
/// auffaellt.
#[test]
fn the_catalogue_check_values_match() {
    let msg = b"123456789";
    assert_eq!(takt_native::crc::crc32(msg), 0xCBF4_3926, "CRC-32/ISO-HDLC");
    assert_eq!(takt_native::crc::crc32c(msg), 0xE306_9283, "CRC-32C/Castagnoli");
    assert_eq!(takt_native::crc::crc16(msg), 0xBB3D, "CRC-16/ARC");
}

/// Abschnittsweise gerechnet ergibt denselben Wert wie in einem Zug.
#[test]
fn a_split_crc32_matches_the_whole() {
    use takt_native::crc::{crc32, crc32_final, crc32_start, crc32_update};
    let msg = b"123456789";
    for split in 0..=msg.len() {
        let (a, b) = msg.split_at(split);
        let state = crc32_update(crc32_update(crc32_start(), a), b);
        assert_eq!(crc32_final(state), crc32(msg), "Trennung nach {split} Byte");
    }
}

/// 4.5: Jede Funktion traegt einen Kostenvertrag.
#[test]
fn every_function_declares_its_cost() {
    for f in Native::ALL {
        let c = takt_native::cost_of(f);
        assert!(c.per_byte > 0, "{}: keine Kosten je Byte", f.name());
        assert!(c.stack > 0, "{}: kein Stack-Vertrag", f.name());
    }
}
