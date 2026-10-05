//! Die Safe-State-Latenz (9.4.5) rechnet aus Perioden, Bestaetigungszeiten
//! und dem Fault-Wald.

use takt_mir::analysis::latency::{SiteKind, latency};
use takt_mir::sample::full_program;

#[test]
fn the_sample_program_has_a_bound() {
    let p = full_program();
    let l = latency(&p);
    assert!(!l.sites.is_empty(), "das Beispielprogramm hat Fault-Stellen");
    assert!(l.worst_case() > 0, "und damit eine Schranke");
    assert_eq!(l.tick_ns, p.config.tick, "der Basis-Tick kommt aus dem Programm");
}

#[test]
fn every_site_is_at_least_one_period() {
    let p = full_program();
    for s in &latency(&p).sites {
        assert!(s.detect >= 1, "eine Stelle wird fruehestens in der naechsten Aktivierung gesehen: {s:?}");
        assert!(s.fault >= 1, "der Weg zum Fault-Ziel kostet mindestens einen Tick: {s:?}");
    }
}

/// SYN-027: Das Beispielprogramm traegt jede Art von Fault-Stelle — `check`
/// im `loop:`, `expect` hinter einem `match` und `abort` in `exit:` —, und
/// die Analyse findet jede.
#[test]
fn every_site_kind_is_recognised() {
    let p = full_program();
    let l = latency(&p);
    let kinds: Vec<SiteKind> = l.sites.iter().map(|s| s.kind).collect();
    for kind in [SiteKind::Check, SiteKind::Expect, SiteKind::Abort] {
        assert!(kinds.contains(&kind), "{} ist eine Fault-Stelle: {kinds:?}", kind.name());
    }
}
