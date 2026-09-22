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

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
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
    let ported = read("crates/takt-bringup-esp32c6/programs/test_uart_c6.takt");

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
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let src = read("crates/takt-bringup-esp32c6/programs/test_uart_c6.takt");
    let out = takt_sema::compile(&src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    let program = out.program.expect("Programm");

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

/// **Beide Szenarien bestehen gegen das Modell des C6.** Ohne diese
/// Zusicherung pruefte der Test oben nur, dass zwei Dateien gleich
/// aussehen — nicht, dass der Stapel laeuft.
#[test]
fn the_ported_stack_passes_its_scenarios() {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let src = read("crates/takt-bringup-esp32c6/programs/test_uart_c6.takt");
    let program = takt_sema::compile(&src, &options).program.expect("Programm");

    let names: Vec<String> = program
        .machines
        .iter()
        .filter(|m| m.kind == takt_mir::machine::MachineKind::Scenario)
        .map(|m| m.name.clone())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    for name in names {
        let options = takt_interp::RunOptions { ticks: 600, scenario: Some(name.clone()), ..Default::default() };
        let result = takt_interp::run(&program, &takt_interp::Trace::default(), &options).expect("Lauf");
        assert_eq!(result.verdict, takt_interp::Verdict::Pass, "{name}:\n{}", result.trace.render());
    }
}
