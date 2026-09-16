//! Operator-Metadaten (2.5, v1.1): `label`, `display`, `group`, `doc` an
//! Channels, Params, Tunables, Commands, Maschinen und Zustaenden — reine
//! Beobachtung, im Programm-Hash, nicht im Logik-Hash.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::hash::{logic_hash, program_hash};
use takt_sema::{Build, Options};

fn compile(src: &str) -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

const WITH_META: &str = "system:
    language = 1
    tick = 10 ms

input  p : float[V] in 0..100 V @ hw(\"daq/ai0\") with max_age = 50 ms, label = \"Spannung\", display = V, group = \"Zelle\", doc = \"Zellspannung\"
output v : bool @ hw(\"gpio/v\") with safe = false, label = \"Ventil\"
param LIMIT : float[V] = 80 V with label = \"Grenze\", display = V
tunable param GAIN : int in 0..9 = 1 with label = \"Gain\", group = \"Regler\"
command start with label = \"Start\", doc = \"beginnt\"

machine m with label = \"Steuerung\", group = \"Zelle\", doc = \"regelt\":
    initial IDLE

    state IDLE with label = \"Ruhe\":
        when p.or(0 V) > LIMIT: -> OPEN

    state OPEN with doc = \"offen\":
        enter:
            v = true
        after 100 ms: -> IDLE
";

/// Dasselbe Programm ohne Metadaten.
const WITHOUT_META: &str = "system:
    language = 1
    tick = 10 ms

input  p : float[V] in 0..100 V @ hw(\"daq/ai0\") with max_age = 50 ms
output v : bool @ hw(\"gpio/v\") with safe = false
param LIMIT : float[V] = 80 V
tunable param GAIN : int in 0..9 = 1
command start

machine m:
    initial IDLE

    state IDLE:
        when p.or(0 V) > LIMIT: -> OPEN

    state OPEN:
        enter:
            v = true
        after 100 ms: -> IDLE
";

#[test]
fn metadata_lands_on_all_six_carriers() {
    let p = compile(WITH_META);
    let channel = |name: &str| p.channels.iter().find(|c| c.name == name).expect(name);
    let meta = &channel("p").meta;
    assert_eq!(meta.label.as_deref(), Some("Spannung"));
    assert!(meta.display.is_some());
    assert_eq!(meta.group.as_deref(), Some("Zelle"));
    assert_eq!(meta.doc.as_deref(), Some("Zellspannung"));
    assert_eq!(channel("v").meta.label.as_deref(), Some("Ventil"));
    let param = |name: &str| p.params.iter().find(|q| q.name == name).expect(name);
    assert_eq!(param("LIMIT").meta.label.as_deref(), Some("Grenze"));
    assert!(param("LIMIT").meta.display.is_some());
    assert_eq!(param("GAIN").meta.group.as_deref(), Some("Regler"));
    assert_eq!(p.commands[0].meta.doc.as_deref(), Some("beginnt"));
    let m = &p.machines[0];
    assert_eq!((m.meta.label.as_deref(), m.meta.group.as_deref()), (Some("Steuerung"), Some("Zelle")));
    let state = |name: &str| m.states.iter().find(|s| s.name == name).expect(name);
    assert_eq!(state("IDLE").meta.label.as_deref(), Some("Ruhe"));
    assert_eq!(state("OPEN").meta.doc.as_deref(), Some("offen"));
}

#[test]
fn metadata_changes_the_program_hash_but_not_the_logic_hash() {
    let (with, without) = (compile(WITH_META), compile(WITHOUT_META));
    assert_eq!(logic_hash(&with), logic_hash(&without));
    assert_ne!(program_hash(&with), program_hash(&without));
}
