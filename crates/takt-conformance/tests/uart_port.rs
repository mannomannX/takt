//! Die Portierung des Treiberstapels auf ein zweites Board (12.10).
//!
//! **Was hier belegt wird.** `feedback/test_uart.takt` und
//! `crates/takt-bringup-esp32c6/programs/test_uart_c6.takt` sind derselbe
//! Stapel auf zwei Registerbildern. Geaendert ist genau eine Schicht: L0,
//! die Registerzugriffe. L1 (COBS), L2 (Link) und L3 (Zeilen) stehen Zeile
//! fuer Zeile gleich — und genau das ist die Aussage der Treiberstufe: Ein
//! Protokollstapel ueberlebt den Boardwechsel, wenn die Sprache den
//! Registerzugriff traegt.
//!
//! Der Test vergleicht die portable Mitte beider Dateien Zeile fuer Zeile.
//! Wer L1 aendert und L0 vergisst, faellt hier auf; wer beide aendert, hat
//! den Stapel bewusst weiterentwickelt und zieht den Test nach.

use std::path::PathBuf;

use takt_diag::Policy;
use takt_interp::{CoverKind, Coverage, RunOptions, Trace, Verdict};
use takt_mir::Program;
use takt_mir::machine::MachineKind;

/// Genug Ticks fuer jedes Szenario; das laengste braucht 325.
const TICKS: u64 = 1_000;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

const PORTED: &str = "crates/takt-bringup-esp32c6/programs/test_uart_c6.takt";

fn ported() -> Program {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&read(PORTED), &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

/// Die Szenarien des Programms, je ein Lauf ueber `stimulus`.
fn scenario_runs(program: &Program, stimulus: &Trace) -> Vec<(String, takt_interp::RunResult)> {
    program
        .machines
        .iter()
        .filter(|m| m.kind == MachineKind::Scenario)
        .map(|m| {
            let options = RunOptions { ticks: TICKS, scenario: Some(m.name.clone()), ..Default::default() };
            (m.name.clone(), takt_interp::run(program, stimulus, &options).expect("Lauf"))
        })
        .collect()
}

/// Der Teil zwischen zwei Abschnittsueberschriften, ohne sie.
fn section(text: &str, from: &str, to: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| l.starts_with(from)).unwrap_or_else(|| panic!("fehlt: {from}"));
    let end = lines.iter().position(|l| l.starts_with(to)).unwrap_or_else(|| panic!("fehlt: {to}"));
    assert!(start < end, "{from} steht nach {to}");
    lines[start..end].iter().map(|l| l.to_string()).collect()
}

/// **L1 bis L3 sind in beiden Fassungen dieselben Zeilen.**
#[test]
fn only_the_driver_layer_differs_between_the_two_boards() {
    let original = read("feedback/test_uart.takt");
    let ported = read(PORTED);

    let a = section(&original, "# 3. L1 —", "# 7. Simulationsmodell");
    let b = section(&ported, "# 3. L1 —", "# 7. Simulationsmodell");

    assert!(a.len() > 300, "der portable Teil ist zu klein: {} Zeilen", a.len());
    assert_eq!(a.len(), b.len(), "verschieden lang: {} gegen {}", a.len(), b.len());

    let diff: Vec<String> = a
        .iter()
        .zip(&b)
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .map(|(i, (x, y))| format!("  Zeile {i}:\n    Original: {x}\n    Port:     {y}"))
        .collect();
    assert!(diff.is_empty(), "L1-L3 weichen ab:\n{}", diff.join("\n"));
}

/// **Die Portierung nennt die echten Register.** Vier Ports an den
/// Adressen, die die PAC des ESP32-C6 fuehrt — nicht an erfundenen.
#[test]
fn the_ported_driver_names_the_real_uart0_registers() {
    let program = ported();

    let mut addresses: Vec<u64> = program.ports.iter().map(|p| p.address).collect();
    addresses.sort_unstable();
    // FIFO, INT_RAW, INT_CLR, STATUS.
    assert_eq!(addresses, [0x6000_0000, 0x6000_0004, 0x6000_0010, 0x6000_001C]);

    // Jeder Port gehoert einer `driver machine` (Pruefung 64).
    for port in &program.ports {
        let owner = port.owner.unwrap_or_else(|| panic!("`{}` ohne Besitzer", port.name));
        assert!(program.machines[owner.index()].driver, "`{}` gehoert keiner `driver machine`", port.name);
    }
}

/// **Jedes Szenario besteht gegen das Modell des C6.** Ohne diese
/// Zusicherung pruefte der Test oben nur, dass zwei Dateien gleich
/// aussehen — nicht, dass der Stapel laeuft.
#[test]
fn the_ported_stack_passes_its_scenarios() {
    let runs = scenario_runs(&ported(), &Trace::default());
    assert_eq!(runs.len(), 9, "{:?}", runs.iter().map(|(n, _)| n).collect::<Vec<_>>());
    for (name, result) in runs {
        assert_eq!(result.verdict, Verdict::Pass, "{name}:\n{}", result.trace.render());
    }
}

/// **Jede Reaktion des Treibers wird einmal ausgeloest** (13.8, was
/// `takt driver-test` verlangt): Das Modell speist jeden Leitungsfehler
/// ein, der Stimulus den Reset, und ueber alle Szenarien erreicht
/// `uart_port` jeden Zustand und jede Transition. Die Stall-Pruefung ist
/// dabei verletzt worden — frueher verschluckte die Sendepumpe bei voller
/// FIFO ein Byte je Tick (8.6: untersucht heisst konsumiert), und der
/// Stall kam nie zustande.
#[test]
fn every_reaction_of_the_driver_is_triggered() {
    let program = ported();
    let stimulus =
        Trace::parse(&read("crates/takt-bringup-esp32c6/programs/test_uart_c6.stim.trace")).expect("Stimulus");
    let mut coverage = Coverage::default();
    for (name, result) in scenario_runs(&program, &stimulus) {
        assert_eq!(result.verdict, Verdict::Pass, "{name}:\n{}", result.trace.render());
        coverage.merge(&result.coverage);
    }
    let items: Vec<_> =
        takt_interp::coverage::items(&program).into_iter().filter(|i| i.machine == "uart_port").collect();
    let missing: Vec<&str> =
        coverage.missing(&items).into_iter().filter(|i| i.kind != CoverKind::Check).map(|i| i.key.as_str()).collect();
    assert!(missing.is_empty(), "nicht ausgeloest: {missing:?}");
    let src = read(PORTED);
    let stall = items
        .iter()
        .find(|i| src[i.span.start as usize..].starts_with("check tx_stalled_for"))
        .expect("Stall-Pruefung");
    assert!(coverage.has(CoverKind::CheckFailed, stall), "die Stall-Pruefung wurde nie verletzt");
}
