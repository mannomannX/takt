//! Das Messprogramm des Boardmodus (13.8) gegen die Konfigurationen der
//! Boards: Es uebersetzt fuer die Hardware, und jede seiner Adressen steht
//! dort mit Richtung und Typ (Pruefung 60).

use takt_diag::Policy;
use takt_sema::{Build, Options};

fn config(name: &str) -> takt_mir::hardware::Hardware {
    let path = format!("{}/../../corpus-try/hw/{name}.hw", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    takt_mir::hardware::parse(&text).unwrap_or_else(|e| panic!("{path}:{}: {}", e.line, e.message))
}

#[test]
fn the_probe_binds_to_every_board_configuration() {
    let options = Options { policy: Policy::default(), build: Build::Hw, profile: None, ..Default::default() };
    let out = takt_sema::compile(takt_conformance::wire::PROBE, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    for board in ["esp32c6", "stm32f401"] {
        let hw = config(board);
        let diags = takt_sema::calibrated::check_bindings(&program, &hw);
        let errors: Vec<String> = diags.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
        assert!(errors.is_empty(), "{board}: {}", errors.join("\n"));
        for address in [takt_conformance::wire::OUT, takt_conformance::wire::IN] {
            assert!(hw.channel(address).and_then(|c| c.port.as_ref()).is_some(), "{board}: `{address}` ohne Pin");
        }
    }
}
