//! Der C-Teil nach CERT C und MISRA C (plan/cert.md Abschnitt 6, „Der
//! C-Teil“; M11 Schritt 33b, FB-524): `clang-tidy` und `cppcheck` mit dem
//! MISRA-Addon ueber den Rahmen, den `takt build --emit embed` fuer einige
//! Programme schreibt, den Kopf des SDK, den POSIX-Port und die Beispiele.
//!
//! **Der Katalog ist die Quelle.** Wie die Werkzeuge gerufen werden und
//! welche Befunde als Abweichung stehen, liest dieser Test aus
//! plan/cert.md. Jede Abweichung nennt Teil, Zahl und Regel; die Zahl haelt
//! er genau: Ein neuer Befund scheitert wie ein behobener, die Zahl faellt
//! nur.
//!
//! Die Pruefung braucht die Kopfdateien von POSIX und laeuft darum unter
//! Linux; auf CI in `.github/workflows/analysis.yml`. Ohne beide Werkzeuge
//! besteht sie nicht (FB-392): `TAKT_CLANG_TIDY` und `TAKT_CPPCHECK` nennen
//! sie, sonst `clang-tidy` neben clang und `cppcheck` im Pfad.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    std::fs::canonicalize(&dir).unwrap_or(dir)
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    std::fs::canonicalize(&dir).expect("Verzeichnis")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// --- Der Katalog ------------------------------------------------------------

/// Der Abschnitt „Der C-Teil“ in plan/cert.md.
fn section() -> String {
    let cert = read(&root().join("plan/cert.md"));
    let from = cert.find("### Der C-Teil").expect("Abschnitt „Der C-Teil“ in plan/cert.md");
    let to = cert[from + 1..].find("\n#").map_or(cert.len(), |n| from + 1 + n);
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

/// Ein Werkzeug, wie der Katalog es nennt: die Fassung und die Argumente.
struct Tool {
    version: String,
    args: Vec<String>,
}

/// Eine Abweichung: Befund und Teil, die Zahl, die Regel und der Grund.
struct Deviation {
    count: usize,
    rule: String,
    reason: String,
}

struct Catalog {
    tidy: Tool,
    cppcheck: Tool,
    /// Die ausgeschalteten Pruefungen von clang-tidy mit ihrer Regel.
    off: BTreeMap<String, String>,
    deviations: BTreeMap<(String, String), Deviation>,
}

fn catalog() -> Catalog {
    let text = section();
    let tools = table(&text, "| Werkzeug | Fassung | Aufruf |");
    let tool = |name: &str| {
        let row = tools.iter().find(|r| r[0] == name).unwrap_or_else(|| panic!("Zeile `{name}` der Werkzeuge"));
        Tool { version: row[1].clone(), args: ticked(&row[2]) }
    };
    let off = table(&text, "| Ausgeschaltet | Regel | Grund |")
        .into_iter()
        .map(|r| (ticked(&r[0]).concat(), r[1].clone()))
        .collect();
    let mut deviations = BTreeMap::new();
    for row in table(&text, "| Befund | Teil | Zahl | Regel | Grund |") {
        let key = (ticked(&row[0]).concat(), row[1].clone());
        let count = row[2].parse().unwrap_or_else(|_| panic!("Zahl `{}` in {key:?}", row[2]));
        let deviation = Deviation { count, rule: row[3].clone(), reason: row[4].clone() };
        assert!(deviations.insert(key.clone(), deviation).is_none(), "{key:?} steht zweimal im Katalog");
    }
    Catalog { tidy: tool("clang-tidy"), cppcheck: tool("cppcheck"), off, deviations }
}

/// Eine Regel, wie der Katalog sie nennt: `MISRA C:2012 15.5 (A)`,
/// `MISRA C:2012 D.4.6 (A)` oder `CERT C ERR33-C`. Die Kategorie nach MISRA:
/// `M` Mandatory, `R` Required, `A` Advisory.
fn rule_ok(rule: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = rule.strip_prefix("MISRA C:2012 ") {
        let Some((id, category)) = rest.split_once(' ') else { return false };
        let id = id.strip_prefix("D.").unwrap_or(id);
        let parts: Vec<&str> = id.split('.').collect();
        return parts.len() == 2 && parts.iter().all(|p| number(p)) && matches!(category, "(M)" | "(R)" | "(A)");
    }
    if let Some(id) = rule.strip_prefix("CERT C ").and_then(|r| r.strip_suffix("-C")) {
        return id.len() == 5 && id[..3].bytes().all(|b| b.is_ascii_uppercase()) && number(&id[3..]);
    }
    false
}

// --- Die Werkzeuge ----------------------------------------------------------

/// `clang-tidy`: `TAKT_CLANG_TIDY`, sonst neben clang.
fn clang_tidy() -> Option<PathBuf> {
    let found = std::env::var_os("TAKT_CLANG_TIDY").map(PathBuf::from).or_else(|| {
        takt_llvm::toolchain::find().path().map(|clang| clang.with_file_name("clang-tidy")).filter(|p| p.is_file())
    });
    takt_testkit::require("clang-tidy", found, "`TAKT_CLANG_TIDY` setzen oder LLVM mit clang-tidy installieren")
}

/// `cppcheck`: `TAKT_CPPCHECK`, sonst im Pfad.
fn cppcheck() -> Option<PathBuf> {
    let found = std::env::var_os("TAKT_CPPCHECK").map(PathBuf::from).or_else(|| {
        Command::new("cppcheck").arg("--version").output().is_ok_and(|o| o.status.success()).then(|| "cppcheck".into())
    });
    takt_testkit::require("cppcheck", found, "`TAKT_CPPCHECK` setzen oder cppcheck installieren")
}

/// Die Zahlen haengen an der Fassung: Eine andere meldet anders.
fn check_version(tool: &Path, want: &str) {
    let out = Command::new(tool).arg("--version").output().expect("Werkzeug startet");
    let text = String::from_utf8_lossy(&out.stdout);
    let version = want.rsplit(' ').next().unwrap_or(want);
    assert!(text.contains(version), "{}: Fassung {want} verlangt (plan/cert.md), da ist\n{text}", tool.display());
}

/// Ein Befund: die Kennung des Werkzeugs (`cert-err33-c`,
/// `misra-c2012-15.5`), der Teil und die Stelle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Finding {
    id: String,
    part: &'static str,
    at: String,
}

