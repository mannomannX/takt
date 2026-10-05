//! Die Prüfungen mit Kalibrierung (12, 32; 7.2, 9.4.3, 13.8).
//!
//! **Was hier belegt wird.** Bis zur Kalibrierung war die zentrale
//! Zeitzusage der Sprache unbelegt: `takt cost` rechnete Operationen,
//! aber was sie dauern, konnte niemand sagen. Prüfung 12 meldete darum
//! einen Hinweis, Prüfung 32 gab es nicht, und `wcet` wurde abgelehnt
//! statt geprüft.
//!
//! Diese Tests zeigen beides — dass ohne Tabelle nichts behauptet wird,
//! und dass mit Tabelle geurteilt wird.

use takt_diag::{Policy, Span};
use takt_mir::hardware::{self, Target};

const HW: &str = "\
# takt-hw 1
[target.probe]
core_hz = 84000000
i32 = 11905
i64 = 35715
f32 = 11905
f64 = 1190500
mem = 23810
call = 47620
native = 0
t_io = 120000
";

/// Die Tabelle zum Kostenmodell dieses Compilers.
fn hw() -> String {
    format!("{HW}cost_model = {}\n", takt_mir::analysis::cost::MODEL_VERSION)
}

fn ziel() -> Target {
    hardware::parse(&hw()).expect("lesbar").target("probe").expect("Ziel").clone()
}

