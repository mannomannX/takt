//! Der defensive Treiberrand (12.6), Zeile fuer Zeile.
//!
//! Jeder Test nennt die Zeile der Tabelle, die er prueft. Die Tabelle ist
//! normativ; ein Test, der sie nicht trifft, prueft etwas anderes.

use takt_hal::contract::{Contract, Device, Placement, Track, Turn, Window};
use takt_hal::driver::{Delivery, Driver, Element, Reading, Writing};
use takt_hal::edge::{Alert, Edge};
use takt_hal::quality::{Gate, Limits, Quality, Reason, Scalar};
use takt_hal::sim::Sim;
use takt_mir::ChannelId;
use takt_mir::pattern::Address;
use takt_mir::program::{Binding, Channel, ChannelAttrs, Direction, Program};

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

const SEC: i64 = 1_000_000_000;

fn range(lo: f64, hi: f64) -> Limits {
    Limits { range: Some((lo, hi)), ..Limits::default() }
}

fn slew(per_second: f64) -> Limits {
    Limits { max_slew: Some(per_second), ..Limits::default() }
}

// --- Zeile 3: Ranges ----------------------------------------------------

#[test]
fn a_value_outside_the_range_is_bad() {
    let mut g = Gate::default();
    let limits = range(0.0, 10.0);
    assert_eq!(g.check(&Num(5.0), 0, &limits).quality, Quality::Good);
    let v = g.check(&Num(11.0), SEC, &limits);
    assert_eq!(v.quality, Quality::Bad);
    assert_eq!(v.reason, Some(Reason::OutOfRange));
}

/// 3.5: „nicht geklemmt und nicht zu einem Fault" — der Wert faellt weg,
/// er wird nicht auf die Grenze gezogen.
#[test]
fn an_out_of_range_value_is_not_clamped() {
    let mut g = Gate::default();
    let v = g.check(&Num(11.0), 0, &range(0.0, 10.0));
    assert!(!v.held, "ein verworfener Wert haelt nichts");
}

// --- Zeile 4: max_slew --------------------------------------------------

#[test]
fn a_value_beyond_max_slew_is_implausible() {
    let mut g = Gate::default();
    let limits = slew(50.0);
    assert_eq!(g.check(&Num(0.0), 0, &limits).quality, Quality::Good);
    // 60 Einheiten in einer Sekunde bei erlaubten 50.
    let v = g.check(&Num(60.0), SEC, &limits);
    assert_eq!(v.quality, Quality::Bad);
    assert_eq!(v.reason, Some(Reason::Implausible));
}

/// Die Steigung zaehlt in beide Richtungen.
#[test]
fn a_falling_step_is_implausible_too() {
    let mut g = Gate::default();
    let limits = slew(50.0);
    g.check(&Num(100.0), 0, &limits);
    assert_eq!(g.check(&Num(40.0), SEC, &limits).reason, Some(Reason::Implausible));
}

#[test]
fn a_value_within_max_slew_stays_good() {
    let mut g = Gate::default();
    let limits = slew(50.0);
    g.check(&Num(0.0), 0, &limits);
    assert_eq!(g.check(&Num(40.0), SEC, &limits).quality, Quality::Good);
}

/// 3.5: „der erste Wert nach Start oder nach `Bad` gilt als gut".
#[test]
fn the_first_value_has_nothing_to_compare_against() {
    let mut g = Gate::default();
    assert_eq!(g.check(&Num(1e9), 0, &slew(1.0)).quality, Quality::Good);
}

#[test]
fn after_bad_the_next_value_is_good_again() {
    let mut g = Gate::default();
    let limits = slew(50.0);
    g.check(&Num(0.0), 0, &limits);
    assert_eq!(g.check(&Num(60.0), SEC, &limits).quality, Quality::Bad);
    // Ohne diese Regel bliebe der Kanal dauerhaft implausibel, weil er sich
    // weiter am alten Wert maesse.
    assert_eq!(g.check(&Num(61.0), 2 * SEC, &limits).quality, Quality::Good);
}

/// Ohne Zeitdifferenz ist eine Rate nicht definiert; 3.5 laesst den Wert
/// dann durch, statt ihn auf Verdacht zu verwerfen.
#[test]
fn two_deliveries_at_the_same_instant_have_no_rate() {
    let mut g = Gate::default();
    let limits = slew(1.0);
    g.check(&Num(0.0), SEC, &limits);
    assert_eq!(g.check(&Num(1000.0), SEC, &limits).quality, Quality::Good);
}

