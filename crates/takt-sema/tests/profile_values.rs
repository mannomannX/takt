//! Werte eines Profils und Defaults eines `param` (8.4): Ein gefalteter
//! konstanter Ausdruck liegt in der Range des Parameters, sonst SC-3 in der
//! Sema — nicht erst beim Laden (KOR-011).

use takt_diag::Policy;
use takt_sema::{Build, Options};

fn errors(decls: &str) -> Vec<String> {
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms

const BASE : int = 3
{decls}
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            n = P
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(&src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// Ein Profilwert aus einem konstanten Ausdruck: in der Range erlaubt,
/// ausserhalb genau ein SC-3.
#[test]
fn a_folded_profile_value_outside_the_range_is_check_3() {
    let ok = errors("param P : int in 0..5 = 1\nprofile FAST:\n    P = BASE + 2\n");
    assert!(ok.is_empty(), "{ok:?}");
    let e = errors("param P : int in 0..5 = 1\nprofile FAST:\n    P = BASE + 4\n");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("SC-3") && e[0].contains("Range"), "{e:?}");
}

/// Ebenso der Default eines `param`.
#[test]
fn a_folded_param_default_outside_the_range_is_check_3() {
    assert!(errors("param P : int in 0..5 = BASE + 2\n").is_empty());
    let e = errors("param P : int in 0..5 = BASE * 3\n");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("SC-3") && e[0].contains("Range"), "{e:?}");
}
