//! Die Grenze der TCB am Verzeichnis (9.5, 13.4; `plan/cert.md`) und die
//! Komponentenliste der Referenz gegen den Workspace (11.1, FB-333).
//!
//! Jedes Crate des Workspace erbt `unsafe_code = "forbid"`; was `unsafe`
//! braucht, liegt in einem eigenen Workspace und steht im Manifest. Eine
//! Zeile ohne Crate oder ein Crate ohne Zeile faellt hier auf, ebenso eine
//! Native ohne Eintrag und eine Komponente der Referenz, die es nicht gibt.

use std::collections::BTreeSet;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Die Verzeichnisse unter `crates/` mit einem `Cargo.toml`.
fn crate_dirs() -> BTreeSet<String> {
    std::fs::read_dir(root().join("crates"))
        .expect("crates/ lesbar")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("Cargo.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Die Mitglieder des Workspace aus `Cargo.toml`.
fn members() -> BTreeSet<String> {
    let cargo = read("Cargo.toml");
    let list = &cargo[cargo.find("members = [").expect("members")..];
    let list = &list[..list.find(']').expect("Ende von members")];
    list.lines().filter_map(|l| l.trim().strip_prefix("\"crates/")?.strip_suffix("\",").map(str::to_string)).collect()
}

/// Eine Spalte der Tabelle unter `heading` in `plan/cert.md`.
fn column(heading: &str, index: usize) -> Vec<String> {
    let cert = read("plan/cert.md");
    let from = cert.find(heading).unwrap_or_else(|| panic!("`{heading}` fehlt in plan/cert.md"));
    cert[from..]
        .lines()
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .skip(2)
        .filter_map(|l| l.split('|').nth(index + 1).map(|c| c.trim().to_string()))
        .collect()
}

#[test]
fn every_crate_outside_the_workspace_is_in_the_manifest() {
    // Was die Mitglieder erben, steht in der Wurzel: `forbid`, nicht `deny`
    // (ein `allow` im Crate hebt es nicht auf), und nicht `warn` (KON2-034).
    let cargo = read("Cargo.toml");
    let rust = cargo.split("[workspace.lints.rust]").nth(1).expect("`[workspace.lints.rust]` in Cargo.toml");
    let section: Vec<&str> = rust.lines().map(str::trim).take_while(|l| !l.starts_with('[')).collect();
    assert!(section.contains(&"unsafe_code = \"forbid\""), "die Wurzel verbietet `unsafe` nicht: {section:?}");
    let (dirs, members) = (crate_dirs(), members());
    for m in &members {
        let manifest = read(&format!("crates/{m}/Cargo.toml"));
        assert!(manifest.contains("[lints]\nworkspace = true"), "`{m}` erbt `forbid(unsafe_code)` nicht");
    }
    let outside: BTreeSet<String> = dirs.difference(&members).cloned().collect();
    let listed: BTreeSet<String> = column("### Crates mit `unsafe`", 0).into_iter().collect();
    assert_eq!(outside, listed, "Crates ausserhalb des Workspace gegen das Manifest in plan/cert.md");
    for c in &outside {
        let src = root().join("crates").join(c).join("src");
        let allows = walk(&src)
            .iter()
            .any(|f| std::fs::read_to_string(f).is_ok_and(|t| t.contains("#![allow(unsafe_code, reason = ")));
        assert!(allows, "`{c}` nennt keinen Grund fuer `unsafe` (`#![allow(unsafe_code, reason = …)]`)");
    }
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .flat_map(|p| if p.is_dir() { walk(&p) } else { vec![p] })
        .filter(|p| p.extension().is_some_and(|x| x == "rs"))
        .collect()
}

#[test]
fn every_native_has_an_entry_in_the_manifest() {
    let names: Vec<String> = column("### Native Funktionen", 0);
    let crates: Vec<String> = column("### Native Funktionen", 1);
    let all: BTreeSet<String> = takt_native::Native::ALL.iter().map(|n| n.name().to_string()).collect();
    assert_eq!(names.iter().cloned().collect::<BTreeSet<_>>(), all, "kuratierte Natives gegen plan/cert.md");
    assert!(crates.iter().all(|c| crate_dirs().contains(c)), "unbekanntes Crate in {crates:?}");
}

#[test]
fn the_components_of_11_1_are_the_workspace() {
    let reference = read("plan/definition.md");
    let from = reference.find("### 11.1 Komponenten").expect("11.1");
    let block = &reference[from..];
    let block = &block[block.find("```\n").expect("Codeblock") + 4..];
    let block = &block[..block.find("```").expect("Ende des Codeblocks")];
    let mut built = BTreeSet::new();
    let mut planned = BTreeSet::new();
    let mut cli = None;
    for line in block.lines() {
        let (name, rest) = line.split_once(' ').expect("Name und Beschreibung");
        let rest = rest.trim_start();
        if rest.starts_with("(geplant") {
            planned.insert(name.to_string());
        } else {
            built.insert(name.to_string());
        }
        if name == "takt-cli" {
            cli = Some(rest.to_string());
        }
    }
    let dirs = crate_dirs();
    assert_eq!(built, dirs, "11.1 gegen crates/");
    assert!(planned.is_disjoint(&dirs), "geplant und doch da: {:?}", planned.intersection(&dirs).collect::<Vec<_>>());

    let cli = cli.expect("Zeile takt-cli");
    let (now, later) = cli.split_once("; geplant:").expect("`; geplant:` in der Zeile takt-cli");
    let listed: BTreeSet<String> = now.split(" | ").map(|c| c.trim().to_string()).collect();
    let main = read("crates/takt-cli/src/main.rs");
    let dispatch = &main[main.find("let ok = match command.as_str() {").expect("Verteiler")..];
    let dispatch = &dispatch[..dispatch.find("_ =>").expect("Ende des Verteilers")];
    let commands: BTreeSet<String> = dispatch
        .lines()
        .filter_map(|l| l.trim().strip_prefix('"')?.split_once('"').map(|(c, _)| c.to_string()))
        .collect();
    assert_eq!(listed, commands, "Kommandos in 11.1 gegen `takt-cli`");
    for c in later.split(" | ").map(|c| c.split(" (").next().unwrap_or(c).trim()) {
        assert!(!commands.contains(c), "`{c}` ist geplant und doch da");
    }
}