// --- Zeile 3 und 4 mit debounce ----------------------------------------

/// 3.5: „fuer bis zu drei aufeinanderfolgende Lieferungen als `Suspect`
/// … erst danach `Bad`".
#[test]
fn debounce_holds_three_deliveries_then_gives_up() {
    let mut g = Gate::default();
    let limits = Limits { debounce: 3, ..range(0.0, 10.0) };
    assert_eq!(g.check(&Num(5.0), 0, &limits).quality, Quality::Good);
    for i in 1..=3 {
        let v = g.check(&Num(99.0), i * SEC, &limits);
        assert_eq!(v.quality, Quality::Suspect, "Lieferung {i}");
        assert!(v.held, "der letzte gute Wert wird gehalten");
        assert_eq!(v.reason, Some(Reason::OutOfRange));
    }
    let v = g.check(&Num(99.0), 4 * SEC, &limits);
    assert_eq!(v.quality, Quality::Bad, "nach `debounce` Lieferungen");
    assert!(!v.held);
}

/// Die Zaehlung gilt fuer *aufeinanderfolgende* Verletzungen: Ein guter
/// Wert dazwischen setzt sie zurueck.
#[test]
fn a_good_delivery_resets_the_debounce_count() {
    let mut g = Gate::default();
    let limits = Limits { debounce: 2, ..range(0.0, 10.0) };
    g.check(&Num(5.0), 0, &limits);
    assert_eq!(g.check(&Num(99.0), SEC, &limits).quality, Quality::Suspect);
    assert_eq!(g.check(&Num(5.0), 2 * SEC, &limits).quality, Quality::Good);
    assert_eq!(g.check(&Num(99.0), 3 * SEC, &limits).quality, Quality::Suspect);
}

/// Ohne `debounce` ist eine Verletzung sofort `Bad` (Entscheidung 18).
#[test]
fn without_debounce_a_violation_is_immediate() {
    let mut g = Gate::default();
    assert_eq!(g.check(&Num(99.0), 0, &range(0.0, 10.0)).quality, Quality::Bad);
}

/// Range vor `max_slew`: Ein Wert weit ausserhalb verletzt fast immer
/// beides, und `OutOfRange` ist die praezisere Auskunft.
#[test]
fn out_of_range_wins_over_implausible() {
    let mut g = Gate::default();
    let limits = Limits { max_slew: Some(1.0), ..range(0.0, 10.0) };
    g.check(&Num(5.0), 0, &limits);
    assert_eq!(g.check(&Num(1000.0), SEC, &limits).reason, Some(Reason::OutOfRange));
}

// --- Der Kern: Zeilen 1 und 2 je Kanal ----------------------------------

const W: Window = Window { lo: 1_000, hi: 2_000, tolerance: 500 };

/// Zeile 1: das Fenster ist `(lo, hi]`; daneben in der Toleranz wird
/// geklemmt, jenseits ist es eine Verletzung.
#[test]
fn the_window_places_a_timestamp() {
    assert_eq!(W.place(1_001), Placement::Inside);
    assert_eq!(W.place(2_000), Placement::Inside);
    assert_eq!(W.place(1_000), Placement::Warped(1_001), "`lo` gehoert zum vorigen Tick");
    assert_eq!(W.place(2_400), Placement::Warped(2_000));
    assert_eq!(W.place(500), Placement::Outside);
    assert_eq!(W.place(2_501), Placement::Outside);
}

/// Das Fenster des Ticks `k` endet an seiner Grenze `k·T0`.
#[test]
fn the_window_of_a_tick_ends_at_its_boundary() {
    assert_eq!(Window::of_tick(3, 1_000, 7), Window { lo: 2_000, hi: 3_000, tolerance: 7 });
}

/// Zeile 2: `seq` lueckenlos und streng steigend.
#[test]
fn a_gap_in_seq_breaks_the_contract() {
    let mut t = Track::new(0);
    assert!(t.element(1_100, 7, &W).is_ok(), "das erste Element setzt die Folge");
    assert!(t.element(1_200, 8, &W).is_ok());
    assert_eq!(t.element(1_300, 10, &W), Err(Contract::Sequence));
    assert_eq!(t.element(1_400, 10, &W), Err(Contract::Sequence), "Wiederholung ist keine Folge");
}

