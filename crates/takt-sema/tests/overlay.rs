//! Overlay und Spitzenlast mit gescopten Instanzen (Referenz 11.5,
//! 9.4.3, 5.11): Exklusive Instanzen teilen Speicher und Rechenlast,
//! Latches nicht.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::analysis::{schedulability, size};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn ok(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Zwei Instanzen im selben Zustand, eine dritte im exklusiven.
const THREE: &str = r#"
input  mode : int in 0..2 @ sim("i/mode")

output a : bool @ hw("o/a") with safe = false
output b : bool @ hw("o/b") with safe = false
output c : bool @ hw("o/c") with safe = false

machine blip(o: output bool) every 1 ms:
    initial LOW

    state LOW:
        enter:
            o = false

        after 3 ms: -> HIGH

    state HIGH:
        enter:
            o = true

        after 3 ms: -> LOW

machine ctrl:
    initial FIRST

    state FIRST:
        instance p = blip(o = a)
        instance q = blip(o = b)

        when mode == 1: -> SECOND

    state SECOND:
        instance r = blip(o = c)

        when mode == 0: -> FIRST
"#;

/// Dasselbe Programm ohne die dritte Instanz — die Vergleichsgroesse.
const TWO: &str = r#"
input  mode : int in 0..2 @ sim("i/mode")

output a : bool @ hw("o/a") with safe = false
output b : bool @ hw("o/b") with safe = false

machine blip(o: output bool) every 1 ms:
    initial LOW

    state LOW:
        enter:
            o = false

        after 3 ms: -> HIGH

    state HIGH:
        enter:
            o = true

        after 3 ms: -> LOW

machine ctrl:
    initial FIRST

    state FIRST:
        instance p = blip(o = a)
        instance q = blip(o = b)

        when mode == 1: -> SECOND

    state SECOND:
        when mode == 0: -> FIRST
"#;

fn machine_states(p: &Program) -> u64 {
    size::size(p).items.iter().find(|i| i.name.starts_with("Maschinenzustaende")).expect("Posten").bytes
}

/// 11.5: Die dritte Instanz liegt im exklusiven Zustand und kostet
/// nichts zusaetzlich — sie teilt den Platz mit den beiden anderen.
#[test]
fn exclusive_instances_share_memory() {
    let three = machine_states(&ok(THREE));
    let two = machine_states(&ok(TWO));
    assert_eq!(three, two, "die exklusive dritte Instanz haette Platz kosten duerfen");
}

/// 11.5: Der Report nennt, was das Overlay spart, und die Ersparnis ist
/// genau die eine ueberlagerte Instanz.
#[test]
fn the_report_names_what_the_overlay_saves() {
    let p = ok(THREE);
    let s = size::size(&p);
    assert!(s.overlay_saved > 0, "keine Ersparnis gemeldet");
    let lines = s.lines().join("\n");
    assert!(lines.contains("Overlay:") && lines.contains("gespart"), "{lines}");
    // Gespart wird genau eine Instanz: Die flache Summe rechnete drei,
    // das Overlay zwei.
    assert!(s.overlay_saved < machine_states(&p), "die Ersparnis ist groesser als der Posten selbst");
}

/// 9.4.3: Die Spitzenlast ist das Maximum ueber die Konfigurationen,
/// nicht die Summe aller Maschinen.
#[test]
fn the_budget_is_the_peak_over_configurations_not_the_sum() {
    let three = schedulability::load(&ok(THREE));
    let two = schedulability::load(&ok(TWO));
    // Die dritte Instanz liegt exklusiv: Ihre Rechenlast hebt die Spitze
    // nicht. Was bleibt, ist ihr Ein- und Ausschalten, wenn `ctrl` den
    // Zustand wechselt (9.3 Schritte 2 und 3): ein Speicherzugriff im Tick
    // des Besitzers, kein Tick der Instanz.
    assert_eq!(
        takt_mir::fns::CostVec { mem: two.peak.mem, ..three.peak },
        two.peak,
        "die exklusive Instanz zaehlte in die Summe"
    );
    assert_eq!(three.peak.mem, two.peak.mem + 1, "das Schalten der dritten Instanz");
}

/// 11.5: `persist` liegt in Sigma und wird nie ueberlagert — der Posten
/// des Journals haengt nicht am Zustandsbaum.
#[test]
fn latches_are_never_overlaid() {
    let p = ok(r#"
input  mode : int in 0..2 @ sim("i/mode")
output a : bool @ hw("o/a") with safe = false

machine keeper(o: output bool) every 1 ms:
    persist var seen : int in 0..99 = 0

    initial RUN

    state RUN:
        loop:
            seen = 1
            o = true

machine ctrl:
    initial FIRST

    state FIRST:
        instance k = keeper(o = a)

        when mode == 1: -> SECOND

    state SECOND:
        loop:
            pass

        when mode == 0: -> FIRST
"#);
    let s = size::size(&p);
    let journal: u64 = s.items.iter().filter(|i| i.name.contains("Journal")).map(|i| i.bytes).sum();
    assert!(journal > 0, "das Journal fehlt im Report: {:?}", s.items.iter().map(|i| &i.name).collect::<Vec<_>>());
}
