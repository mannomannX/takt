//! Szenarien nativ (13.6, Satz 9.4.4): `takt test` fuehrt jedes Szenario
//! als eigenen Lauf im Interpreter; der Rahmen treibt dieselben Maschinen
//! samt Szenario, und die Outputs stimmen bis zum Ende des Szenarios
//! ueberein — auch das Verdikt kommt nativ.

use takt_conformance::compare;
use takt_mir::machine::MachineKind;
use takt_mir::program::Program;

mod common;

const TICKS: u64 = 60;

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Der Tick einer Trace-Zeile.
fn tick_of(line: &str) -> u64 {
    line.strip_prefix("t=").and_then(|r| r.split_whitespace().next()).and_then(|t| t.parse().ok()).unwrap_or(0)
}

#[test]
fn every_scenario_runs_natively_like_takt_test() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("38_scenarios.takt");
    let scenarios: Vec<String> =
        p.machines.iter().filter(|m| m.kind == MachineKind::Scenario).map(|m| m.name.clone()).collect();
    assert!(!scenarios.is_empty(), "38_scenarios hat Szenarien");
    for name in &scenarios {
        let options = takt_interp::RunOptions { ticks: TICKS, scenario: Some(name.clone()), ..Default::default() };
        let result = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
        let interpreted = result.trace.render();
        // Der Interpreter endet mit dem Szenario (13.6); der Rahmen laeuft
        // die Ticks zu Ende, verglichen wird bis dorthin.
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        let native = common::run_native_scenario(&clang, &p, &format!("szenario_{name}"), name, TICKS)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let native: String = native.lines().filter(|l| tick_of(l) <= last).map(|l| format!("{l}\n")).collect();
        assert!(interpreted.contains("verdict"), "{name}: das Szenario hat nichts entschieden:\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(
            diffs.is_empty(),
            "{name}: {} Abweichungen:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
            diffs.len(),
            diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
            interpreted,
            native
        );
    }
}

/// Ein Programm mit einem Szenario je Ausgang aus 13.5: bestanden, `verify`
/// verletzt, `verdict fail`, `until … timeout` ohne Ziel, ein scheiterndes
/// `expect`, ein Fault unter `fault_is_fail = false` und ein Lauf ohne
/// Aussage (KON2-025).
const OUTCOMES: &str = "system:
    language = 1
    tick     = 1 ms

input  p     : float[bar] in 0..100 bar @ hw(\"daq/p\")   with max_age = 10 ms
output valve : bool                     @ hw(\"o/valve\") with safe = false
output p_sim : float[bar]               @ sim(\"daq/p\")  with safe = 1 bar

machine dut:
    initial CLOSED

    state CLOSED:
        loop:
            check p < 50 bar, \"overpressure\"

        when p > 5 bar: -> OPEN

    state OPEN:
        enter:
            valve = true

        when p < 2 bar: -> CLOSED

scenario \"passes\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            p_sim = 10 bar
            until dut.state == OPEN timeout 10 ms
            verify valve, \"valve should open\"
            verdict pass \"opened\"

scenario \"verify fails\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            p_sim = 1 bar
            wait 2 ms
            verify valve, \"valve should open\"
            verdict pass \"done\"

scenario \"verdict fails\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            wait 2 ms
            verdict fail \"by design\"

scenario \"until times out\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            p_sim = 1 bar
            until dut.state == OPEN timeout 5 ms
            verdict pass \"never\"

scenario \"expect fails\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            wait 2 ms
            expect dut.state == OPEN, \"must be open\"
            verdict pass \"never\"

scenario \"fault tolerated\" every 1 ms with fault_is_fail = false:
    initial RUN
    state RUN:
        sequence:
            p_sim = 80 bar
            wait 3 ms
            verdict pass \"the interlock tripped\"

scenario \"says nothing\" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            wait 3 ms
            p_sim = 1 bar
";

/// Das Endverdikt aus den Zeilen des Rahmens (13.5): FAIL nach einem
/// verletzten `verify`, einem `verdict fail` oder — wo Faults zaehlen — einem
/// Fault; PASS nur nach einem `verdict pass`; sonst INCONCLUSIVE.
fn final_verdict(native: &str, fault_is_fail: bool) -> &'static str {
    let words = |l: &str| l.split_whitespace().skip(1).map(str::to_string).collect::<Vec<_>>();
    let lines: Vec<Vec<String>> = native.lines().map(words).collect();
    let failed = lines.iter().any(|w| {
        matches!(w.first().map(String::as_str), Some("verify" | "verdict")) && w.get(3).is_some_and(|v| v == "0")
            || fault_is_fail && w.first().is_some_and(|k| k == "fault")
    });
    let passed = lines.iter().any(|w| w.first().is_some_and(|k| k == "verdict") && w.get(3).is_some_and(|v| v == "1"));
    if failed {
        "FAIL"
    } else if passed {
        "PASS"
    } else {
        "INCONCLUSIVE"
    }
}

/// **Jeder Ausgang eines Szenarios kommt nativ wie im Interpreter** (13.5,
/// 13.6; KON2-025): Die Zeilen `verify` und `verdict` vergleicht `compare`
/// mit Tick und Ausgang, und das Endverdikt, das aus den Zeilen des Rahmens
/// folgt, ist das des Interpreters.
#[test]
fn every_scenario_outcome_is_the_interpreters() {
    let Some(clang) = common::clang() else { return };
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(OUTCOMES, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let p = out.program.expect("Programm");
    for (name, want, fault_is_fail) in [
        ("passes", "PASS", true),
        ("verify_fails", "FAIL", true),
        ("verdict_fails", "FAIL", true),
        ("until_times_out", "FAIL", true),
        ("expect_fails", "FAIL", true),
        ("fault_tolerated", "PASS", false),
        ("says_nothing", "INCONCLUSIVE", true),
    ] {
        let options = takt_interp::RunOptions { ticks: TICKS, scenario: Some(name.to_string()), ..Default::default() };
        let result = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
        assert_eq!(result.verdict.name(), want, "{name}: der Interpreter");
        let interpreted = result.trace.render();
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        let native = common::run_native_scenario(&clang, &p, &format!("ausgang_{name}"), name, TICKS)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let native: String = native.lines().filter(|l| tick_of(l) <= last).map(|l| format!("{l}\n")).collect();
        let diffs = compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{name}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
        assert_eq!(final_verdict(&native, fault_is_fail), want, "{name}: das Endverdikt nativ:\n{native}");
    }
}