/// Nach einer Verletzung folgt der Stand der Lieferung: Die naechste
/// lueckenlose Folge ist wieder vertragsgemaess (plan/m10.md 2.3).
#[test]
fn after_a_gap_the_next_gapless_seq_is_fine() {
    let mut t = Track::new(0);
    t.element(1_100, 1, &W).ok();
    assert_eq!(t.element(1_200, 5, &W), Err(Contract::Sequence));
    assert!(t.element(1_300, 6, &W).is_ok());
}

/// Zeile 2: Zeitstempel duerfen gleich bleiben, nicht fallen.
#[test]
fn timestamps_may_repeat_but_not_fall() {
    let mut t = Track::new(0);
    t.element(1_500, 1, &W).ok();
    assert!(t.element(1_500, 2, &W).is_ok());
    assert_eq!(t.element(1_400, 3, &W), Err(Contract::Timestamp));
    let mut s = Track::new(0);
    s.reading(1_500, 0, false, &W).ok();
    assert_eq!(s.reading(1_499, 0, false, &W), Err(Contract::Timestamp));
}

/// Zeile 2: `len(D) <= MAXPT` je Stream und Tick.
#[test]
fn more_than_maxpt_elements_break_the_contract() {
    let mut t = Track::new(2);
    for seq in 0..2 {
        t.element(1_500, seq, &W).ok();
    }
    assert!(t.finish().is_ok());
    for seq in 2..5 {
        t.element(1_600, seq, &W).ok();
    }
    assert_eq!(t.finish(), Err(Contract::TooMany));
}

/// Zeile 2: Flags konsistent — `Bad` ohne Wert, und das Alter monoton:
/// Ein Wert kann nicht frueher gemessen sein als der vorige.
#[test]
fn the_age_of_a_value_is_monotonic() {
    let mut t = Track::new(0);
    assert!(t.reading(1_500, 300, false, &W).is_ok(), "gemessen bei 1200");
    assert!(t.reading(1_600, 400, false, &W).is_ok(), "derselbe Wert, aelter");
    assert_eq!(t.reading(1_700, 600, false, &W), Err(Contract::Flags), "gemessen bei 1100 < 1200");
    assert_eq!(t.reading(1_800, 0, true, &W), Err(Contract::Flags), "`Bad` mit Wert");
}

/// Zeile 2: Schweigen ist keine Erholung.
#[test]
fn a_silent_driver_stays_degraded() {
    let mut d = Device::default();
    assert_eq!(d.settle(true, Some(Contract::Sequence)), Turn::Degraded(Contract::Sequence));
    assert_eq!(d.settle(true, Some(Contract::Sequence)), Turn::Steady, "keine zweite Meldung");
    assert_eq!(d.settle(false, None), Turn::Steady);
    assert!(d.degraded);
    assert_eq!(d.settle(true, None), Turn::Recovered);
}

// --- Der Rand als Ganzes ------------------------------------------------

fn channel(name: &str, dir: Direction, address: &str) -> Channel {
    Channel {
        dir,
        name: name.into(),
        ty: takt_mir::TypeId(0),
        binding: Binding::Hw(Address::simple(address)),
        attrs: ChannelAttrs::default(),
        meta: takt_mir::program::Meta::default(),
        owner: None,
        span: takt_diag::Span::new(0, 0),
    }
}

/// Ein Programm mit zwei Inputs am Geraet `adc`, einem an `dio` und einem
/// Output.
fn program() -> Program {
    let mut p = Program::new(takt_mir::program::Config::new(1, 1_000_000));
    p.channels.push(channel("p", Direction::Input, "adc/p"));
    p.channels.push(channel("o", Direction::Output, "dio/o"));
    p.channels.push(channel("q", Direction::Input, "adc/q"));
    p.channels.push(channel("k", Direction::Input, "dio/k"));
    p
}

fn edge(p: &Program, tolerance: i64) -> Edge {
    Edge::new(p, vec![Limits::default(); p.channels.len()], tolerance)
}

fn reading(channel: ChannelId, t: i64) -> Reading<Num> {
    Reading { channel, value: Some(Num(1.0)), quality: Quality::Good, t, age: 0 }
}

const IN: ChannelId = ChannelId(0);
const OUT: ChannelId = ChannelId(1);
const OTHER: ChannelId = ChannelId(2);
const KEY: ChannelId = ChannelId(3);