fn compile(src: &str) -> takt_mir::Program {
    let options = takt_sema::Options {
        policy: Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

const KOPF: &str = "system:\n    language = 1\n    tick = 10 ms\n    tick_source = hw(\"sys/clock\")\n\n\
                    output led : bool @ hw(\"ui/led\") with safe = false\n\n";

/// **Eine Last, die passt, meldet nichts.**
#[test]
fn a_program_that_fits_produces_no_diagnostic() {
    let p =
        compile(&format!("{KOPF}machine m:\n    initial S\n\n    state S:\n        enter:\n            led = true\n"));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    assert!(d.is_empty(), "{d:?}");
}

/// **Eine Last, die nicht passt, ist ein Fehler mit Zahlen.**
///
/// 7.2: „Verletzung ist ein Compile-Fehler mit Vorschlag (Periode
/// erhöhen, Phase verschieben, Fault-Pfade verkleinern)."
#[test]
fn a_program_that_does_not_fit_is_an_error() {
    let p = compile(&format!(
        "{}machine m:\n    var acc : float = 0.0\n\n    initial S\n\n    state S:\n        loop:\n            \
         for i in range(2000):\n                acc = acc + 1.5\n            led = acc > 0.0\n",
        KOPF.replace("tick = 10 ms", "tick = 100 us")
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let fehler = d.iter().find(|d| d.is_error()).expect("ein Fehler");
    assert!(fehler.code == "SC-32", "{}", fehler.code);
    assert!(format!("{fehler}").contains("passt nicht"), "{fehler}");
    assert!(fehler.suggestion.is_some(), "7.2 verlangt einen Vorschlag");
}

/// **`wcet` wird geprüft, nicht mehr abgelehnt** (Prüfung 12).
#[test]
fn a_declared_wcet_is_checked_against_the_calibration() {
    let p = compile(&format!(
        "{KOPF}machine m with budget = {{wcet = 1 us}}:\n    var acc : float = 0.0\n\n    initial S\n\n    \
         state S:\n        loop:\n            for i in range(500):\n                acc = acc + 1.5\n            \
         led = acc > 0.0\n"
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let fehler = d.iter().find(|d| d.code == "SC-12").expect("SC-12 urteilt");
    assert!(fehler.is_error(), "{fehler}");
    assert!(format!("{fehler}").contains("je Aktivierung"), "{fehler}");
}

/// Ein eingehaltenes `wcet` meldet nichts.
#[test]
fn a_generous_wcet_passes() {
    let p = compile(&format!(
        "{KOPF}machine m with budget = {{wcet = 1 ms}}:\n    initial S\n\n    state S:\n        enter:\n            \
         led = true\n"
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    assert!(d.iter().all(|d| d.code != "SC-12"), "{d:?}");
}

/// **Ohne `tick_source` meldet Prüfung 32 einen Fehler.**
///
/// Sie verlangt zweierlei — die Ungleichung *und* dass die Tickquelle in
/// der Hardware-Konfiguration steht (7.1). Das Zweite ist unabhängig von
/// der Kalibrierung, und nicht entscheidbar heißt nicht bestanden
/// (Tabelle 10, Zeile 32).
#[test]
fn a_missing_tick_source_is_reported() {
    let p = compile(&KOPF.replace("    tick_source = hw(\"sys/clock\")\n", ""));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let w = d.iter().find(|d| d.code == "SC-32").expect("SC-32 meldet");
    assert!(w.is_error(), "{w}");
    assert!(format!("{w}").contains("`tick_source` fehlt"), "{w}");
}

/// Die Bindungen eines Programms gegen eine Konfiguration mit einem Kanal.
fn bindings_with_clock(tick_source: &str, channel: &str) -> Vec<takt_diag::Diagnostic> {
    let p = compile(&KOPF.replace("sys/clock", tick_source));
    let config =
        format!("# takt-hw 4\n[channel ui/led]\ndirection = output\n\n[channel {channel}]\ndirection = input\n");
    let hw = hardware::parse(&config).expect("lesbar");
    takt_sema::calibrated::check_bindings(&p, &hw).into_iter().filter(|d| d.code == "SC-32").collect()
}

/// **Die Tickquelle muss in der Konfiguration stehen** (Pruefung 32, 7.1):
/// Eine Adresse, die die Konfiguration nicht kennt, ist ein Fehler, der sie
/// nennt; das eingebaute Geraet `sys` kennt jede Konfiguration (12.7).
#[test]
fn a_tick_source_missing_from_the_configuration_is_an_error() {
    let d = bindings_with_clock("daq/clock", "daq/other");
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].is_error() && d[0].message.contains("`daq/clock`"), "{d:?}");
    assert!(bindings_with_clock("daq/clock", "daq/clock").is_empty());
    assert!(bindings_with_clock("sys/clock", "daq/other").is_empty(), "`sys` ist eingebaut");
}

/// Die Last einer Maschine mit einer Schleife aus `n` Gleitkommaadditionen.
fn adder(name: &str, n: u32) -> String {
    format!(
        "machine {name}:\n    var acc : float = 0.0\n\n    initial S\n\n    state S:\n        loop:\n            \
         for i in range({n}):\n                acc = acc + 1.5\n"
    )
}

/// **Die Last ist die Summe ueber die Maschinen** (7.2: `Peak = Σ_m B_m`):
/// Zwei Maschinen, die einzeln in den Tick passen, passen zusammen nicht.
#[test]
fn two_machines_that_fit_alone_can_overload_the_tick_together() {
    let fits = |src: &str| sc32(&compile(&format!("{KOPF}{src}")), &ziel()).iter().all(|d| !d.is_error());
    // 5000 Additionen zu je rund 1,19 µs: gut 6 ms von 10 ms je Maschine.
    assert!(fits(&adder("a", 5000)), "eine Maschine passt");
    assert!(fits(&adder("b", 5000)), "die andere auch");
    let both = sc32(&compile(&format!("{KOPF}{}\n{}", adder("a", 5000), adder("b", 5000))), &ziel());
    assert!(both.iter().any(|d| d.is_error() && d.message.contains("passt nicht")), "{both:?}");
}

/// **Gescopte Instanzen in exklusiven Zustaenden zaehlen mit ihrem
/// Maximum** (5.11, 9.4.3): Sie laufen nie zugleich. Stehen beide im
/// selben Zustand, zaehlt die Summe.
#[test]
fn scoped_instances_in_exclusive_states_count_with_their_maximum() {
    let program = |second: &str| {
        format!(
            "{KOPF}output a : bool @ hw(\"o/a\") with safe = false\n\
             output b : bool @ hw(\"o/b\") with safe = false\n\
             input  mode : int in 0..1 @ sim(\"i/mode\")\n\n\
             machine heavy(o: output bool):\n    var acc : float = 0.0\n\n    initial S\n\n    state S:\n        \
             loop:\n            for i in range(5000):\n                acc = acc + 1.5\n            o = acc > 0.0\n\n\
             machine ctrl:\n    initial FIRST\n\n    state FIRST:\n        instance p = heavy(o = a)\n{second}\n        \
             when mode == 1: -> SECOND\n\n    state SECOND:\n        when mode == 0: -> FIRST\n"
        )
    };
    let exclusive =
        program("").replace("    state SECOND:\n", "    state SECOND:\n        instance q = heavy(o = b)\n\n");
    let d = sc32(&compile(&exclusive), &ziel());
    assert!(d.iter().all(|d| !d.is_error()), "exklusiv: das Maximum passt: {d:?}");
    let together = sc32(&compile(&program("        instance q = heavy(o = b)\n")), &ziel());
    assert!(together.iter().any(|d| d.is_error() && d.message.contains("passt nicht")), "{together:?}");
}

/// **Eine unvollständige Tabelle urteilt nicht, sondern nennt die Lücke.**
///
/// Ein „passt" auf zu kleiner Grundlage wäre schlimmer als kein Urteil:
/// Es sähe aus wie eine bestandene Prüfung.
#[test]
fn an_incomplete_calibration_names_what_it_misses() {
    let mut t = ziel();
    t.c_target.set(takt_mir::fns::CostClass::Mem, 0);
    let p =
        compile(&format!("{KOPF}machine m:\n    initial S\n\n    state S:\n        enter:\n            led = true\n"));
    let d = takt_sema::calibrated::check(&p, &t, Span::new(0, 0));
    let w = d.iter().find(|d| d.code == "SC-32").expect("SC-32 meldet");
    assert!(format!("{w}").contains("mem"), "die Meldung nennt die fehlende Klasse: {w}");
    assert!(!w.is_error(), "eine fehlende Messung ist kein Programmfehler");
}

/// **Eine Last in `native` braucht ihr Gewicht.** Ohne Natives verlangt
/// die Tabelle es nicht; nennt eine Deklaration `cost = {native: …}`
/// (4.5), waere der Aufruf mit Gewicht null zeitlos — die Pruefung
/// urteilt dann nicht, sondern nennt die Luecke.
#[test]
fn a_native_load_needs_the_native_weight() {
    let p = compile(&format!(
        "{KOPF}native fn crc32(b: bytes<16>) -> u32 with cost = {{native: 50}}, stack = 64, total\n\n\
         machine m:\n    var b : bytes<16> = default\n    var c : u32 = 0\n\n    initial S\n\n    state S:\n        \
         loop:\n            c = crc32(b)\n            led = c > 0\n"
    ));
    let d = takt_sema::calibrated::check(&p, &ziel(), Span::new(0, 0));
    let w = d.iter().find(|d| d.code == "SC-32").expect("SC-32 meldet");
    assert!(format!("{w}").contains("native"), "die Meldung nennt `native`: {w}");
    assert!(!w.is_error(), "eine fehlende Messung ist kein Programmfehler");
}

/// Ein Ziel, dessen Journal den Kern anhaelt (12.3): 200 ms je Loeschung,
/// 5 ms je Programmiervorgang, bei 10 ms Tick also 21 Perioden.
fn blockierendes_ziel(blocking: Option<bool>) -> Target {
    let mut text =
        format!("{}iram = 65536\nnvm_sector_bytes = 4096\nnvm_erase_ns = 200000000\nnvm_program_ns = 5000000\n", hw());
    if let Some(b) = blocking {
        text.push_str(&format!("nvm_blocking = {b}\n"));
    }
    hardware::parse(&text.replace("takt-hw 1", "takt-hw 4")).expect("lesbar").target("probe").expect("Ziel").clone()
}

const PERSIST: &str = "machine m:\n    persist var n : int in 0..10 = 0\n\n    initial RUN\n\n    state RUN:\n        \
                       loop:\n            led = true\n";

const PERSIST_IDLE: &str = "machine m:\n    persist var n : int in 0..10 = 0\n\n    initial RUN\n\n    state RUN:\n        \
                            loop:\n            led = true\n\n        after 200 ms: -> SLEEP\n\n    state SLEEP idle:\n        \
                            after 500 ms: -> RUN\n";

/// **Eine Tabelle zu einem anderen Kostenmodell urteilt nicht** (13.8,
/// FB-300): Ihre Gewichte gehoeren zu anderen Zaehlungen. Pruefung 32
/// meldet das statt eines Urteils; der Speicher haengt nicht an den
/// Gewichten und wird weiter geprueft.
#[test]
fn a_table_of_another_cost_model_does_not_judge() {
    // 20 000 Operationen in `f64` zu je 1,19 µs passen nicht in 10 ms.
    let p = compile(&format!(
        "{KOPF}machine m:\n    var y : float = 1.0\n    initial S\n\n    state S:\n        loop:\n            \
         for i in range(10000):\n                y = y * 0.5 + 1.0\n"
    ));
    let current =
        hardware::parse(&format!("{}ram = 1\n", hw())).expect("lesbar").target("probe").expect("Ziel").clone();
    let mut stale = current.clone();
    stale.cost_model = None;
    let judged = takt_sema::calibrated::check(&p, &current, Span::new(0, 0));
    assert!(judged.iter().any(|d| d.code == "SC-32" && d.is_error()), "passende Tabelle urteilt: {judged:?}");
    assert!(judged.iter().any(|d| d.code == "SC-39"), "Speicher: {judged:?}");
    let d = takt_sema::calibrated::check(&p, &stale, Span::new(0, 0));
    assert!(d.iter().any(|d| d.code == "SC-39"), "der Speicher wird weiter geprueft: {d:?}");
    let sc32: Vec<String> = d.iter().filter(|d| d.code == "SC-32").map(|d| d.message.clone()).collect();
    assert_eq!(sc32.len(), 1, "{sc32:?}");
    assert!(sc32[0].contains("Kostenmodell"), "{sc32:?}");
    assert!(d.iter().all(|d| !d.is_error() || d.code == "SC-39"), "kein Urteil aus der Tabelle: {d:?}");
}

fn sc32(p: &takt_mir::Program, target: &Target) -> Vec<takt_diag::Diagnostic> {
    takt_sema::calibrated::check(p, target, Span::new(0, 0)).into_iter().filter(|d| d.code == "SC-32").collect()
}

/// **Blockierendes NVM unter `fault` ohne Schlaf ist ein Fehler** (12.3,
/// Pruefung 32): Jeder Schreibvorgang waere ein Fault.
#[test]
fn a_blocking_journal_under_fault_without_idle_is_an_error() {
    let p = compile(&format!("{KOPF}{PERSIST}"));
    let d = sc32(&p, &blockierendes_ziel(Some(true)));
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].is_error(), "{d:?}");
    assert!(d[0].message.contains("21 Perioden") && d[0].message.contains("210 ms"), "{}", d[0].message);
}

/// Mit einem `idle`-Zustand schreibt das Journal im Schlaf: nur ein Hinweis.
#[test]
fn a_blocking_journal_with_idle_is_a_note() {
    let p = compile(&format!("{KOPF}{PERSIST_IDLE}"));
    let d = sc32(&p, &blockierendes_ziel(Some(true)));
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, takt_diag::Severity::Note, "{d:?}");
    assert!(d[0].message.contains("Schlaffenstern"), "{}", d[0].message);
}

/// Unter `overrun = alert` ist es eine Warnung mit der Zahl (7.3).
#[test]
fn a_blocking_journal_under_alert_is_a_warning() {
    let src = format!("{}{PERSIST}", KOPF.replacen("tick = 10 ms\n", "tick = 10 ms\n    overrun = alert\n", 1));
    let d = sc32(&compile(&src), &blockierendes_ziel(Some(true)));
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, takt_diag::Severity::Warning, "{d:?}");
    assert!(d[0].message.contains("Overrun-Alert"), "{}", d[0].message);
}

/// Ein XIP-Ziel ohne `nvm_blocking` ist nicht entscheidbar, und nicht
/// entscheidbar heisst nicht bestanden: ein Fehler, der den Schluessel
/// nennt (Tabelle 10, Zeile 32). Ein Ziel, das nicht blockiert, meldet
/// nichts.
#[test]
fn a_missing_nvm_blocking_on_a_xip_target_is_undecidable() {
    let p = compile(&format!("{KOPF}{PERSIST}"));
    let d = sc32(&p, &blockierendes_ziel(None));
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].is_error(), "{d:?}");
    assert!(d[0].message.contains("nicht entscheidbar") && d[0].message.contains("`nvm_blocking`"), "{}", d[0].message);
    assert!(sc32(&p, &blockierendes_ziel(Some(false))).is_empty());
    assert!(sc32(&p, &ziel()).is_empty(), "ohne iram und ohne NVM kein Urteil");
}

