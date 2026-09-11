//! Der defensive Treiberrand (12.6), Zeile fuer Zeile.
//!
//! Jeder Test nennt die Zeile der Tabelle, die er prueft. Die Tabelle ist
//! normativ; ein Test, der sie nicht trifft, prueft etwas anderes.

use takt_hal::driver::{Delivery, Driver, Reading, Writing};
use takt_hal::edge::{Alert, Contract, Edge};
use takt_hal::quality::{Gate, Limits, Quality, Reason, Scalar};
use takt_hal::sim::Sim;
use takt_mir::ChannelId;
use takt_mir::program::{Channel, ChannelAttrs, Direction, Program};

/// Ein Wert, wie ihn ein Treiber liefert.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Num(f64);

impl Scalar for Num {
    fn as_f64(&self) -> Option<f64> {
        Some(self.0)
    }

    fn as_i64(&self) -> Option<i64> {
        None
    }
}

/// Ein Channel mit den Attributen, die der jeweilige Test braucht.
fn channel(debounce: Option<u32>) -> Channel {
    Channel {
        dir: Direction::Input,
        name: "p".into(),
        ty: takt_mir::TypeId(0),
        binding: takt_mir::program::Binding::None,
        attrs: ChannelAttrs { debounce, ..ChannelAttrs::default() },
        meta: takt_mir::program::Meta::default(),
        owner: None,
        span: takt_diag::Span::new(0, 0),
    }
}

const SEC: i64 = 1_000_000_000;

// --- Zeile 3: Ranges ----------------------------------------------------

#[test]
fn a_value_outside_the_range_is_bad() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: None };
    assert_eq!(g.check(&Num(5.0), &c, 0, &limits).quality, Quality::Good);
    let v = g.check(&Num(11.0), &c, SEC, &limits);
    assert_eq!(v.quality, Quality::Bad);
    assert_eq!(v.reason, Some(Reason::OutOfRange));
}

/// 3.5: „nicht geklemmt und nicht zu einem Fault" — der Wert faellt weg,
/// er wird nicht auf die Grenze gezogen.
#[test]
fn an_out_of_range_value_is_not_clamped() {
    let mut g = Gate::default();
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: None };
    let v = g.check(&Num(11.0), &channel(None), 0, &limits);
    assert!(!v.held, "ein verworfener Wert haelt nichts");
}

// --- Zeile 4: max_slew --------------------------------------------------

#[test]
fn a_value_beyond_max_slew_is_implausible() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: None, max_slew: Some(50.0) };
    assert_eq!(g.check(&Num(0.0), &c, 0, &limits).quality, Quality::Good);
    // 60 Einheiten in einer Sekunde bei erlaubten 50.
    let v = g.check(&Num(60.0), &c, SEC, &limits);
    assert_eq!(v.quality, Quality::Bad);
    assert_eq!(v.reason, Some(Reason::Implausible));
}

#[test]
fn a_value_within_max_slew_stays_good() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: None, max_slew: Some(50.0) };
    g.check(&Num(0.0), &c, 0, &limits);
    assert_eq!(g.check(&Num(40.0), &c, SEC, &limits).quality, Quality::Good);
}

/// 3.5: „der erste Wert nach Start oder nach `Bad` gilt als gut".
#[test]
fn the_first_value_has_nothing_to_compare_against() {
    let mut g = Gate::default();
    let limits = Limits { range: None, max_slew: Some(1.0) };
    assert_eq!(g.check(&Num(1e9), &channel(None), 0, &limits).quality, Quality::Good);
}

#[test]
fn after_bad_the_next_value_is_good_again() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: None, max_slew: Some(50.0) };
    g.check(&Num(0.0), &c, 0, &limits);
    assert_eq!(g.check(&Num(60.0), &c, SEC, &limits).quality, Quality::Bad);
    // Ohne diese Regel bliebe der Kanal dauerhaft implausibel, weil er sich
    // weiter am alten Wert maesse.
    assert_eq!(g.check(&Num(61.0), &c, 2 * SEC, &limits).quality, Quality::Good);
}

/// Ohne Zeitdifferenz ist eine Rate nicht definiert; 3.5 laesst den Wert
/// dann durch, statt ihn auf Verdacht zu verwerfen.
#[test]
fn two_deliveries_at_the_same_instant_have_no_rate() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: None, max_slew: Some(1.0) };
    g.check(&Num(0.0), &c, SEC, &limits);
    assert_eq!(g.check(&Num(1000.0), &c, SEC, &limits).quality, Quality::Good);
}

// --- Zeile 3 und 4 mit debounce ----------------------------------------