/// Zeile 1: Ein Zeitstempel knapp neben dem Fenster wird geklemmt und
/// gezaehlt, nicht verworfen.
#[test]
fn a_timestamp_outside_the_window_is_clamped() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    let out = e.contract(&[reading(IN, 500)], &[], (1_000, 2_000), &p);
    assert_eq!(e.time_warped, 1);
    assert!(matches!(out.alerts[0], Alert::TimeWarped { clamped: 1_001, .. }));
    assert_eq!(out.readings, vec![Some(1_001)], "geklemmt, nicht verworfen");
}

/// Zeile 1, zweiter Fall: jenseits der Toleranz wie Zeile 2.
#[test]
fn a_timestamp_far_outside_degrades_the_driver() {
    let p = program();
    let mut e = edge(&p, 1_000);
    let out = e.contract(&[reading(IN, -1_000_000)], &[], (1_000, 2_000), &p);
    assert!(matches!(out.alerts[0], Alert::DriverDegraded { what: Contract::TimeWindow, .. }));
    assert_eq!(out.readings, vec![None]);
}

/// Zeile 2: `Bad` mit Wert ist ein Widerspruch in den Flags.
#[test]
fn bad_with_a_value_is_a_contract_violation() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    let r = Reading { quality: Quality::Bad, ..reading(IN, 1_500) };
    let out = e.contract(&[r], &[], (1_000, 2_000), &p);
    assert!(matches!(out.alerts[0], Alert::DriverDegraded { what: Contract::Flags, .. }));
}

/// Zeile 2: Erholung, sobald der Treiber wieder vertragsgemaess liefert.
#[test]
fn a_driver_recovers_when_it_delivers_correctly_again() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    let bad = Reading { quality: Quality::Bad, ..reading(IN, 1_500) };
    e.contract(&[bad], &[], (1_000, 2_000), &p);
    let out = e.contract(&[reading(IN, 2_500)], &[], (2_000, 3_000), &p);
    assert!(matches!(&out.alerts[0], Alert::DriverRecovered { driver } if driver == "adc"));
    assert_eq!(out.readings, vec![Some(2_500)]);
    assert!(out.degraded.is_empty());
}

/// Zeile 2: Ein Vertragsbruch trifft *alle* Inputs des Treibers — auch die,
/// die in diesem Tick nichts geliefert haben —, aber keinen anderen
/// Treiber und keinen Output.
#[test]
fn a_contract_violation_degrades_every_channel_of_the_driver() {
    let p = program();
    let mut e = edge(&p, 1_000);
    let r = [reading(IN, -1_000_000), reading(KEY, 1_500)];
    let out = e.contract(&r, &[], (1_000, 2_000), &p);
    assert_eq!(out.readings, vec![None, Some(1_500)]);
    assert_eq!(out.degraded, vec![IN, OTHER]);
}

/// Zeile 2 fuer Stroeme: Ein Element mit Luecke laesst den ganzen Stapel
/// des Treibers fallen.
#[test]
fn a_gap_drops_the_elements_of_the_driver() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    let el = |seq| Element { channel: IN, t: 1_500, seq, value: Num(1.0) };
    let first = e.contract::<Num>(&[], &[el(1), el(2)], (1_000, 2_000), &p);
    assert_eq!(first.elements, vec![Some(1_500), Some(1_500)]);
    let gap = e.contract::<Num>(&[], &[el(4)], (1_000, 2_000), &p);
    assert_eq!(gap.elements, vec![None]);
    assert!(matches!(gap.alerts[0], Alert::DriverDegraded { what: Contract::Sequence, .. }));
    let back = e.contract::<Num>(&[], &[el(5)], (1_000, 2_000), &p);
    assert_eq!(back.elements, vec![Some(1_500)], "lueckenlos ab dem Gelieferten");
}

/// Zeile 5: Ein nicht decodierbares Element zaehlt.
#[test]
fn a_malformed_element_is_counted() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    assert_eq!(e.malformed(IN), Alert::Malformed { channel: IN });
    assert_eq!(e.malformed, 1);
}

/// Zeile 6: Ein nicht bestaetigter Schreibvorgang faultet den Besitzer.
#[test]
fn an_unconfirmed_write_faults_the_owner() {
    let p = program();
    let e = edge(&p, 10_000_000);
    let sim: Sim<Num> = Sim::new();
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Unconfirmed)];
    assert_eq!(e.confirm(&sim, &w, &p), vec![OUT]);
}