/// Pruefung 32 rundet die Perioden auf: 200 ms plus zweimal 5,5 ms sind
/// 211 ms, bei 10 ms Tick also 22 Perioden, nicht 21.
#[test]
fn the_journal_periods_round_up() {
    let mut target = blockierendes_ziel(Some(true));
    target.nvm.as_mut().expect("NVM").program_ns = Some(5_500_000);
    let d = sc32(&compile(&format!("{KOPF}{PERSIST}")), &target);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].message.contains("22 Perioden") && d[0].message.contains("211 ms"), "{}", d[0].message);
}

/// Was eine Aktivierung samt Fault-Pfad auf dem Ziel kostet, in ns
/// aufgerundet, und was die Aktivierung allein kostet.
fn activation_ns(p: &takt_mir::Program, t: &Target) -> (u64, u64) {
    let b = p.machines.iter().find(|m| m.name == "m").and_then(|m| m.budget).expect("Budget");
    let ns = |ps: u64| ps.div_ceil(1000);
    (ns(t.c_target.duration_ps(b.activation + b.fault_path)), ns(t.c_target.duration_ps(b.activation)))
}

/// Eine Maschine mit billigem Schritt und teurem Fault-Ziel.
fn costly_fault(wcet_ns: u64, every: &str) -> String {
    format!(
        "{KOPF}machine m{every} with budget = {{wcet = {wcet_ns} ns}}:\n    fault -> SAFE\n    \
         var acc : float = 0.0\n    var a : int in 0..9 = 1\n\n    initial S\n\n    state S:\n        loop:\n            \
         check a < 9, \"zu gross\"\n            led = true\n\n    state SAFE:\n        enter:\n            \
         for i in range(200):\n                acc = acc + 1.5\n            led = acc > 0.0\n"
    )
}

