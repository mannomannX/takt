//! `takt driver-test`, Wirtsmodus (Referenz 13.8): ein Treiber in Takt
//! gegen sein Geraetemodell; bestanden, wenn jede Reaktion ausgeloest ist.

use std::path::PathBuf;
use std::process::{Command, Output};

const PROGRAM: &str = "crates/takt-bringup-esp32c6/programs/test_uart_c6.takt";
const STIMULUS: &str = "crates/takt-bringup-esp32c6/programs/test_uart_c6.stim.trace";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_uart_driver_passes_with_every_reaction_triggered() {
    let out = takt(&["driver-test", PROGRAM, "--stim", STIMULUS]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("a_full_tx_fifo_faults_the_port_until_it_drains: PASS"), "{stdout}");
    assert!(stdout.contains("Treiber uart_port: Zustaende 2/2, Transitionen 2/2"), "{stdout}");
    // Die Break-Pruefung haengt an `RX_BREAK_IS_FAULT`; sie gehoert in eine
    // Kampagne und steht darum nur im Bericht.
    assert!(stdout.contains("Nicht erreicht:\n") && stdout.contains(": uart_port check\n"), "{stdout}");
}

#[test]
fn a_reaction_without_a_scenario_fails_the_driver() {
    // Ohne den Stimulus gibt niemand das Kommando `uart_reset`.
    let out = takt(&["driver-test", PROGRAM]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    assert!(stdout.contains(": uart_port Transition RUN->RUN\n"), "{stdout}");
    assert!(stdout.contains("FAIL: 1 Reaktionen des Treibers von keinem Szenario ausgeloest (13.8)"), "{stdout}");
}

#[test]
fn a_program_without_a_driver_is_refused() {
    let out = takt(&["driver-test", "corpus-try/38_scenarios.takt"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("keine `driver machine`"));
}

/// **Ein Treiber-Crate in Rust auf dem Wirt** (13.8, FB-285): Das
/// Pruefgeraet verletzt den Vertrag mit Absicht; jeder Verstoss steht mit
/// seiner Zeile aus 12.6 im Bericht, und der Treiber besteht nicht. Ohne
/// clang uebersprungen.
#[test]
fn a_driver_crate_is_judged_by_the_edge_on_the_host() {
    let found = takt_llvm::toolchain::find().path().cloned();
    let Some(_) = takt_testkit::require("clang", found, "`TAKT_CLANG` setzen oder LLVM installieren") else { return };
    let program = "crates/takt-conformance/tests/programs/driver_edge.takt";
    let out = takt(&["driver-test", "--crate", "crates/takt-driver-probe", program, "--ticks", "16"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    for line in [
        "Zeile 1  t=1 driver edge_a warped p",
        "Zeile 1  t=2 driver edge_a degraded window",
        "Zeile 6  t=4 runtime Driver o",
        "Zeile 2  t=5 driver edge_u degraded seq",
        "Zeile 2  t=7 driver edge_u degraded maxpt",
        "Zeile 5  t=9 stream pairs dropped=0 overflowed=0 malformed=1",
        "Zeile 2  t=11 driver edge_b degraded timestamp",
    ] {
        assert!(stdout.contains(line), "{line}:\n{stdout}");
    }
    assert!(stdout.contains("Verstoesse gegen den Treibervertrag (12.6)") && stdout.ends_with("FAIL\n"), "{stdout}");
}

/// Ein Treiber mit Geraetemodell (12.10), dessen Szenario jede Reaktion
/// ausloest: zwei Zustaende, zwei Transitionen, ein Handler.
const DEVICE: &str = r#"system:
    language = 1
    tick     = 10 ms

record Regs layout little:
    flags : u8 with bits:
        full  : bool at 0 ro
        clear : bool at 1 w1c

record RegsModel layout little:
    flags : u8 with bits:
        full  : bool at 0
        clear : bool at 1

port regs : Regs @ mmio(0x60000000)

output regs_r : RegsModel        @ sim("mmio/0x60000000/r")
input  regs_w : stream<bytes<4>> @ sim("mmio/0x60000000/w") with capacity = 8, max_rate = 200 Hz

output busy : bool @ hw("o/busy") with safe = false

driver machine dev:
    initial IDLE

    state IDLE:
        loop:
            busy = false

        on regs_w as w:
            busy = true

        when regs.flags.full: -> FULL

    state FULL:
        loop:
            busy = true
            regs.flags.clear = true

        when not regs.flags.full: -> IDLE

scenario "fills":
    initial RUN

    state RUN:
        sequence:
            regs_r.flags.full = true
            wait 30 ms
            regs_r.flags.full = false
            wait 30 ms
            verdict pass "voll und leer"
"#;

/// Das Programm, nach `edit` abgewandelt, in einem eigenen Verzeichnis.
fn device(name: &str, edit: impl Fn(&str) -> String) -> PathBuf {
    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-driver-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join("device.takt");
    std::fs::write(&file, edit(DEVICE)).expect("Programm");
    file
}

/// Eine Abwandlung des Programms.
type Edit = dyn Fn(&str) -> String;

/// 13.8: bestanden ist der Treiber nur, wenn kein Szenario scheitert und
/// jeder Zustand, jede Transition und jeder Handler erreicht ist. Je
/// Bedingung eine Variante, die genau sie verletzt.
#[test]
fn every_condition_of_passing_fails_the_driver_on_its_own() {
    let pass = device("pass", str::to_string);
    let out = takt(&["driver-test", pass.to_str().expect("Pfad")]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        stdout.contains("Treiber dev: Zustaende 2/2, Transitionen 2/2, Checks 0/0 (0 verletzt), Handler 1/1"),
        "{stdout}"
    );
    let cases: [(&str, &Edit, &[&str], &str); 4] = [
        ("fail", &|p| p.replace("verdict pass", "verdict fail"), &[], "fills: FAIL"),
        (
            "handler",
            &|p| p.replace("            regs.flags.clear = true\n", ""),
            &[],
            "FAIL: 1 Reaktionen des Treibers von keinem Szenario ausgeloest (13.8)",
        ),
        (
            "state",
            &|p| p.replace("regs_r.flags.full = true", "regs_r.flags.full = false"),
            &[],
            "FAIL: 4 Reaktionen des Treibers von keinem Szenario ausgeloest (13.8)",
        ),
        ("inconclusive", &str::to_string, &["--ticks", "2"], "fills: INCONCLUSIVE"),
    ];
    for (name, edit, extra, line) in cases {
        let file = device(name, edit);
        let path = file.to_str().expect("Pfad");
        let out = takt(&[&["driver-test", path][..], extra].concat());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success(), "{name} bestand:\n{stdout}");
        assert!(stdout.contains(line), "{name}: {line}\n{stdout}");
        let _ = std::fs::remove_dir_all(file.parent().expect("Verzeichnis"));
    }
    let _ = std::fs::remove_dir_all(pass.parent().expect("Verzeichnis"));
}

/// `--crate` nennt ein Verzeichnis mit `takt-drivers.toml` (12.6); fehlt das
/// eine oder das andere, sagt der Fehler, was fehlt.
#[test]
fn a_driver_crate_needs_a_directory_with_its_wiring() {
    let program = "crates/takt-conformance/tests/programs/driver_edge.takt";
    let none = takt(&["driver-test", "--crate", "gibtsnicht", program, "--ticks", "4"]);
    let stderr = String::from_utf8_lossy(&none.stderr);
    assert!(!none.status.success() && stderr.contains("takt driver-test: gibtsnicht:"), "{stderr}");
    let unwired = takt(&["driver-test", "--crate", "crates/takt-diag", program, "--ticks", "4"]);
    let stderr = String::from_utf8_lossy(&unwired.stderr);
    assert!(!unwired.status.success() && stderr.contains("keine Verdrahtung `takt-drivers.toml` (12.6)"), "{stderr}");
}
