//! Welche Suite welches Korpusprogramm faehrt (FB-378, M11 Schritte 28 und
//! 29): `corpus-try/manifest.csv` ist die einzige Quelle.
//!
//! Die Listen der Suiten standen von Hand im Test: Ein neues Korpusprogramm
//! lief in keinem Vergleich, bis jemand es nachtrug, und eine Luecke ohne
//! Grund wurde nie wieder geprueft. Das Manifest fuehrt darum je Programm
//! die Spalte `Suiten`: die Suiten, die es faehrt, und jede Ausnahme mit `!`
//! und Grund (`!board kein Port auf dem F401 (FB-123)`), durch `;` getrennt.
//!
//! **Jede Suite faehrt jedes Programm.** Eine Ausnahme braucht einen echten
//! Grund — die Suite kann eine Konstruktion nicht — mit FB-Zeile oder
//! Schritt, oder sie folgt der Auswahlregel der Suite ([`selects`]) und
//! heisst dann `!suite Regel`. Ob die Eintraege der Regel folgen, prueft
//! `tests/suites.rs`.

use std::sync::OnceLock;

use takt_mir::Program;
use takt_mir::census::{Construct, Feature, StmtTag, TypeTag, census};

/// Die Suiten, die ein Programm fahren oder begruendet auslassen muss:
/// Interpreter gegen nativ (`differential.rs`), die MCU-Ziele
/// (`mcu_codegen.rs`, `mcu_harness.rs`), x86-64 gegen aarch64
/// (`targets.rs`), die Boards (`board::corpus`), die feindliche
/// Fliesskomma-Umgebung (`embed.rs`) und der Beweiser (`takt-prove`).
pub const SUITES: [&str; 6] = ["vergleich", "mcu", "ziele", "board", "ieee", "beweiser"];

/// Die Beispiele unter `corpus-try/sim/`, die Abnahme und Boards neben dem
/// Korpus fahren: die Startmuster (12.7) und die Plattform ohne Startstufe.
/// Ihr Manifest (`sim/manifest.csv`) fuehrt keine Suiten.
pub const EXAMPLES: [&str; 2] = ["sim/12_7/program.takt", "sim/14_7/program.takt"];

/// Der Grund einer Ausnahme, die der Auswahlregel folgt ([`selects`]).
pub const RULE: &str = "Regel";

/// **Die Auswahlregel einer Suite** (FB-378): Jede Suite faehrt jedes
/// Programm, das die Sema annimmt (`None` heisst: sie lehnt es ab). Zwei sind
/// fuer den ganzen Korpus zu teuer und waehlen nach dem, was sie pruefen:
///
/// - `ziele` (x86-64 gegen aarch64): Programme mit Fliesskomma. Beide Ziele
///   gehoeren derselben Klasse an und bekommen dieselbe IR (12.8); Ganzzahlen
///   rechnen sie gleich, abweichen kann nur die Befehlsauswahl fuer
///   Fliesskomma.
/// - `ieee` (verstellte Fliesskomma-Umgebung des Wirts): Programme, die in
///   ihr rechnen — Fliesskomma, Natives, deren Rust-Code im Einstieg des
///   Rahmens laeuft, und Jobs und Journal mit ihren eigenen Einstiegen
///   (12.11, KON1-020).
pub fn selects(suite: &str, p: Option<&Program>) -> bool {
    let Some(p) = p else { return false };
    let used = || census(p);
    let float = |c: &std::collections::BTreeSet<Construct>| {
        c.contains(&Construct::Type(TypeTag::Float)) || c.contains(&Construct::Type(TypeTag::Mat))
    };
    match suite {
        "ziele" => float(&used()),
        "ieee" => {
            let c = used();
            float(&c)
                || !p.natives.is_empty()
                || c.contains(&Construct::Stmt(StmtTag::Job))
                || c.contains(&Construct::Feature(Feature::Persist))
        }
        _ => true,
    }
}

/// Die Zeilen einer CSV mit Komma und Anfuehrungszeichen (RFC 4180).
pub fn csv_rows(text: &str) -> impl Iterator<Item = Vec<String>> + '_ {
    let mut chars = text.chars().peekable();
    std::iter::from_fn(move || {
        chars.peek()?;
        let (mut row, mut cell, mut quoted) = (Vec::new(), String::new(), false);
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                ('"', _) => quoted = !quoted,
                (',', false) => row.push(std::mem::take(&mut cell)),
                ('\n', false) => break,
                ('\r', false) => {}
                _ => cell.push(c),
            }
        }
        row.push(cell);
        Some(row)
    })
}