fn sc12(p: &takt_mir::Program, t: &Target) -> Vec<takt_diag::Diagnostic> {
    takt_sema::calibrated::check(p, t, Span::new(0, 0)).into_iter().filter(|d| d.code == "SC-12").collect()
}

/// **`wcet` gilt fuer Aktivierung und Fault-Pfad, je Aktivierung, ohne
/// `T_IO`** (7.2, 9.4.3): Ein `wcet`, das nur den Schritt deckt, verfehlt
/// das Budget; genau die Summe besteht, eine Nanosekunde weniger nicht —
/// auch bei `every 100 ms` und bei einem `T_IO` fast so gross wie der Tick.
#[test]
fn a_declared_wcet_covers_the_fault_path_per_activation_without_t_io() {
    let target = ziel();
    let (full, step) = activation_ns(&compile(&costly_fault(1_000_000, "")), &target);
    assert!(full > step + 100_000, "der Fault-Pfad ist teuer: {full} gegen {step} ns");
    let judged = |wcet: u64, every: &str, t: &Target| sc12(&compile(&costly_fault(wcet, every)), t);
    let d = judged(step + 1000, "", &target);
    assert!(d.iter().any(|d| d.is_error() && d.message.contains("je Aktivierung")), "nur der Schritt: {d:?}");
    assert!(judged(full, "", &target).is_empty(), "genau die Summe besteht");
    assert!(!judged(full - 1, "", &target).is_empty(), "eine Nanosekunde zu wenig");
    assert!(judged(full, " every 100 ms", &target).is_empty(), "je Aktivierung, nicht je Tick");
    assert!(!judged(full - 1, " every 100 ms", &target).is_empty());
    let mut slow_io = target.clone();
    slow_io.t_io_ps = 9_990_000_000;
    assert!(judged(full, "", &slow_io).is_empty(), "`T_IO` geht nicht ein");
}