/// Zeile 6: ein bestaetigter Schreibvorgang bei intaktem Heartbeat nicht.
#[test]
fn a_confirmed_write_is_no_fault() {
    let p = program();
    let e = edge(&p, 10_000_000);
    let sim: Sim<Num> = Sim::new();
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Acked)];
    assert!(e.confirm(&sim, &w, &p).is_empty());
}

/// Zeile 6: ein stiller Heartbeat faultet, auch wenn der Treiber
/// bestaetigt.
#[test]
fn a_silent_heartbeat_faults_the_owner() {
    let p = program();
    let e = edge(&p, 10_000_000);
    let mut sim: Sim<Num> = Sim::new();
    sim.set_alive(false);
    let w = [(Writing { channel: OUT, value: Num(1.0), at: None }, Delivery::Acked)];
    assert_eq!(e.confirm(&sim, &w, &p), vec![OUT]);
}

/// Zeile 6: ein ueberfahrener Sendepuffer (`free[o] > capacity`).
#[test]
fn an_overrun_send_buffer_faults_the_owner() {
    assert!(takt_hal::contract::output_fails(true, true, Some(65), Some(64), None));
    assert!(!takt_hal::contract::output_fails(true, true, Some(64), Some(64), None));
    assert!(!takt_hal::contract::output_fails(true, true, None, Some(64), None));
}

/// FB-124 (8.8, Zeile 6): Ein Sender, der sich fertig meldet, obwohl sein
/// Puffer nicht leer ist (`idle` bei `free < capacity`), bricht den
/// Vertrag; bei leerem Puffer oder ohne Antwort nicht.
#[test]
fn an_idle_sender_with_bytes_in_its_buffer_faults_the_owner() {
    use takt_hal::contract::output_fails;
    assert!(output_fails(true, true, Some(60), Some(64), Some(true)));
    assert!(!output_fails(true, true, Some(64), Some(64), Some(true)));
    assert!(!output_fails(true, true, Some(60), Some(64), Some(false)));
    assert!(!output_fails(true, true, Some(60), Some(64), None), "ohne Antwort raet der Rand nicht");
    assert!(!output_fails(true, true, None, Some(64), Some(true)), "ohne `free` keine Aussage");
}

/// Zeile 7: Erst nach `N` aufeinanderfolgenden Verletzungen ist es ein
/// Fault; ein einzelner Ausreisser ist Jitter (7.1).
#[test]
fn the_tick_period_faults_only_after_n_runs() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    assert!(!e.period(2_000, 1_000, 100, 3));
    assert!(!e.period(2_000, 1_000, 100, 3));
    assert!(e.period(2_000, 1_000, 100, 3), "dritte Verletzung in Folge");
}

#[test]
fn a_single_outlier_is_no_hardware_fault() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
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
    assert_eq!(
        out,
        vec![reading(IN, 1_500)].into_iter().map(|r| Reading { value: Some(Num(42.0)), ..r }).collect::<Vec<_>>()
    );
    assert_eq!(sim.write(&Writing { channel: OUT, value: Num(1.0), at: None }), Delivery::Acked);
    assert_eq!(sim.written.len(), 1);
}

/// Ein Treiber darf abwerten; der Rand wertet nicht auf: `Bad` ohne Wert
/// ist vertragsgemaess und geht als `Bad` weiter.
#[test]
fn the_edge_never_upgrades_what_the_driver_reports() {
    let p = program();
    let mut e = edge(&p, 10_000_000);
    let r: [Reading<Num>; 1] = [Reading { value: None, quality: Quality::Bad, ..reading(IN, 1_500) }];
    let out = e.contract(&r, &[], (1_000, 2_000), &p);
    assert_eq!(out.readings, vec![Some(1_500)]);
    assert_eq!(e.driver_bad(IN).quality, Quality::Bad);
}

// --- Zeile 7: Grenzen der Periode (RT-005) ------------------------------

/// **Die Toleranz selbst ist noch in der Toleranz**, ihr Ueberschreiten um
/// eine Nanosekunde nicht; Toleranz 0 laesst nur die nominale Periode zu.
#[test]
fn the_tolerance_of_the_period_is_inclusive() {
    let mut p = takt_hal::contract::Period::default();
    assert!(!p.observe(1_100, 1_000, 100, 1), "genau die Toleranz");
    assert!(!p.observe(900, 1_000, 100, 1), "nach unten ebenso");
    assert!(p.observe(1_101, 1_000, 100, 1), "eine Nanosekunde darueber");
    let mut zero = takt_hal::contract::Period::default();
    assert!(!zero.observe(1_000, 1_000, 0, 1));
    assert!(zero.observe(1_001, 1_000, 0, 1), "Toleranz 0: jede Abweichung");
}