/// Was zusammen gebunden wird: die Quellen eines Programms des Wirts und
/// ihre Kopfpfade. `cppcheck` prueft die Regeln ueber Dateien hinweg (5.x,
/// 8.x) je Bindung.
struct Link {
    units: Vec<PathBuf>,
    include: Vec<PathBuf>,
}

/// Der Teil des C-Teils, dem eine Datei gehoert; `out` ist das Verzeichnis
/// der erzeugten Lieferformen.
fn part(path: &str, out: &Path) -> &'static str {
    if path.ends_with("/takt_sdk.h") {
        "SDK"
    } else if path.contains("/ports/") {
        "Port"
    } else if path.contains("/examples/") {
        "Beispiele"
    } else if path.starts_with(&*out.to_string_lossy()) {
        "Rahmen"
    } else {
        "fremd"
    }
}

/// Ein Befund aus einer Zeile `DATEI:ZEILE:SPALTE: …`; der Kopf des SDK
/// steht in jeder Lieferform und im Crate, er zaehlt einmal je Stelle.
fn finding(id: &str, file: &str, line: &str, column: &str, out: &Path) -> Finding {
    let part = part(file, out);
    let file = if part == "SDK" { "takt_sdk.h" } else { file };
    Finding { id: id.to_string(), part, at: format!("{file}:{line}:{column}") }
}

fn includes(link: &Link) -> Vec<String> {
    link.include.iter().map(|d| format!("-I{}", d.display())).collect()
}

