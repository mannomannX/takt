//! Argumente gegen Parameter (FB-407): Ein Argument mit falschem Typ ist
//! ein Fehler, aber kein fehlendes Argument — eine Ursache, eine Meldung.

use takt_diag::Policy;
use takt_sema::{Build, Options};

fn errors(body: &str) -> Vec<String> {
    let src = format!(
        "system:
    language = 1
    tick     = 10 ms

const BASE : int = 4096

output flash_cmd : FlashCmd @ hw(\"flash/cmd\") with safe = NONE
output n         : u32      @ hw(\"o/n\")       with safe = 0

fn twice(x: u32) -> u32:
    return x * 2

machine m:
    initial RUN
    state RUN:
        loop:
{body}"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    takt_sema::compile(&src, &options).diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect()
}

/// `READ(addr = BASE, ...)`: das Feld `addr` ist `u32`, `BASE` ein `int`.
#[test]
fn a_named_variant_field_of_the_wrong_type_is_one_error() {
    let e = errors("            flash_cmd = READ(addr = BASE, size = 16)\n");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("erwartet `u32`, gefunden `int`"), "{e:?}");
}

/// Dasselbe fuer ein benanntes und ein positionales Funktionsargument.
#[test]
fn a_function_argument_of_the_wrong_type_is_one_error() {
    for call in ["twice(x = BASE)", "twice(BASE)"] {
        let e = errors(&format!("            n = {call}\n"));
        assert_eq!(e.len(), 1, "{call}: {e:?}");
        assert!(e[0].contains("erwartet `u32`, gefunden `int`"), "{call}: {e:?}");
    }
}

/// Ein Argument, das wirklich fehlt, bleibt eine eigene Meldung.
#[test]
fn a_missing_argument_is_still_reported() {
    let e = errors("            flash_cmd = READ(addr = 0)\n");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("Argument `size` fehlt"), "{e:?}");
}