/// **`runs = 0` gilt wie 1**, und nach der N-ten Verletzung meldet jede
/// weitere erneut — `Runtime(Hardware)` je Tick, bis die Periode
/// zurueckkehrt; danach beginnt die Zaehlung von vorn.
#[test]
fn every_violation_after_the_nth_reports_again_until_the_period_returns() {
    let mut once = takt_hal::contract::Period::default();
    assert!(once.observe(2_000, 1_000, 100, 0), "runs 0 wie 1");
    let mut p = takt_hal::contract::Period::default();
    let seen: Vec<bool> = (0..5).map(|_| p.observe(2_000, 1_000, 100, 3)).collect();
    assert_eq!(seen, [false, false, true, true, true]);
    assert!(!p.observe(1_000, 1_000, 100, 3), "zurueck in der Toleranz");
    assert!(!p.observe(2_000, 1_000, 100, 3), "neu gezaehlt");
}

// --- Zeile 3 und 4: Ganzzahlen und nicht endliche Werte (RT-021) --------

/// Ein ganzzahliger Wert, wie ein `i64`-Kanal ihn liefert.
#[derive(Clone, Copy, Debug)]
struct Int(i64);

impl Scalar for Int {
    fn as_f64(&self) -> Option<f64> {
        Some(self.0 as f64)
    }

    fn as_i64(&self) -> Option<i64> {
        Some(self.0)
    }
}

/// **Ganzzahlen werden ganzzahlig gegen ganzzahlige Grenzen geprueft**
/// (3.4): `2^53 + 1` rundet in `f64` auf `2^53` und laege sonst in der Range.
#[test]
fn an_integer_beyond_2_pow_53_is_compared_exactly() {
    let big = 1i64 << 53;
    let limits = range(0.0, big as f64);
    assert_eq!(Gate::default().check(&Int(big), 0, &limits).quality, Quality::Good);
    let v = Gate::default().check(&Int(big + 1), 0, &limits);
    assert_eq!((v.quality, v.reason), (Quality::Bad, Some(Reason::OutOfRange)));
    let low = range(-(big as f64), 0.0);
    assert_eq!(Gate::default().check(&Int(-big - 1), 0, &low).reason, Some(Reason::OutOfRange));
    assert_eq!(Gate::default().check(&Int(i64::MAX), 0, &range(0.0, 1e19)).quality, Quality::Good);
}

/// **NaN und Unendlich sind nie gut** (4.1: `float` kennt nur endliche
/// Werte; Anhang A, INT-025: wie ein Range-Verstoss): ob mit Range, nur mit
/// `max_slew` oder ohne Grenzen. Danach misst die Steigung wieder am letzten
/// endlichen Wert.
#[test]
fn a_non_finite_value_is_never_good() {
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for limits in [range(0.0, 10.0), slew(50.0), Limits::default()] {
            let mut g = Gate::default();
            g.check(&Num(1.0), 0, &limits);
            let v = g.check(&Num(x), SEC, &limits);
            assert_eq!((v.quality, v.reason), (Quality::Bad, Some(Reason::OutOfRange)), "{x} unter {limits:?}");
        }
    }
    let mut g = Gate::default();
    let limits = Limits { debounce: 1, ..slew(50.0) };
    g.check(&Num(0.0), 0, &limits);
    assert_eq!(g.check(&Num(f64::NAN), SEC, &limits).quality, Quality::Suspect);
    let v = g.check(&Num(1_000.0), 2 * SEC, &limits);
    assert_eq!(v.reason, Some(Reason::Implausible), "gemessen an 0, nicht an NaN");
}

// --- Zeilen 1, 2 und 6: Grenzfaelle (RT-022) ----------------------------