/// `clang-tidy` ueber jede Quelle der Bindung, mit ihren Kopfdateien.
fn run_tidy(tool: &Path, args: &[String], link: &Link, out: &Path) -> Vec<Finding> {
    let mut found = Vec::new();
    for unit in &link.units {
        let run = Command::new(tool)
            .args(args)
            .args(["--quiet", "--header-filter=.*"])
            .arg(unit)
            .args(["--", "-std=c11"])
            .args(includes(link))
            .output()
            .expect("clang-tidy startet");
        for line in String::from_utf8_lossy(&run.stdout).lines() {
            let Some((place, rest)) = line.split_once(": warning: ").or_else(|| line.split_once(": error: ")) else {
                continue;
            };
            let ids = rest.rsplit_once(" [").map_or("clang-diagnostic-error", |(_, id)| id.trim_end_matches(']'));
            // Eine Stelle mit mehreren Namen (Aliase einer Pruefung) zaehlt
            // unter dem aus CERT C, sonst unter dem ersten.
            let cert = ids.split(',').find(|n| n.starts_with("cert-") && n.ends_with("-c"));
            let id = cert.or_else(|| ids.split(',').next()).unwrap_or(ids);
            let mut at = place.rsplitn(3, ':');
            let (Some(column), Some(line), Some(file)) = (at.next(), at.next(), at.next()) else { continue };
            found.push(finding(id, file, line, column, out));
        }
    }
    found
}

