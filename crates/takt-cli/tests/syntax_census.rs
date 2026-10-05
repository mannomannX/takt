//! Die Abdeckung der Syntax, die die MIR nicht sieht (13.2, KOR-006).
//!
//! `takt_syntax::census` zaehlt die Formen, die die Sema entzuckert oder
//! aufloest: `pulse`, `x += 1`, `case a..b`, Konstanten, Einheiten. Jede
//! Form braucht ein Programm unter `corpus-try` oder `crates`, das
//! fehlerfrei uebersetzt, oder einen benannten Grund in [`GAPS`]. Die Liste
//! schrumpft nur: Bekommt eine Form dort ein Programm, scheitert der Test,
//! bis ihr Eintrag gestrichen ist. Eine neue Variante des Syntaxbaums
//! uebersetzt erst, wenn `census.rs` sie einordnet.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use takt_syntax::census::{Sugar, census};

/// Formen ohne Programm, mit Grund.
const GAPS: &[(Sugar, &str)] = &[(
    Sugar::ImportModule,
    "die Sema lehnt jedes `import` eines Moduls ab (takt-sema/src/collect.rs: Module werden noch nicht \
         unterstuetzt); nur in corpus-try/10_nuances.takt",
)];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("Verzeichniseintrag").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|x| x == "takt") {
            out.push(path);
        }
    }
}

/// Je Form die Programme, die sie benutzen und fehlerfrei uebersetzen.
fn covered() -> BTreeMap<Sugar, Vec<String>> {
    let root = workspace();
    let mut files = Vec::new();
    for dir in ["corpus-try", "crates"] {
        collect(&root.join(dir), &mut files);
    }
    files.retain(|f| !f.ends_with("takt-sema/src/prelude.takt"));
    files.sort();
    assert!(files.len() >= 300, "nur {} Programme", files.len());
    let mut out: BTreeMap<Sugar, Vec<String>> = BTreeMap::new();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("lesbar");
        let dir = path.parent().expect("Verzeichnis");
        let channel_imports = takt_sema::channel_imports(&src)
            .into_iter()
            .filter_map(|name| std::fs::read_to_string(dir.join(&name)).ok().map(|text| (name, text)))
            .collect();
        let options = takt_sema::Options { build: takt_sema::Build::Sim, channel_imports, ..Default::default() };
        if takt_sema::compile(&src, &options).diagnostics.iter().any(|d| d.is_error()) {
            continue;
        }
        let file = takt_syntax::parse_file(&takt_syntax::tokenize(&src)).0;
        let name = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        for form in census(&file) {
            out.entry(form).or_default().push(name.clone());
        }
    }
    out
}

/// **Jede Form hat ein Programm oder einen Grund, und die Liste der Gruende
/// schrumpft nur.**
#[test]
fn every_desugared_form_has_a_program_or_a_reason() {
    // Die Sema rekursiert; der Stapel ist bemessen wie in der CLI (2.1).
    let covered = takt_syntax::parser::with_stack(32 << 20, covered);
    let gaps: BTreeMap<Sugar, &str> = GAPS.iter().copied().collect();
    assert_eq!(gaps.len(), GAPS.len(), "eine Form steht doppelt in GAPS");
    let mut wrong = Vec::new();
    for form in Sugar::ALL {
        match (covered.get(&form), gaps.get(&form)) {
            (Some(_), None) => {}
            (None, Some(why)) => assert!(!why.trim().is_empty(), "{form:?}: Grund fehlt"),
            (None, None) => wrong.push(format!("{form:?}: kein Programm und kein Grund")),
            (Some(files), Some(_)) => {
                wrong.push(format!("{form:?}: hat ein Programm ({}), Eintrag streichen", files[0]))
            }
        }
    }
    let unused: BTreeSet<_> = gaps.keys().filter(|g| !Sugar::ALL.contains(g)).collect();
    assert!(unused.is_empty(), "{unused:?}");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