/// **Zeile 1 an ihren Grenzen**: `hi` ist drin, `hi + 1` geklemmt,
/// `hi + tol` geklemmt, `hi + tol + 1` draussen; ebenso unten. Toleranz 0
/// klemmt nichts, der Default von einem Tick klemmt bis eine Periode daneben.
#[test]
fn the_window_at_its_edges() {
    let w = Window { lo: 1_000, hi: 2_000, tolerance: 300 };
    let cases = [
        (2_000, Placement::Inside),
        (2_001, Placement::Warped(2_000)),
        (2_300, Placement::Warped(2_000)),
        (2_301, Placement::Outside),
        (1_001, Placement::Inside),
        (1_000, Placement::Warped(1_001)),
        (701, Placement::Warped(1_001)),
        (700, Placement::Outside),
    ];
    for (t, want) in cases {
        assert_eq!(w.place(t), want, "t = {t}");
    }
    let zero = Window { tolerance: 0, ..w };
    assert_eq!((zero.place(1_000), zero.place(2_001)), (Placement::Outside, Placement::Outside));
    let tick = Window::of_tick(2, 1_000, 1_000);
    assert_eq!((tick.place(1), tick.place(0)), (Placement::Warped(1_001), Placement::Outside), "ein Tick daneben");
}

/// Das Fenster von Tick 0 liegt vor dem Start, und am Ende des Bereichs
/// saettigt es, statt umzuschlagen.
#[test]
fn the_window_of_tick_zero_and_of_the_last_tick() {
    assert_eq!(Window::of_tick(0, 1_000, 5), Window { lo: -1_000, hi: 0, tolerance: 5 });
    let last = Window::of_tick(i64::MAX, 2, 0);
    assert_eq!((last.lo, last.hi), (i64::MAX - 2, i64::MAX));
    assert_eq!(last.place(i64::MAX), Placement::Inside);
}

/// Nach `seq = i64::MAX` gibt es keine lueckenlose Folge mehr.
#[test]
fn nothing_follows_the_largest_seq() {
    let mut t = Track::new(0);
    assert!(t.element(1_500, i64::MAX - 1, &W).is_ok());
    assert!(t.element(1_500, i64::MAX, &W).is_ok());
    assert_eq!(t.element(1_500, i64::MIN + 1, &W), Err(Contract::Sequence), "kein Umschlagen");
}

/// Ein Zeitstempel, der ueber die Tickgrenze zurueckfaellt, ist eine
/// Verletzung, auch wenn er im Fenster des neuen Ticks geklemmt wuerde.
#[test]
fn a_timestamp_falling_across_a_tick_boundary_breaks_the_contract() {
    let mut t = Track::new(0);
    assert!(t.reading(1_900, 0, false, &W).is_ok());
    let next = Window { lo: 2_000, hi: 3_000, tolerance: 500 };
    assert_eq!(t.reading(1_800, 0, false, &next), Err(Contract::Timestamp));
}

/// Ein Programm mit einem Eingabestrom `s` am Geraet `uart` mit `max_rate`
/// und einem Ausgabestrom `tx` mit `capacity_bytes = 64`.
fn stream_program(rate: takt_mir::expr::ExprKind, tick: i64) -> Program {
    let mut p = Program::new(takt_mir::program::Config::new(1, tick));
    let mut s = channel("s", Direction::Input, "uart/rx");
    s.attrs.max_rate = Some(takt_mir::expr::Expr::new(rate, takt_mir::TypeId(0), takt_diag::Span::new(0, 0)));
    p.channels.push(s);
    let mut tx = channel("tx", Direction::Output, "uart/tx");
    tx.attrs.capacity_bytes = Some(64);
    p.channels.push(tx);
    p
}

/// **`MAXPT = ceil(max_rate * T0)`** (8.6), auch fuer eine gebrochene Rate:
/// 2,5 Hz bei einer Sekunde Tick sind drei Elemente, nicht zwei.
#[test]
fn maxpt_rounds_up_also_for_a_fractional_rate() {
    use takt_hal::edge::{maxpt, maxpt_of};
    use takt_mir::expr::ExprKind;
    assert_eq!(maxpt(3, SEC / 2), Some(2), "1,5 aufgerundet");
    assert_eq!(maxpt(2, SEC / 2), Some(1));
    assert_eq!(maxpt(0, SEC), Some(1), "mindestens eins");
    assert_eq!(maxpt(u64::MAX, 2), None, "Ueberlauf ist keine Zahl");
    let p = stream_program(ExprKind::Float(2.5), SEC);
    assert_eq!(maxpt_of(&p.channels[0], SEC), Some(3));
    let p = stream_program(ExprKind::Float(0.1), 10 * SEC);
    assert_eq!(maxpt_of(&p.channels[0], 10 * SEC), Some(1), "0,1 Hz mal 10 s ist genau eins");
    let p = stream_program(ExprKind::Int(1000), SEC / 1000);
    assert_eq!(maxpt_of(&p.channels[0], SEC / 1000), Some(1));
}

