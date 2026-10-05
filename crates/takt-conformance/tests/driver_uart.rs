//! Das Treiberbeispiel aus FB-182 als Abnahme (M8 Schritt 19).
//!
//! **Was hier belegt wird.** `feedback/test_uart.takt` ist das Programm,
//! an dem die Treiberstufe entworfen wurde: vier Registerports, eine
//! `driver machine`, ein COBS-Framer, ein Link mit ACK/NAK und
//! Retransmit, `resume` im Sendezustand, zwei Szenarien und ein
//! Geraetemodell. Bis v1.2 lief es nur als Probe ohne Ports.
//!
//! Der Test sagt dreierlei: Das Programm uebersetzt ohne Fehler, seine
//! Szenarien bestehen im Interpreter, und der Codegen bringt es ganz
//! durch — jeder Portzugriff als `volatile` an seiner Adresse (12.10).

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::machine::MachineKind;
use takt_sema::{Build, Options};

mod common;

/// Genug Ticks fuer beide Szenarien; das laengere braucht 31.
const TICKS: u64 = 600;

fn program() -> Program {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../feedback/test_uart.takt");
    let src = std::fs::read_to_string(path).expect("feedback/test_uart.takt lesbar");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

/// **Beide Szenarien bestehen.** Das ist die Abnahme: Nutzlast geht durch
/// vier Schichten hinaus, kommt ueber das Geraetemodell zurueck und
/// erreicht die Anwendung unversehrt.
#[test]
fn both_scenarios_pass_in_the_interpreter() {
    let p = program();
    let names: Vec<String> =
        p.machines.iter().filter(|m| m.kind == MachineKind::Scenario).map(|m| m.name.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");

    for name in names {
        let options = takt_interp::RunOptions { ticks: TICKS, scenario: Some(name.clone()), ..Default::default() };
        let result = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
        assert_eq!(
            result.verdict,
            takt_interp::Verdict::Pass,
            "{name}: {}\n{}",
            result.verdict.name(),
            result.trace.render()
        );
        // Ohne diese Zusicherung bestuende auch ein Lauf, in dem nie
        // etwas gesendet wurde.
        for p in &result.properties {
            assert!(!p.outcome.text().starts_with("verletzt"), "{name}: {} {}", p.name, p.outcome.text());
        }
    }
}

/// **Der Codegen bringt das ganze Programm durch.** Kein `NotYet`: Ports,
/// `driver machine`, `resume`, Bitfelder mit Zugriffsarten und die
/// Blockmethoden des Framers sind alle gesenkt.
#[test]
fn the_whole_program_lowers_to_native_code() {
    let p = program();
    let target = takt_llvm::Target::by_name("x86_64").expect("Ziel");
    let lowered = takt_llvm::lower::program_with(
        &p,
        target.triple,
        &takt_llvm::symbols::Prefix::default(),
        takt_llvm::Instrument::default_for(p.config.runtime_profile(), target),
    );

    // Ein uebersprungener Schritt ist ein Loch, kein Schoenheitsfehler:
    // Er meldet sich spaeter als unbekanntes Symbol beim Linker (FB-104).
    let skipped: Vec<String> = lowered.skipped.iter().map(|s| format!("{}: {}", s.machine, s.reason)).collect();
    assert!(
        skipped.is_empty(),
        "{}",
        skipped.join(
            "
"
        )
    );

    // 12.10: Jeder Portzugriff steht an seiner Adresse und ist auf der MCU
    // `volatile`, auf dem Wirt ein Aufruf der Runtime — sonst duerfte LLVM
    // zwei Registerlesevorgaenge zusammenfassen (FB-261).
    for address in [0x4000_1000u64, 0x4000_1004, 0x4000_1008, 0x4000_100C] {
        let want = format!("inttoptr i64 {address} to ptr");
        assert!(lowered.ir.contains(&want), "Portadresse {address:#x} fehlt im IR");
    }
    assert!(lowered.ir.contains("call void @app_mmio_read("), "kein Lesen ueber die Runtime im IR");
    assert!(lowered.ir.contains("call void @app_mmio_write("), "kein Schreiben ueber die Runtime im IR");
    let mcu =
        takt_llvm::lower::program(&p, takt_llvm::Target::RISCV32IMAC.triple, &takt_llvm::symbols::Prefix::default()).ir;
    assert!(mcu.contains("load volatile"), "kein `load volatile` im IR der MCU");
    assert!(mcu.contains("store volatile"), "kein `store volatile` im IR der MCU");
}

/// **Beide Szenarien laufen nativ wie im Interpreter** (12.10, 9.4.4;
/// KON1-019). Ports, `driver machine`, `resume` und Framer rechnet der
/// erzeugte Code mit dem Geraetemodell; verglichen werden Ausgaenge, Faults
/// und die Beobachtungen bis zum Ende des Szenarios — `verify` und `verdict`
/// zeigen, dass die Nutzlast durchging, und kaemen nativ nicht, wenn nicht.
#[test]
fn both_scenarios_run_natively_like_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    let p = program();
    assert!(!p.properties.is_empty(), "das Programm traegt seine Eigenschaften");
    for name in p.machines.iter().filter(|m| m.kind == MachineKind::Scenario).map(|m| m.name.clone()) {
        let options = takt_interp::RunOptions { ticks: TICKS, scenario: Some(name.clone()), ..Default::default() };
        let result = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf");
        let interpreted = result.trace.render();
        assert!(
            interpreted.contains(&format!("verdict {name} pass")),
            "{name}: kein bestandenes Verdikt:\n{interpreted}"
        );
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        let native = common::run_native_scenario(&clang, &p, &format!("uart_{}", name.replace(' ', "_")), &name, TICKS)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let tick_of = |l: &str| l.strip_prefix("t=").and_then(|r| r.split(' ').next()?.parse::<u64>().ok());
        let native: String =
            native.lines().filter(|l| tick_of(l).is_some_and(|t| t <= last)).map(|l| format!("{l}\n")).collect();
        assert!(native.contains(" verdict "), "{name}: nativ kein Verdikt:\n{native}");
        let diffs = takt_conformance::compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{name}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    }
}
