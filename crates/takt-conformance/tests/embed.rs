//! Takt als Baustein in einem fremden Programm (12.11, M11): was der Wirt
//! an seiner Umgebung aendern darf, ohne dass Takt anders rechnet.
//!
//! Jeder Lauf hier geht ueber den Produktrahmen mit dem Kern des SDK, wie
//! jeder Wirt ihn bindet ([`takt_conformance::testhost`], M11 Schritt 9,
//! FB-693): keine eigene Schleife, die den Kern nachahmt.

mod common;

use takt_conformance::run::{compare, f32_outputs, widen_f32};
use takt_conformance::stimulus::Stimulus;
use takt_conformance::testhost::Run;
use takt_llvm::toolchain::Clang;

/// MXCSR des Wirts: Flush-to-Zero, alle Ausnahmen maskiert,
/// Denormals-are-Zero — alles, was die IEEE-Vorgabe nicht ist —, je in
/// einem der vier Rundungsmodi (Bits 13 und 14): naechste, gegen minus
/// unendlich, gegen plus unendlich, gegen null.
const HOSTILE: [(&str, u32); 4] = [("RN", 0x9FC0), ("RD", 0xBFC0), ("RU", 0xDFC0), ("RZ", 0xFFC0)];

/// Faehrt `p` im Produktrahmen ueber `ticks` Ticks ohne Stimulus, gebaut gegen
/// die Hardware-Konfiguration `hardware` (8.10), und liefert seinen Trace.
fn product(
    clang: &Clang,
    p: &takt_mir::Program,
    label: &str,
    ticks: u64,
    hardware: Option<&takt_mir::hardware::Hardware>,
) -> String {
    common::run_product(clang, p, label, &Run { ticks, hardware, ..Run::default() })
        .unwrap_or_else(|e| panic!("{label}: {e}"))
}

/// Der Trace des Interpreters ueber `ticks` Ticks ohne Stimulus.
fn interpreted(p: &takt_mir::Program, ticks: u64) -> String {
    let options = takt_interp::RunOptions { ticks, ..Default::default() };
    takt_interp::run(p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render()
}

/// Programme ausserhalb der Suite `ieee`, deren Rahmen mit Stroemen,
/// geplanten Ausgaben, Monitoren und Schlaf mehr Felder der Arena fuehrt.
const AROUND: [&str; 4] = ["13_framing.takt", "28_scheduled.takt", "47_monitors.takt", "56_idle_timer.takt"];

/// **Der Wirt verstellt seine Umgebung, und Takt rechnet wie der
/// Interpreter** (4.2, 12.11; FB-382). Jeder Lauf der Programme der Suite
/// `ieee` und aus [`AROUND`] — still, mit Stimulus, mit den erzeugten
/// Eingaben und auf den Pfaden des Solvers ([`takt_conformance::cases`]) —
/// laeuft im Produktrahmen mit dem Kern des SDK (M11 Schritt 9) unter einem
/// eigenen Praefix, mit MXCSR aus [`HOSTILE`] reihum und der Arena vor
/// `init` mit 0xA5 gefuellt. Nach jedem Aufruf des SDK ist MXCSR wieder der
/// des Wirts, und der Trace gleicht dem des Interpreters bitgenau: Ein
/// Einstieg, der die Umgebung des Wirts erbt oder verstellt, oder ein Feld,
/// das `init` nicht setzt, fiele hier auf. Was im Produkt keinen Weg hat,
/// faehrt auch diese Probe nicht; `GAPS` in `differential.rs` fuehrt es.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "die Probe setzt MXCSR, das es nur auf x86-64 gibt")]
fn a_hostile_host_environment_changes_nothing() {
    let Some(clang) = common::clang() else { return };
    let queue =
        std::sync::Mutex::new(takt_conformance::suites::programs("ieee").into_iter().chain(AROUND).collect::<Vec<_>>());
    let failed = std::sync::Mutex::new(Vec::new());
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..(cores - 1).clamp(1, 4) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    failed.lock().expect("Befunde").extend(hostile_runs(&clang, name));
                }
            });
        }
    });
    let failed = failed.into_inner().expect("Befunde");
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Die Laeufe eines Programms unter verstellter Umgebung; je Abweichung eine
/// Meldung.
fn hostile_runs(clang: &Clang, name: &str) -> Vec<String> {
    let p = common::board::corpus(name);
    let f32s = f32_outputs(&p);
    let mut failed = Vec::new();
    for (i, case) in takt_conformance::cases::cases(name, &p).iter().enumerate() {
        let (mode, mxcsr) = HOSTILE[i % HOSTILE.len()];
        let label = format!("{name} ({}, {mode})", case.label);
        let stimulus = takt_interp::Trace::parse(&case.stimulus).expect("Stimulus");
        let Ok(inputs) = Stimulus::from_trace(&stimulus) else { continue };
        let run = Run {
            ticks: case.ticks,
            stimulus: &inputs,
            prefix: Some("wirt"),
            mxcsr: Some(mxcsr),
            fill: 0xA5,
            ..Run::default()
        };
        if takt_conformance::testhost::sources(&p, &run, takt_conformance::testhost::host_triple(), false).is_err() {
            continue;
        }
        let options = takt_interp::RunOptions { ticks: case.ticks, inputs: true, ..Default::default() };
        let interpreted = match takt_interp::run(&p, &stimulus, &options) {
            Ok(r) => widen_f32(&r.trace.render(), &f32s),
            Err(e) => {
                failed.push(format!("{label}: der Interpreter bricht ab: {e:?}"));
                continue;
            }
        };
        match common::run_product(clang, &p, name, &run) {
            Ok(native) => {
                let diffs = compare(&interpreted, &native);
                if !diffs.is_empty() {
                    let shown: Vec<String> = diffs.iter().take(6).map(ToString::to_string).collect();
                    failed.push(format!("{label}: {} Abweichungen\n{}", diffs.len(), shown.join("\n")));
                }
            }
            Err(e) => failed.push(format!("{label}: {e}")),
        }
    }
    failed
}