/// 3.5: „fuer bis zu drei aufeinanderfolgende Lieferungen als `Suspect`
/// … erst danach `Bad`".
#[test]
fn debounce_holds_three_deliveries_then_gives_up() {
    let mut g = Gate::default();
    let c = channel(Some(3));
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: None };
    assert_eq!(g.check(&Num(5.0), &c, 0, &limits).quality, Quality::Good);
    for i in 1..=3 {
        let v = g.check(&Num(99.0), &c, i * SEC, &limits);
        assert_eq!(v.quality, Quality::Suspect, "Lieferung {i}");
        assert!(v.held, "der letzte gute Wert wird gehalten");
        assert_eq!(v.reason, Some(Reason::OutOfRange));
    }
    let v = g.check(&Num(99.0), &c, 4 * SEC, &limits);
    assert_eq!(v.quality, Quality::Bad, "nach `debounce` Lieferungen");
    assert!(!v.held);
}

/// Die Zaehlung gilt fuer *aufeinanderfolgende* Verletzungen: Ein guter
/// Wert dazwischen setzt sie zurueck.
#[test]
fn a_good_delivery_resets_the_debounce_count() {
    let mut g = Gate::default();
    let c = channel(Some(2));
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: None };
    g.check(&Num(5.0), &c, 0, &limits);
    assert_eq!(g.check(&Num(99.0), &c, SEC, &limits).quality, Quality::Suspect);
    assert_eq!(g.check(&Num(5.0), &c, 2 * SEC, &limits).quality, Quality::Good);
    assert_eq!(g.check(&Num(99.0), &c, 3 * SEC, &limits).quality, Quality::Suspect);
}

/// Ohne `debounce` ist eine Verletzung sofort `Bad` (Entscheidung 18).
#[test]
fn without_debounce_a_violation_is_immediate() {
    let mut g = Gate::default();
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: None };
    assert_eq!(g.check(&Num(99.0), &channel(None), 0, &limits).quality, Quality::Bad);
}

/// Range vor `max_slew`: Ein Wert weit ausserhalb verletzt fast immer
/// beides, und `OutOfRange` ist die praezisere Auskunft.
#[test]
fn out_of_range_wins_over_implausible() {
    let mut g = Gate::default();
    let c = channel(None);
    let limits = Limits { range: Some((0.0, 10.0)), max_slew: Some(1.0) };
    g.check(&Num(5.0), &c, 0, &limits);
    assert_eq!(g.check(&Num(1000.0), &c, SEC, &limits).reason, Some(Reason::OutOfRange));
}

// --- Der Rand als Ganzes ------------------------------------------------

/// Ein Programm mit einem Input und einem Output.
fn program() -> Program {
    let mut p = Program::new(takt_mir::program::Config::new(1, 1_000_000));
    p.channels.push(channel(None));
    p.channels.push(Channel { dir: Direction::Output, name: "o".into(), ..channel(None) });
    p
}

fn limits(n: usize) -> Vec<Limits> {
    vec![Limits::default(); n]
}

const IN: ChannelId = ChannelId(0);
const OUT: ChannelId = ChannelId(1);

/// Zeile 1: Ein Zeitstempel knapp neben dem Fenster wird geklemmt und
/// gezaehlt, nicht verworfen.
#[test]
fn a_timestamp_outside_the_window_is_clamped() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let r = [Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Good, t: 500 }];
    let out = e.validate("sim", &r, &[], (1_000, 2_000), &p);
    assert_eq!(e.time_warped, 1);
    assert!(matches!(out.alerts[0], Alert::TimeWarped { clamped: 1_001, .. }));
    assert_eq!(out.readings[0].1.quality, Quality::Good, "geklemmt, nicht verworfen");
}

/// Zeile 1, zweiter Fall: jenseits der Toleranz wie Zeile 2.
#[test]
fn a_timestamp_far_outside_degrades_the_driver() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 1_000);
    let r = [Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Good, t: -1_000_000 }];
    let out = e.validate("sim", &r, &[], (1_000, 2_000), &p);
    assert!(matches!(out.alerts[0], Alert::DriverDegraded { what: Contract::TimeWindow, .. }));
    assert_eq!(out.readings[0].1.reason, Some(Reason::Driver));
}

/// Zeile 2: `Bad` mit Wert ist ein Widerspruch in den Flags.
#[test]
fn bad_with_a_value_is_a_contract_violation() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let r = [Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Bad, t: 1_500 }];
    let out = e.validate("sim", &r, &[], (1_000, 2_000), &p);
    assert!(matches!(out.alerts[0], Alert::DriverDegraded { what: Contract::Flags, .. }));
}