/// **`MAXPT` ueber den ganzen Rand** (8.6, Zeile 2): genau `MAXPT`
/// Elemente sind vertragsgemaess, eines mehr degradiert den Treiber, und
/// der naechste Tick mit `MAXPT` erholt ihn.
#[test]
fn maxpt_holds_through_the_edge_and_recovers() {
    let p = stream_program(takt_mir::expr::ExprKind::Int(3), SEC);
    let mut e = edge(&p, SEC);
    let el = |seq| Element { channel: ChannelId(0), t: 1_500, seq, value: Num(1.0) };
    let ok = e.contract::<Num>(&[], &[el(0), el(1), el(2)], (1_000, 2_000), &p);
    assert!(ok.alerts.is_empty(), "{:?}", ok.alerts);
    let many = e.contract::<Num>(&[], &[el(3), el(4), el(5), el(6)], (1_000, 2_000), &p);
    assert!(matches!(many.alerts[..], [Alert::DriverDegraded { what: Contract::TooMany, .. }]), "{:?}", many.alerts);
    assert_eq!(many.elements, vec![None; 4]);
    let back = e.contract::<Num>(&[], &[el(7)], (1_000, 2_000), &p);
    assert!(matches!(back.alerts[..], [Alert::DriverRecovered { .. }]), "{:?}", back.alerts);
}

/// **Mehrere Verletzungen eines Treibers in einem Tick melden die erste**
/// (Zeile 2: ein Alert je Degradierung), und die Erholung nach einer
/// Fensterverletzung kommt mit dem naechsten Tick im Fenster.
#[test]
fn several_violations_in_one_tick_report_the_first() {
    let p = program();
    let mut e = edge(&p, 100);
    let bad = Reading { quality: Quality::Bad, ..reading(IN, 1_500) };
    let out = e.contract(&[reading(OTHER, -1_000_000), bad], &[], (1_000, 2_000), &p);
    assert!(
        matches!(out.alerts[..], [Alert::DriverDegraded { what: Contract::TimeWindow, .. }]),
        "nur die erste: {:?}",
        out.alerts
    );
    let back = e.contract(&[reading(OTHER, 2_500)], &[], (2_000, 3_000), &p);
    assert!(matches!(back.alerts[..], [Alert::DriverRecovered { .. }]), "{:?}", back.alerts);
}

/// **Zeile 6 ueber `Edge::confirm` mit `capacity_bytes` des Programms**:
/// Meldet der Treiber mehr freien Platz, als der Puffer hat, ist er
/// ueberfahren.
#[test]
fn a_send_buffer_beyond_its_capacity_faults_through_the_edge() {
    let p = stream_program(takt_mir::expr::ExprKind::Int(10), SEC);
    let e = edge(&p, SEC);
    let tx = ChannelId(1);
    let w = [(Writing { channel: tx, value: Num(1.0), at: None }, Delivery::Acked)];
    let mut sim: Sim<Num> = Sim::new();
    sim.set_free(tx, 64);
    assert!(e.confirm(&sim, &w, &p).is_empty(), "genau die Kapazitaet");
    sim.set_free(tx, 65);
    assert_eq!(e.confirm(&sim, &w, &p), vec![tx]);
}

/// FB-124 (8.8): Das Simulationsgeraet meldet `idle` genau dann, wenn
/// `free == capacity`; der Rand nimmt die Antwort an. Ohne Kapazitaet weiss
/// es nichts.
#[test]
fn the_simulated_sender_is_idle_exactly_with_an_empty_buffer() {
    use takt_hal::Driver;
    let p = stream_program(takt_mir::expr::ExprKind::Int(10), SEC);
    let e = edge(&p, SEC);
    let tx = ChannelId(1);
    let w = [(Writing { channel: tx, value: Num(1.0), at: None }, Delivery::Acked)];
    let mut sim: Sim<Num> = Sim::new();
    sim.set_free(tx, 60);
    assert_eq!(Driver::idle(&sim, tx), None, "ohne Kapazitaet");
    sim.set_capacity(tx, 64);
    assert_eq!(Driver::idle(&sim, tx), Some(false));
    assert!(e.confirm(&sim, &w, &p).is_empty(), "ein voller Puffer mit `idle = false` ist vertragsgemaess");
    sim.set_free(tx, 64);
    assert_eq!(Driver::idle(&sim, tx), Some(true));
    assert!(e.confirm(&sim, &w, &p).is_empty());
}