/// Je Zeile des Manifests die Datei und ihre Zelle `Suiten`; leer, wo die
/// Spalte fehlt.
pub fn manifest() -> &'static [(String, String)] {
    static ROWS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ROWS.get_or_init(|| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/manifest.csv");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut rows = csv_rows(&text);
        let head = rows.next().unwrap_or_default();
        let file = head.iter().position(|c| c == "Datei").expect("manifest.csv: Spalte `Datei`");
        let suites = head.iter().position(|c| c == "Suiten");
        rows.filter_map(|r| {
            let cell = suites.and_then(|s| r.get(s)).cloned().unwrap_or_default();
            Some((r.get(file)?.clone(), cell))
        })
        .collect()
    })
}

/// Die Teile einer Zelle `Suiten`: je Suite, ob sie gefahren wird, und der
/// Grund einer Ausnahme.
pub fn parts(entry: &str) -> impl Iterator<Item = (&str, bool, &str)> {
    entry.split(';').map(str::trim).filter(|p| !p.is_empty()).map(|part| match part.strip_prefix('!') {
        Some(rest) => {
            let (suite, reason) = rest.split_once(' ').unwrap_or((rest, ""));
            (suite, false, reason.trim())
        }
        None => (part, true, ""),
    })
}

/// Ob ein Grund eine Zeile im Plan nennt: `FB-<n>` oder `Schritt <n>`.
fn names_a_row(reason: &str) -> bool {
    ["FB-", "Schritt "].iter().any(|key| {
        reason.match_indices(key).any(|(at, _)| reason[at + key.len()..].starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// Prueft eine Zelle `Suiten`: Jede Suite ist genannt, gefahren oder
/// ausgelassen nach der Regel ([`RULE`]) oder mit einem Grund, der eine
/// FB-Zeile oder einen Schritt nennt; liefert die Fehler.
pub fn check(entry: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let mut named = Vec::new();
    for (suite, runs, reason) in parts(entry) {
        if !SUITES.contains(&suite) {
            errors.push(format!("unbekannte Suite `{suite}`"));
        }
        if !runs && reason != RULE && !names_a_row(reason) {
            errors.push(format!("`!{suite}` ohne Grund mit FB-Zeile oder Schritt"));
        }
        named.push(suite);
    }
    for suite in SUITES {
        if !named.contains(&suite) {
            errors.push(format!("`{suite}` weder gefahren noch mit `!` begruendet"));
        }
    }
    errors
}

/// Die Korpusprogramme, die die Suite `suite` faehrt, in der Reihenfolge
/// des Manifests.
pub fn programs(suite: &str) -> Vec<&'static str> {
    assert!(SUITES.contains(&suite), "unbekannte Suite `{suite}`");
    manifest()
        .iter()
        .filter(|(_, entry)| parts(entry).any(|(s, runs, _)| runs && s == suite))
        .map(|(file, _)| file.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{check, csv_rows};

    /// Die Pruefung einer Zelle: jede Suite genannt, Ausnahmen mit Grund.
    #[test]
    fn an_entry_names_every_suite_and_every_reason() {
        assert!(check("vergleich; mcu; ziele; board; ieee; beweiser").is_empty());
        assert!(check("vergleich; mcu; ziele; !board Port auf mmio des C6 (FB-123); ieee; beweiser").is_empty());
        assert_eq!(check("vergleich; mcu; ziele; !board; ieee; beweiser").len(), 1, "Ausnahme ohne Grund");
        assert_eq!(check("vergleich; mcu; ziele; !board Grund offen; ieee; beweiser").len(), 1, "ohne Zeile");
        assert_eq!(check("vergleich; mcu; ziele; !board FB-Zeile fehlt; ieee; beweiser").len(), 1, "ohne Nummer");
        assert!(check("vergleich; mcu; ziele; !board kann es nicht (M11 Schritt 29); ieee; beweiser").is_empty());
        assert!(check("vergleich; mcu; !ziele Regel; board; !ieee Regel; beweiser").is_empty());
        assert_eq!(check("vergleich; mcu; ziele; ieee; beweiser").len(), 1, "board fehlt");
        assert_eq!(check("vergleich; mcu; ziele; board; ieee; beweiser; flug").len(), 1, "unbekannte Suite");
    }

    /// Anfuehrungszeichen schuetzen Komma und Zeilenumbruch, `""` ist ein
    /// Anfuehrungszeichen.
    #[test]
    fn a_quoted_cell_keeps_its_commas_and_quotes() {
        let rows: Vec<Vec<String>> = csv_rows("a,\"b, \"\"c\"\"\nd\",e\r\nf\n").collect();
        assert_eq!(rows, [vec!["a", "b, \"c\"\nd", "e"], vec!["f"]]);
    }
}