/// Zeile 2: Erholung, sobald der Treiber wieder vertragsgemaess liefert.
#[test]
fn a_driver_recovers_when_it_delivers_correctly_again() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let bad = [Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Bad, t: 1_500 }];
    e.validate("sim", &bad, &[], (1_000, 2_000), &p);
    let good = [Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Good, t: 2_500 }];
    let out = e.validate("sim", &good, &[], (2_000, 3_000), &p);
    assert!(matches!(out.alerts[0], Alert::DriverRecovered { .. }));
    assert_eq!(out.readings[0].1.quality, Quality::Good);
}

/// Zeile 2: Ein Vertragsbruch trifft *alle* Kanaele des Treibers.
#[test]
fn a_contract_violation_degrades_every_channel_of_the_driver() {
    let mut p = program();
    p.channels.push(channel(None));
    let other = ChannelId(2);
    let mut e = Edge::new(&p, limits(p.channels.len()), 1_000);
    let r = [
        Reading { channel: IN, value: Some(Num(1.0)), quality: Quality::Good, t: 1_500 },
        Reading { channel: other, value: Some(Num(1.0)), quality: Quality::Good, t: -1_000_000 },
    ];
    let out = e.validate("sim", &r, &[], (1_000, 2_000), &p);
    assert_eq!(out.readings.len(), 2);
    assert!(out.readings.iter().all(|(_, v)| v.reason == Some(Reason::Driver)));
}

/// Zeile 6: Ein nicht bestaetigter Schreibvorgang faultet den Besitzer.
#[test]
fn an_unconfirmed_write_faults_the_owner() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let sim: Sim<Num> = Sim::new();
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Unconfirmed)];
    assert_eq!(e.confirm(&sim, &w, &p).driver_faults, vec![OUT]);
}

/// Zeile 6: ein bestaetigter Schreibvorgang bei intaktem Heartbeat nicht.
#[test]
fn a_confirmed_write_is_no_fault() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let sim: Sim<Num> = Sim::new();
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Acked)];
    assert!(e.confirm(&sim, &w, &p).driver_faults.is_empty());
}

/// Zeile 6: ein stiller Heartbeat faultet, auch wenn der Treiber
/// bestaetigt.
#[test]
fn a_silent_heartbeat_faults_the_owner() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let mut sim: Sim<Num> = Sim::new();
    sim.set_alive(false);
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Acked)];
    assert_eq!(e.confirm(&sim, &w, &p).driver_faults, vec![OUT]);
}

/// Zeile 7: Erst nach `N` aufeinanderfolgenden Verletzungen ist es ein
/// Fault; ein einzelner Ausreisser ist Jitter (7.1).
#[test]
fn the_tick_period_faults_only_after_n_runs() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    assert!(!e.period(2_000, 1_000, 100, 3));
    assert!(!e.period(2_000, 1_000, 100, 3));
    assert!(e.period(2_000, 1_000, 100, 3), "dritte Verletzung in Folge");
}

#[test]
fn a_single_outlier_is_no_hardware_fault() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    assert!(!e.period(2_000, 1_000, 100, 3));
    assert!(!e.period(1_050, 1_000, 100, 3), "wieder in Toleranz");
    assert!(!e.period(2_000, 1_000, 100, 3), "die Zaehlung begann von vorn");
}

// --- Der Simulationstreiber ist ein Treiber ------------------------------

/// Prinzip 4: Der Simulationstreiber implementiert dieselbe
/// Schnittstelle wie ein Hardwaretreiber — das ist die Zusage, auf der
/// „Sim/HW-Umschaltung ist ein Treiberwechsel" beruht.
#[test]
fn the_simulation_driver_is_an_ordinary_driver() {
    let mut sim: Sim<Num> = Sim::new();
    sim.feed(IN, Num(42.0), 1_500);
    let mut out = Vec::new();
    sim.read(1_500, &mut out);
    assert_eq!(out, vec![Reading { channel: IN, value: Some(Num(42.0)), quality: Quality::Good, t: 1_500 }]);
    assert_eq!(sim.write(&Writing { channel: OUT, value: Num(1.0), at: None }), Delivery::Acked);
    assert_eq!(sim.written.len(), 1);
}

/// Ein Treiber darf abwerten; der Rand wertet nicht auf.
#[test]
fn the_edge_never_upgrades_what_the_driver_reports() {
    let p = program();
    let mut e = Edge::new(&p, limits(p.channels.len()), 10_000_000);
    let r: [Reading<Num>; 1] = [Reading { channel: IN, value: None, quality: Quality::Bad, t: 1_500 }];
    let out = e.validate("sim", &r, &[], (1_000, 2_000), &p);
    assert_eq!(out.readings[0].1.quality, Quality::Bad);
}