/// **Ein Ausgangstreiber bekommt die Grenze des Ticks, den er stellt, auch
/// um einen Schlaf** (8.10, 9.9, 12.6; FB-371, FB-693). `56_idle_timer`
/// schlaeft zwischen seinen Phasen. Vor einer Zeile `runtime Driver led`
/// bestaetigt der Treiber von `ui/led` den Commit des Ticks davor nicht, und
/// der Testwirt erkennt diesen Commit an der Zeit, die der Rahmen ihm nennt.
/// Nennte er nach dem Schlaf (Ticks 50 und 120) oder vor ihm (Tick 70) eine
/// andere, bestaetigte der Treiber, und der Fault fehlte im Produkt.
#[test]
fn an_output_driver_sees_the_tick_it_commits() {
    let Some(clang) = common::clang() else { return };
    let p = common::board::corpus("56_idle_timer.takt");
    for tick in [51, 71, 121] {
        let stimulus = takt_interp::Trace::parse(&format!("t={tick} runtime Driver led\n")).expect("Stimulus");
        let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
        let native = common::run_native_all_with(&clang, &p, "commit_now", 130, &inputs)
            .unwrap_or_else(|e| panic!("Tick {tick}: {e}"));
        let slept = native.lines().filter_map(|l| l.split_once(" slept=")).any(|(_, n)| n.trim() != "0");
        assert!(slept, "Tick {tick}: der Kern schlief nie:\n{native}");
        let options = takt_interp::RunOptions { ticks: 130, ..Default::default() };
        let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
        assert!(interpreted.contains(&format!("t={tick} fault m Runtime(Driver)")), "Tick {tick}:\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty(),
            "Tick {tick}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}"
        );
    }
}

/// Ein Programm, das in Tick 2 eine Ausgabe auf `at` plant (9.8); ein
/// Timing-Fault fuehrt nach `SAFE` (probe 9), sonst steht probe 2.
fn scheduled(at: &str) -> takt_mir::Program {
    let src = format!(
        "system:\n    language = 1\n    tick     = 1 ms\n\n\
         output o     : bool        @ hw(\"gpio/loop_out\") with safe = false\n\
         output probe : int in 0..9 @ hw(\"o/probe\")       with safe = 0\n\n\
         machine m:\n    fault -> SAFE\n    initial RUN\n\n\
         \x20   state RUN:\n        enter:\n            probe = 1\n        after 2 ms: -> GO\n\n\
         \x20   state GO:\n        enter:\n            at {at}:\n                o = true\n            probe = 2\n\n\
         \x20   state SAFE:\n        enter:\n            probe = 9\n"
    );
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    out.program.unwrap_or_else(|| panic!("{at}: {:?}", out.diagnostics))
}