/// **Die Stack-Schranke ist ein Posten der RAM-Summe** (Pruefung 12 und 39,
/// 12.3): Ohne den Stack der nativen Funktion passt das Programm, mit ihm
/// nicht; die Meldung nennt den Stackanteil.
#[test]
fn the_stack_bound_counts_towards_the_ram_limit() {
    let p = compile(&format!(
        "{KOPF}native fn crc32(b: bytes<16>) -> u32 with cost = {{native: 50}}, stack = 4096, total\n\n\
         machine m:\n    var b : bytes<16> = default\n    var c : u32 = 0\n\n    initial S\n\n    state S:\n        \
         loop:\n            c = crc32(b)\n            led = c > 0\n"
    ));
    let mut target = ziel();
    let total = takt_mir::analysis::size::size(&p).with_hardware(&target).ram_total();
    target.memory.ram = Some(total - 2048);
    let d: Vec<_> =
        takt_sema::calibrated::check(&p, &target, Span::new(0, 0)).into_iter().filter(|d| d.code == "SC-39").collect();
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].is_error() && d[0].message.contains("davon Stack 4096 Byte"), "{}", d[0].message);
    target.memory.ram = Some(total);
    assert!(takt_sema::calibrated::check(&p, &target, Span::new(0, 0)).iter().all(|d| d.code != "SC-39"));
}