/// `cppcheck` mit dem MISRA-Addon ueber die ganze Bindung.
fn run_cppcheck(tool: &Path, args: &[String], link: &Link, out: &Path, report: &Path) -> Vec<Finding> {
    let args: Vec<String> = args
        .iter()
        .map(|a| match a.strip_prefix("--library=tools/") {
            Some(rest) => format!("--library={}", root().join("tools").join(rest).display()),
            None => a.clone(),
        })
        .collect();
    let run = Command::new(tool)
        .args(&args)
        .args(["--std=c11", "--language=c", "--platform=unix64", "--quiet"])
        .arg("--template={file}:{line}:{column}: {severity}: {id}: {message}")
        .arg(format!("--output-file={}", report.display()))
        .args(includes(link))
        .args(&link.units)
        .output()
        .expect("cppcheck startet");
    assert!(run.status.success(), "cppcheck: {}", String::from_utf8_lossy(&run.stderr));
    let mut found = Vec::new();
    for line in read(report).lines() {
        let mut fields = line.splitn(6, ": ");
        let (Some(place), Some(_severity), Some(id)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let mut at = place.rsplitn(3, ':');
        let (Some(column), Some(line), Some(file)) = (at.next(), at.next(), at.next()) else { continue };
        found.push(finding(id, file, line, column, out));
    }
    found
}

/// Beide Werkzeuge ueber alle Bindungen; jede Stelle zaehlt einmal.
fn analyse(catalog: &Catalog, links: &[Link], out: &Path) -> Option<BTreeSet<Finding>> {
    let (tidy, cpp) = (clang_tidy()?, cppcheck()?);
    check_version(&tidy, &catalog.tidy.version);
    check_version(&cpp, &catalog.cppcheck.version);
    let mut found = BTreeSet::new();
    for (i, link) in links.iter().enumerate() {
        found.extend(run_tidy(&tidy, &catalog.tidy.args, link, out));
        let report = out.join(format!("cppcheck-{i}.txt"));
        found.extend(run_cppcheck(&cpp, &catalog.cppcheck.args, link, out, &report));
    }
    Some(found)
}

/// Die Befunde gegen den Katalog: je Befund und Teil genau die Zahl der
/// Abweichung. Der Fehler nennt die Zeilen, die der Katalog braeuchte, und
/// die Stellen jedes Befunds, dessen Zahl nicht stimmt.
fn judge(found: &BTreeSet<Finding>, catalog: &Catalog) -> Result<(), String> {
    let mut counted: BTreeMap<(String, String), usize> = BTreeMap::new();
    for f in found {
        *counted.entry((f.id.clone(), f.part.to_string())).or_default() += 1;
    }
    let mut problems = Vec::new();
    for ((id, part), n) in &counted {
        let at = || -> Vec<&str> {
            found.iter().filter(|f| f.id == *id && f.part == part).take(8).map(|f| f.at.as_str()).collect()
        };
        match catalog.deviations.get(&(id.clone(), part.clone())) {
            Some(d) if d.count == *n => {}
            Some(d) => {
                problems.push(format!(
                    "| `{id}` | {part} | {n} | {} | … | (Katalog: {}), an {:?}",
                    d.rule,
                    d.count,
                    at()
                ));
            }
            None => problems.push(format!("| `{id}` | {part} | {n} | ? | … | neu, an {:?}", at())),
        }
    }
    for ((id, part), d) in &catalog.deviations {
        if !counted.contains_key(&(id.clone(), part.clone())) {
            problems.push(format!("| `{id}` | {part} | 0 | {} | … | behoben: die Zeile faellt weg", d.rule));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

// --- Die Lieferformen -------------------------------------------------------

/// Die Programme, deren Rahmen geprueft wird: Praefix, Quelle, Schalter.
/// `valve` und `port_test` sind die der Beispiele; `streams` bringt die
/// Stroeme, Records und Treiber des Randes dazu.
const PROGRAMS: &[(&str, &str, &[&str])] = &[
    ("valve", "examples/c-host/takt/valve.takt", &["--form", "logical"]),
    ("port_test", "corpus-try/124_port_test.takt", &["--form", "rtos", "--hardware", "corpus-try/hw/linux-x86_64.hw"]),
    ("streams", "corpus-try/05_streams_and_protocol.takt", &["--form", "logical"]),
];

/// Schreibt die Lieferformen nach `out` und nennt die Bindungen: je
/// Programm sein Rahmen, bei `valve` mit `examples/c-host`, bei `port_test`
/// mit dem Port fuer Faeden, seinem POSIX-Adapter und `examples/posix`.
fn deliveries(out: &Path) -> Vec<Link> {
    let root = root();
    let ports = root.join("crates/takt-embed-c/ports");
    let mut links = Vec::new();
    for &(prefix, source, extra) in PROGRAMS {
        let dir = out.join(prefix);
        let target = ["--target", "x86_64-unknown-linux-gnu", "--prefix", prefix, "--out", dir.to_str().expect("Pfad")];
        let run = takt(&[&["build", source, "--emit", "embed"][..], extra, &target[..]].concat());
        assert!(run.status.success(), "{source}: {}", String::from_utf8_lossy(&run.stderr));
        let mut units = vec![dir.join(format!("{prefix}_frame.c")), dir.join(format!("{prefix}_ops.c"))];
        let mut include = vec![dir.clone()];
        match prefix {
            "valve" => units.push(root.join("examples/c-host/main.c")),
            "port_test" => {
                units.extend([
                    ports.join("rtos/takt_rtos.c"),
                    ports.join("posix/takt_os.c"),
                    root.join("examples/posix/main.c"),
                ]);
                include.extend([ports.join("rtos"), ports.join("posix")]);
            }
            _ => {}
        }
        links.push(Link { units, include });
    }
    links
}

// --- Die Tests --------------------------------------------------------------

/// **Der C-Teil weicht nur ab, wie der Katalog sagt** (FB-524): Jeder
/// Befund beider Werkzeuge steht mit Teil und genauer Zahl als Abweichung
/// in plan/cert.md, und jede Abweichung des Katalogs tritt so oft auf.
#[test]
#[ignore = "clang-tidy und cppcheck: TAKT_CLANG_TIDY, TAKT_CPPCHECK; auf CI in analysis.yml"]
fn the_c_part_deviates_only_as_the_catalog_says() {
    let catalog = catalog();
    let out = scratch("takt-c-analysis");
    let links = deliveries(&out);
    let Some(found) = analyse(&catalog, &links, &out) else { return };
    let strangers: Vec<&Finding> = found.iter().filter(|f| f.part == "fremd").collect();
    assert!(strangers.is_empty(), "Befunde ausserhalb des C-Teils: {strangers:?}");
    if let Err(rows) = judge(&found, &catalog) {
        panic!("Befunde gegen den Katalog (plan/cert.md, „Der C-Teil“); die Zeilen, wie sie stehen muessten:\n{rows}");
    }
}

/// **Das Netz faengt**: Ein absichtlich eingebauter Verstoss gegen CERT C
/// ERR34-C — `atoi` meldet keinen Fehler — in einer Datei der Beispiele
/// fuellt beide Werkzeuge (`cert-err34-c`, MISRA C:2012 21.7), und der
/// Vergleich mit dem Katalog scheitert an ihm.
#[test]
#[ignore = "clang-tidy und cppcheck: TAKT_CLANG_TIDY, TAKT_CPPCHECK; auf CI in analysis.yml"]
fn a_planted_cert_c_violation_is_caught() {
    let catalog = catalog();
    let out = scratch("takt-c-planted");
    let dir = out.join("examples");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let planted = dir.join("planted.c");
    let text = "#include <stdlib.h>\n\nint takt_planted(const char *text);\n\n\
                int takt_planted(const char *text) { return atoi(text); }\n";
    std::fs::write(&planted, text).expect("Datei");
    let link = Link { units: vec![planted], include: Vec::new() };
    let Some(found) = analyse(&catalog, &[link], &out) else { return };
    let ids: BTreeSet<&str> = found.iter().filter(|f| f.part == "Beispiele").map(|f| f.id.as_str()).collect();
    assert!(ids.contains("cert-err34-c"), "clang-tidy fand den Verstoss nicht: {found:?}");
    assert!(ids.contains("misra-c2012-21.7"), "cppcheck fand den Verstoss nicht: {found:?}");
    let verdict = judge(&found, &catalog).expect_err("der Katalog laesst den Verstoss nicht zu");
    assert!(verdict.contains("| `cert-err34-c` | Beispiele | 1 |"), "{verdict}");
}

/// Der Katalog selbst, ohne Werkzeuge: Jede Abweichung nennt eine Regel
/// aus MISRA C:2012 oder CERT C, ein Befund des MISRA-Addons seine eigene;
/// von einer Regel der Kategorie Mandatory weicht nichts ab, dort steht nur
/// eine Fehlmeldung des Werkzeugs. Jede ausgeschaltete Pruefung steht im
/// Aufruf eines Werkzeugs (`-pruefung` bei clang-tidy, `--suppress=` bei
/// cppcheck) und umgekehrt.
#[test]
fn every_deviation_of_the_c_part_names_its_rule() {
    let catalog = catalog();
    assert!(!catalog.deviations.is_empty());
    for ((id, part), d) in &catalog.deviations {
        assert!(rule_ok(&d.rule), "{id} ({part}): `{}` ist keine Regel aus MISRA C:2012 oder CERT C", d.rule);
        if let Some(number) = id.strip_prefix("misra-c2012-") {
            assert!(d.rule.starts_with(&format!("MISRA C:2012 {number} (")), "{id}: die Regel ist `{}`", d.rule);
        }
        if d.rule.ends_with("(M)") {
            assert!(
                d.reason.starts_with("Fehlmeldung"),
                "{id}: von einer Regel der Kategorie Mandatory weicht nichts ab"
            );
        }
        assert!(["Rahmen", "SDK", "Port", "Beispiele"].contains(&part.as_str()), "{id}: Teil `{part}`");
        assert!(d.count > 0 && !d.reason.is_empty(), "{id} ({part}): Zahl und Grund");
    }
    let checks = catalog.tidy.args.iter().find_map(|a| a.strip_prefix("--checks=")).expect("--checks=");
    let suppressed = catalog.cppcheck.args.iter().filter_map(|a| a.strip_prefix("--suppress="));
    let off: BTreeSet<&str> =
        checks.split(',').filter_map(|c| c.strip_prefix('-')).filter(|c| *c != "*").chain(suppressed).collect();
    assert_eq!(off, catalog.off.keys().map(String::as_str).collect(), "ausgeschaltet im Aufruf und in der Tabelle");
    for (check, rule) in &catalog.off {
        assert!(rule == "—" || rule_ok(rule), "{check}: `{rule}`");
    }
    assert!(rule_ok("MISRA C:2012 D.4.6 (A)") && rule_ok("CERT C ERR33-C") && !rule_ok("MISRA C:2012 15.5"));
}
