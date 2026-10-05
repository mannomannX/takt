//! Pruefungen 40 und 41 nach dem Kern des Bauziels (12.8, FB-384).
//!
//! Ein Programm ohne `system: target` wird fuer die Form der Einbindung
//! gebaut; der Kern steht dann nur im Bau (`--target`), nicht im Programm.

use takt_sema::{Core, Options, compile};

/// `float = f64` (Default) und ein `int` ohne Range in einer Schleife,
/// ohne `system: target`.
const SRC: &str = "\
system:
    language = 1
    tick     = 10 ms

output total : int @ sim(\"t\")

machine m:
    var acc : int = 0
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                acc = acc + i
            total = acc
";

fn codes(core: Option<Core>) -> Vec<String> {
    let options = Options { core, ..Default::default() };
    let out = compile(SRC, &options);
    let mut codes: Vec<String> =
        out.diagnostics.iter().map(|d| d.code.to_string()).filter(|c| c == "SC-40" || c == "SC-41").collect();
    codes.sort();
    codes
}

#[test]
fn a_narrow_core_without_f64_hardware_gets_both_lints() {
    let core = Core { word_bits: 32, f64_hardware: false };
    assert_eq!(codes(Some(core)), ["SC-40", "SC-41"]);
}

#[test]
fn a_32_bit_core_with_f64_hardware_gets_only_the_width_lint() {
    let core = Core { word_bits: 32, f64_hardware: true };
    assert_eq!(codes(Some(core)), ["SC-40"]);
}

#[test]
fn a_64_bit_core_gets_no_lint() {
    let core = Core { word_bits: 64, f64_hardware: true };
    assert!(codes(Some(core)).is_empty());
}

#[test]
fn without_a_core_and_without_a_profile_no_lint_is_guessed() {
    assert!(codes(None).is_empty());
}
