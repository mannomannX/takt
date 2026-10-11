//! Der Regelkatalog des eigenen Codes gegen den Code (plan/cert.md
//! Abschnitt 6, M11 Schritt 33).
//!
//! Der Katalog nennt fuer jede Regel aus MISRA C:2025 Addendum 6, die fuer
//! Rust gilt, was sie haelt. Dieser Test haelt ihn fest: Die Crates der
//! Stufe 2 tragen ihre Lints am Kopf, jede Abweichung eines Katalog-Lints
//! nennt eine Regel des Katalogs, kein Katalog-Lint ist per `allow`
//! abgeschaltet, und was der Katalog als „Pruefung“ nennt, wird hier
//! geprueft.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Abschnitt 6 von plan/cert.md.
fn section() -> String {
    let cert = read(&root().join("plan/cert.md"));
    let from = cert.find("## 6. Regelwerk des eigenen Codes").expect("Abschnitt 6 in plan/cert.md");
    let to = cert[from..].find("\n## 7.").map_or(cert.len(), |n| from + n);
    cert[from..to].to_string()
}

/// Die Zeilen der Tabelle, deren Kopf mit `head` beginnt, als Zellen.
fn table(text: &str, head: &str) -> Vec<Vec<String>> {
    let from = text.find(head).unwrap_or_else(|| panic!("Tabelle `{head}` fehlt"));
    text[from..]
        .lines()
        .take_while(|l| l.starts_with('|'))
        .skip(2)
        .map(|l| l.trim_matches('|').split(" | ").map(|c| c.trim().to_string()).collect())
        .collect()
}

/// Die Woerter in Backticks.
fn ticked(cell: &str) -> Vec<String> {
    cell.split('`').skip(1).step_by(2).map(str::to_string).collect()
}