/// **Die Grenze von `guard` im Produktrahmen** (9.8: `T <= now + guard` ist
/// ein Timing-Fault; KON2-009, FB-693). Ohne Konfiguration ist `guard` null
/// wie in der Simulation: `at` genau jetzt faultet, eine Nanosekunde spaeter
/// nicht — im Rahmen wie im Interpreter. Mit `guard_ns = 4338` liegt die
/// Grenze 4338 ns weiter, und dieselbe Regel gilt dort.
#[test]
fn the_guard_boundary_faults_at_equality() {
    let Some(clang) = common::clang() else { return };
    for (at, want) in [("2 ms", "9"), ("2000001 ns", "2")] {
        let p = scheduled(at);
        let native = product(&clang, &p, &format!("guard-{want}"), 8, None);
        let interpreted = interpreted(&p, 8);
        assert!(interpreted.contains(&format!("t=2 out probe {want}")), "{at}:\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{at}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    }
    let hw = takt_mir::hardware::parse("# takt-hw 9\n[channel gpio/loop_out]\nguard_ns = 4338\n").expect("lesbar");
    for (at, want) in [("2004338 ns", "9"), ("2004339 ns", "2")] {
        let p = scheduled(at);
        let native = product(&clang, &p, &format!("guard-hw-{want}"), 8, Some(&hw));
        assert!(native.contains(&format!("t=2 out probe {want}")), "guard 4338, `at {at}`:\n{native}");
    }
}

/// **Eine geplante Ausgabe haelt den Rahmen wach** (9.9, Konjunkt
/// „keine geplanten Ausgaben"; KON2-009, FB-693). Die Maschine steht im
/// `idle`-Zustand und hat `o` fuer 5 ms geplant; ihre Frist liegt bei 20 ms.
/// Schliefe der Kern bis zur Frist, kaeme die Ausgabe zu spaet; sie steht
/// im selben Tick wie im Interpreter.
#[test]
fn a_scheduled_output_keeps_the_frame_awake() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick     = 1 ms\n\n\
               output o     : bool        @ hw(\"gpio/loop_out\") with safe = false\n\
               output probe : int in 0..9 @ hw(\"o/probe\")       with safe = 0\n\n\
               machine m:\n    initial WAIT\n\n\
               \x20   state WAIT idle:\n        enter:\n            at 5 ms:\n                o = true\n\
               \x20       after 20 ms: -> DONE\n\n\
               \x20   state DONE:\n        enter:\n            probe = 3\n";
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let native = product(&clang, &p, "sched-awake", 30, None);
    let interpreted = interpreted(&p, 30);
    assert!(interpreted.contains("t=5 out o true") && interpreted.contains("t=20 out probe 3"), "{interpreted}");
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **`o.jitter` ohne Tickraster** (7.5; KON2-009, FB-693): Mit
/// `tick_granular = false` ist der Jitter der gemessene, ohne den Tick dazu;
/// der Rahmen liefert ihn dem Programm im Lauf.
#[test]
fn the_jitter_without_tick_granularity_is_the_measured_one() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick = 1 ms\n\n\
               output probe  : bool     @ hw(\"gpio/loop_out\") with safe = false\n\
               output spread : Duration @ hw(\"o/spread\")      with safe = 0 s\n\n\
               machine m:\n    initial RUN\n    state RUN:\n        loop:\n            spread = probe.jitter\n";
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    for (granular, want) in [("false", "36563 ns"), ("true", "1036563 ns")] {
        let hw = takt_mir::hardware::parse(&format!(
            "# takt-hw 9\n[channel gpio/loop_out]\njitter_ns = 36563\ntick_granular = {granular}\n"
        ))
        .expect("lesbar");
        let native = product(&clang, &p, &format!("jitter-{granular}"), 3, Some(&hw));
        assert!(native.contains(&format!("out spread {want}")), "tick_granular = {granular}:\n{native}");
    }
}
