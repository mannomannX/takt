//! Geplante Ausgaben (7.5, Pruefung 21): `at`- und `pulse`-Bloecke weisen
//! nur skalare Outputs zu — Bool, Zahl, Enum (5.5).

use takt_diag::Policy;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";

/// Die Fehler eines Programms mit `stmt` im `enter:` eines Zustands.
fn errors(stmt: &str) -> Vec<String> {
    let src = format!(
        "{HEAD}output cells : [2] bool @ hw(\"o/cells[0:2]\") with safe = [false, false]
output flag  : bool     @ hw(\"o/flag\") with safe = false
output level : int in 0..9 @ hw(\"o/level\") with safe = 0

machine m:
    initial RUN
    state RUN:
        enter:
{stmt}            flag = true
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(&src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// Skalare Outputs gehen in `at` und `pulse`.
#[test]
fn scalar_outputs_may_be_scheduled() {
    for stmt in
        ["            at now + 10 ms:\n                level = 3\n", "            pulse flag = true for 20 ms\n"]
    {
        assert!(errors(stmt).is_empty(), "{stmt}{:?}", errors(stmt));
    }
}

/// 5.5, Tabelle 10 Zeile 21: Ein Array-Output ist nicht skalar; `at` und
/// `pulse` auf ihm sind ein Fehler der Pruefung 21.
#[test]
fn an_array_output_cannot_be_scheduled() {
    for stmt in [
        "            at now + 10 ms:\n                cells = [true, true]\n",
        "            pulse cells = [true, false] for 20 ms\n",
    ] {
        let e = errors(stmt);
        assert_eq!(e.len(), 1, "{stmt}{e:?}");
        assert!(e[0].contains("SC-21"), "{stmt}{e:?}");
    }
}