/// Die Regelnummern (`D.4.1`, `R.21.25`) in einem Text.
fn rule_ids(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .map(|w| w.trim_end_matches('.'))
        .filter(|w| {
            let mut parts = w.split('.');
            matches!(parts.next(), Some("D" | "R"))
                && parts.clone().count() == 2
                && parts.all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
        .collect()
}

/// Die Regeln des Katalogs und wofuer sie gelten.
fn rules() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for row in table(&section(), "| Regeln | gilt für |") {
        for id in rule_ids(&row[0]) {
            let scope = row[1].clone();
            assert!(out.insert(id.clone(), scope).is_none(), "{id} steht zweimal im Katalog");
        }
    }
    out
}

/// Die Lints einer Stufe: `clippy::…` und die von rustc, dazu die
/// Speicherordnungen, die erlaubt sind.
#[derive(Debug, Default)]
struct Tier {
    crates: BTreeSet<String>,
    lints: BTreeSet<String>,
    orderings: BTreeSet<String>,
}

/// Die Crates, deren Verzeichnis unter `crates/` ein `Cargo.toml` hat.
fn own_crates() -> BTreeSet<String> {
    std::fs::read_dir(root().join("crates"))
        .expect("crates/ lesbar")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("Cargo.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Stufe 2 aus der Tabelle der Stufen.
fn tier2() -> Tier {
    let rows = table(&section(), "| Stufe | Crates | dazu |");
    let row = rows.iter().find(|r| r[0] == "2").expect("Zeile der Stufe 2");
    let own = own_crates();
    let mut crates = BTreeSet::new();
    for name in ticked(&row[1]).into_iter().filter(|n| n.starts_with("takt") || n.starts_with("libtaktm")) {
        let found: Vec<&String> = match name.strip_suffix('*') {
            Some(prefix) => own.iter().filter(|c| c.starts_with(prefix)).collect(),
            None => own.iter().filter(|c| **c == name).collect(),
        };
        assert!(!found.is_empty(), "`{name}` aus der Stufe 2 ist kein Crate unter crates/");
        crates.extend(found.into_iter().cloned());
    }
    let mut tier = Tier { crates, ..Tier::default() };
    for part in row[2].split(';').map(str::trim) {
        let names = ticked(part);
        if part.starts_with("Clippy") {
            tier.lints.extend(names.into_iter().map(|n| format!("clippy::{n}")));
        } else if part.starts_with("rustc") {
            tier.lints.extend(names);
        } else if part.starts_with("nur") {
            tier.orderings.extend(names.into_iter().filter_map(|n| n.strip_prefix("Ordering::").map(str::to_string)));
        } else {
            panic!("Teil der Stufe 2 nicht verstanden: {part}");
        }
    }
    tier
}

/// Die Lints, die der Katalog nennt (alle Stufen): Eine Abweichung davon
/// braucht eine Regel.
fn catalog_lints() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for row in table(&section(), "| Stufe | Crates | dazu |") {
        for part in row[2].split(';').map(str::trim) {
            let names = ticked(part);
            if part.starts_with("Clippy") {
                out.extend(names.into_iter().map(|n| format!("clippy::{n}")));
            } else if part.starts_with("rustc") {
                out.extend(names);
            }
        }
    }
    out
}

/// Crates der Stufe 2, die ihre Lints noch nicht tragen (M11 Schritt 33a,
/// in Arbeit). Die Liste schrumpft nur: Wer den Kopf bekommt, faellt
/// heraus, sonst scheitert der Test.
const PENDING: &[&str] = &[
    "libtaktm",
    "takt-board-esp32c6",
    "takt-board-stm32f401",
    "takt-board-support",
    "takt-bringup-esp32c6",
    "takt-bringup-host",
    "takt-bringup-stm32f401",
    "takt-crypto",
    "takt-embed",
    "takt-embed-c",
    "takt-native-abi",
    "takt-trampoline-stm32f401",
];

/// Wo die Lints der Stufe stehen: am Kopf des Crates, bei den Bring-ups am
/// Kopf jedes Programms unter `src/bin`, bei `takt-hal` an den Modulen des
/// Kerns ohne `mir` (Katalog: „der Kern ohne `mir`“).
fn heads(krate: &str) -> Vec<PathBuf> {
    let src = root().join("crates").join(krate).join("src");
    if krate == "takt-hal" {
        return vec![src.join("contract.rs"), src.join("quality.rs")];
    }
    let (lib, main) = (src.join("lib.rs"), src.join("main.rs"));
    if lib.is_file() {
        vec![lib]
    } else if main.is_file() {
        vec![main]
    } else {
        walk(&src.join("bin"))
    }
}

/// Die Lints im `deny(…)` eines inneren Attributs, leer ohne eines.
fn denied(text: &str) -> BTreeSet<String> {
    let Some(at) = text.find("#![").and_then(|a| text[a..].find("deny(").map(|d| a + d + "deny(".len())) else {
        return BTreeSet::new();
    };
    let mut depth = 1usize;
    let end = text[at..]
        .char_indices()
        .find_map(|(i, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(at + i)
        })
        .expect("deny( endet");
    let lints = text[at..end].split("reason").next().unwrap_or("");
    lints.split(',').map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// Die `.rs`-Dateien unter `dir`.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .flat_map(|p| if p.is_dir() { walk(&p) } else { vec![p] })
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        .collect();
    out.sort();
    out
}

/// Der Text einer Quelldatei ohne ihre Tests: alles vor `#[cfg(test)]`.
fn without_tests(text: &str) -> &str {
    text.split("#[cfg(test)]").next().unwrap_or(text)
}

/// Die Attribute `#[expect(…)]`, `#![expect(…)]`, `#[allow(…)]` und
/// `#![allow(…)]` einer Datei: Art, Lints, Grund.
fn lint_attributes(text: &str) -> Vec<(&'static str, Vec<String>, String)> {
    let mut out = Vec::new();
    for (kind, opens) in [("expect", ["#[expect(", "#![expect("]), ("allow", ["#[allow(", "#![allow("])] {
        for open in opens {
            for (at, _) in text.match_indices(open) {
                let body = &text[at + open.len()..];
                let end = body.find(")]").unwrap_or(body.len());
                let body = &body[..end];
                let (lints, reason) = match body.find("reason") {
                    Some(r) => (&body[..r], body[r..].split('"').nth(1).unwrap_or("").to_string()),
                    None => (body, String::new()),
                };
                let lints = lints.split(',').map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
                out.push((kind, lints, reason));
            }
        }
    }
    out
}

#[test]
fn the_catalog_lists_each_applicable_rule_of_the_addendum_once() {
    let rules = rules();
    let count = |scope: &str| rules.values().filter(|s| *s == scope).count();
    // MISRA C:2025 Addendum 6: Required oder Advisory, in sicherem Rust
    // anwendbar (77) oder nur mit `unsafe` (47).
    assert_eq!(count("alle"), 77, "Regeln fuer jedes Crate");
    assert_eq!(count("`unsafe`"), 47, "Regeln nur fuer Crates mit `unsafe`");
    assert_eq!(rules.len(), 124);
}

#[test]
fn every_crate_of_tier_two_carries_its_lints() {
    let tier = tier2();
    assert!(!tier.lints.is_empty() && tier.orderings == BTreeSet::from(["SeqCst".to_string()]), "{tier:?}");
    let pending: BTreeSet<String> = PENDING.iter().map(|s| s.to_string()).collect();
    assert!(pending.is_subset(&tier.crates), "PENDING nennt Crates ausserhalb der Stufe 2");
    for krate in &tier.crates {
        for head in heads(krate) {
            let have = denied(&read(&head));
            if pending.contains(krate) {
                assert!(
                    !tier.lints.is_subset(&have),
                    "`{krate}` traegt die Stufe 2 schon; es faellt aus PENDING heraus ({})",
                    head.display()
                );
                continue;
            }
            let missing: Vec<&String> = tier.lints.difference(&have).collect();
            assert!(missing.is_empty(), "{}: es fehlen {missing:?}", head.display());
        }
    }
}

/// Abweichungen je Crate und Regel. Die Zahl steht hier, damit eine neue
/// Abweichung auffaellt und eine behobene die Ratsche senkt.
const DEVIATIONS: &[(&str, &str, usize)] = &[
    ("takt-hal", "R.10.5", 2),
    ("takt-native", "D.4.1", 1),
    ("takt-native", "R.10.5", 2),
    ("takt-rt-core", "D.4.1", 1),
];

/// Die Regeln, auf die sich die Abweichungen von Katalog-Lints in `text`
/// berufen; ein Fehler fuer ein `allow` oder einen Grund ohne Regel.
fn deviations(text: &str, lints: &BTreeSet<String>, rules: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (kind, named, reason) in lint_attributes(text) {
        let ours: Vec<&String> = named.iter().filter(|l| lints.contains(*l)).collect();
        if ours.is_empty() {
            continue;
        }
        if kind != "expect" {
            return Err(format!("`allow` statt `expect` fuer {ours:?}"));
        }
        let rule = reason.strip_prefix("MISRA C:2025 ").and_then(|r| r.split(':').next()).unwrap_or("");
        if !rules.contains_key(rule) {
            return Err(format!("der Grund `{reason}` nennt keine Regel des Katalogs"));
        }
        out.push(rule.to_string());
    }
    Ok(out)
}

#[test]
fn every_deviation_names_a_rule_of_the_catalog() {
    let (rules, lints) = (rules(), catalog_lints());
    let mut counted: BTreeMap<(String, String), usize> = BTreeMap::new();
    for krate in own_crates() {
        for file in walk(&root().join("crates").join(&krate).join("src")) {
            let found = deviations(&read(&file), &lints, &rules).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
            for rule in found {
                *counted.entry((krate.clone(), rule)).or_default() += 1;
            }
        }
    }
    let expected: BTreeMap<(String, String), usize> =
        DEVIATIONS.iter().map(|(c, r, n)| ((c.to_string(), r.to_string()), *n)).collect();
    assert_eq!(counted, expected, "Abweichungen je Crate und Regel gegen die Ratsche DEVIATIONS");
}

#[test]
fn an_allow_and_a_deviation_without_rule_are_caught() {
    let (rules, lints) = (rules(), catalog_lints());
    let check = |text: &str| deviations(text, &lints, &rules);
    assert!(check("#[allow(clippy::indexing_slicing)]\nfn f() {}").is_err(), "ein `allow`");
    assert!(check("#[expect(clippy::indexing_slicing, reason = \"i < N\")]").is_err(), "ein Grund ohne Regel");
    assert!(
        check("#[expect(clippy::as_conversions, reason = \"MISRA C:2025 R.99.1: x\")]").is_err(),
        "eine fremde Regel"
    );
    assert_eq!(
        check("#[expect(clippy::indexing_slicing, reason = \"MISRA C:2025 D.4.1: i < N\")]"),
        Ok(vec!["D.4.1".into()])
    );
    assert_eq!(check("#[allow(clippy::large_enum_variant, reason = \"kein Katalog-Lint\")]"), Ok(vec![]));
    let head =
        "#![no_std]\n#![cfg_attr(not(test), deny(clippy::indexing_slicing, unreachable_pub, reason = \"a, b\"))]";
    assert_eq!(denied(head), BTreeSet::from(["clippy::indexing_slicing".to_string(), "unreachable_pub".to_string()]));
}

#[test]
fn tier_two_synchronizes_sequentially_consistent() {
    // R.21.25: Stufe 2 nutzt nur `Ordering::SeqCst` (ausserhalb von Tests).
    let tier = tier2();
    for krate in tier.crates.iter().filter(|c| !PENDING.contains(&c.as_str())) {
        for file in walk(&root().join("crates").join(krate).join("src")) {
            let text = read(&file);
            for weaker in ["Relaxed", "Acquire", "Release", "AcqRel"] {
                assert!(
                    !without_tests(&text).contains(&format!("Ordering::{weaker}")),
                    "{}: `Ordering::{weaker}` (R.21.25)",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn tier_two_without_std_does_not_allocate() {
    // D.4.12: Ein Crate der Stufe 2 ohne `std` bindet `alloc` nicht.
    for krate in tier2().crates {
        let src = root().join("crates").join(&krate).join("src");
        let Some(head) = heads(&krate).into_iter().next() else { continue };
        if !read(&head).contains("no_std") {
            continue;
        }
        for file in walk(&src) {
            assert!(!read(&file).contains("extern crate alloc"), "{}: `alloc` (D.4.12)", file.display());
        }
    }
}

#[test]
fn no_own_crate_uses_unstable_features_unions_or_procedural_macros() {
    for krate in own_crates() {
        let dir = root().join("crates").join(&krate);
        // R.20.7: Makros nur als `macro_rules!`.
        let manifest = read(&dir.join("Cargo.toml"));
        assert!(!manifest.contains("proc-macro = true"), "`{krate}` ist ein prozedurales Makro (R.20.7)");
        for file in walk(&dir.join("src")) {
            let text = read(&file);
            // D.1.2: nur stabiles Rust.
            assert!(!text.contains("#![feature("), "{}: instabiles Feature (D.1.2)", file.display());
            // R.19.2: keine `union`.
            for line in text.lines().map(str::trim_start).filter(|l| !l.starts_with("//")) {
                let item = line.strip_prefix("pub ").or_else(|| line.strip_prefix("pub(crate) ")).unwrap_or(line);
                assert!(!item.starts_with("union "), "{}: `union` (R.19.2): {line}", file.display());
            }
        }
    }
}

/// Die Namen, die `extern "C"`-Bloecke einer Datei einfuehren.
fn imported(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (at, _) in text.match_indices("extern \"C\" {") {
        let body = &text[at..];
        // Ein `extern`-Block deklariert nur; die erste schliessende Klammer
        // ist sein Ende.
        let body = &body[..body.find('}').unwrap_or(body.len())];
        for line in body.lines().map(str::trim) {
            let rest = line.strip_prefix("pub ").unwrap_or(line);
            let rest = rest.strip_prefix("safe ").or_else(|| rest.strip_prefix("unsafe ")).unwrap_or(rest);
            let rest = rest
                .strip_prefix("fn ")
                .or_else(|| rest.strip_prefix("static mut "))
                .or_else(|| rest.strip_prefix("static "));
            if let Some(rest) = rest {
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                out.push(name);
            }
        }
    }
    out
}

#[test]
fn crates_with_unsafe_import_nothing_from_the_c_library_but_abort() {
    // R.21.x, R.22.x: Die `extern`-Bloecke binden Symbole des eigenen
    // Rahmens (`takt_…`) und des Linkers (`_…`); aus der C-Bibliothek nur
    // `abort` im Panic-Handler (Abweichung zu R.21.8).
    const ABORT: &[&str] = &["takt-embed-c", "takt-native-abi"];
    let workspace = read(&root().join("Cargo.toml"));
    for krate in own_crates().into_iter().filter(|c| !workspace.contains(&format!("\"crates/{c}\""))) {
        for file in walk(&root().join("crates").join(&krate).join("src")) {
            for name in imported(&read(&file)) {
                let ok = name.starts_with("takt_")
                    || name.starts_with('_')
                    || (name == "abort" && ABORT.contains(&krate.as_str()));
                assert!(ok, "{}: `{name}` aus der C-Bibliothek (R.21, R.22)", file.display());
            }
        }
    }
}
