//! Schritt 11: x86-64 ≡ aarch64 (13.8, Satz 9.4.4).
//!
//! **Was hier geprueft wird.** 9.4.4 verlangt bitidentische Ergebnisse
//! „ueber alle Targets". Bis hierher hiess das: Interpreter ≡ nativ auf
//! *einer* Maschine. Dieser Test schaerft es — dasselbe Programm,
//! uebersetzt fuer zwei Architekturen, ausgefuehrt auf beiden, liefert
//! Zeichen fuer Zeichen dasselbe.
//!
//! **Warum das mehr ist als ein weiterer Lauf.** x86-64 und aarch64
//! unterscheiden sich in der Byte-Reihenfolge nicht, wohl aber in
//! Registerbelegung, Befehlsauswahl und — das ist der Punkt — in ihren
//! Fliesskommabefehlen. Ein `fma`, das die eine Maschine hat und die
//! andere emuliert, waere eine andere Zahl. Dass beide dieselbe liefern,
//! ist die Zusage, auf der Simulation und Zertifizierung aufbauen.
//!
//! **Der Test braucht die Werkzeugkette.** Er ueberspringt sich ohne
//! clang und ohne `qemu-aarch64`; `tools/linux.sh` und
//! `tools/Dockerfile.linux` bringen beides mit.

use takt_llvm::Target;

mod common;

/// Die Korpusprogramme, die auf beiden Zielen laufen muessen.
///
/// Dieselbe Art Liste wie die Abnahme in `differential.rs`: Was auf
/// einer Architektur uebersetzt, muss auf der anderen dasselbe tun.
const KORPUS: [&str; 5] =
    ["01_minimal.takt", "02_units_and_data.takt", "16_timing.takt", "19_faults.takt", "20_native.takt"];

/// Die Referenzbeispiele, die auf beiden Zielen laufen muessen.
///
/// `plan.md` 6.2 macht sie zur Abnahme einer EX-ID: fertig heisst
/// „derselbe Trace auf jeder bis dahin unterstuetzten Zielklasse", und
/// M4 traegt zwei. Ohne sie hier waere der Exit fuer die halbe
/// Zielmenge behauptet statt belegt.
const EXAMPLES: [&str; 5] = ["14_1", "14_2", "14_3", "14_4", "14_5"];

const TICKS: u64 = 20;

/// Ein Referenzbeispiel aus `corpus-try/sim/`.
fn example(name: &str) -> takt_mir::Program {
    lade(&format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/sim/{}/program.takt"), name))
}

fn corpus(name: &str) -> takt_mir::Program {
    lade(&format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name))
}

fn lade(path: &str) -> takt_mir::Program {
    let path = path.to_string();
    let name = path.clone();
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.diagnostics.iter().any(|d| d.is_error()), "{name}");
    out.program.expect("Programm")
}

/// Ist die Werkzeugkette fuer aarch64 da?
fn cross_available() -> bool {
    std::process::Command::new("qemu-aarch64").arg("--version").output().is_ok_and(|o| o.status.success())
        && std::process::Command::new("aarch64-linux-gnu-gcc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Uebersetzt und laeuft ein Programm fuer ein Ziel.
/// Uebersetzt und laeuft ein Programm fuer ein Ziel.
///
/// `machine` nennt die eine Maschine, die getickt wird; `None` fuehrt
/// alle — noetig fuer Programme mit Plant-Modell (8.3), deren Eingaenge
/// sonst `Bad` bleiben.
fn run_for(target: Target, p: &takt_mir::Program, name: &str, machine: Option<&str>) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("takt-ziel-{}-{}", target.name, name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let exe = dir.join("lauf");

    // Dieselbe IR, nur mit anderem Triple: Das ist der Kern von 9.4.4.
    let ir = common::ir_for(p, target.triple);
    std::fs::write(&ll, &ir).map_err(|e| e.to_string())?;
    let h = match machine {
        Some(name) => takt_conformance::harness::build(p, name, TICKS),
        None => takt_conformance::harness::build_all(p, TICKS, &[]),
    };
    std::fs::write(&c, &h.source).map_err(|e| e.to_string())?;

    // Uebersetzt wird in zwei Schritten: clang macht aus der IR ein
    // Objekt (fuer beide Ziele derselbe Weg), der Linker der jeweiligen
    // Werkzeugkette bindet. Ein clang, der auch linkt, braeuchte fuer
    // aarch64 die Startdateien und `libgcc` der Cross-Kette — der
    // gcc-Treiber kennt sie ohnehin.
    let obj = dir.join("programm.o");
    let out = std::process::Command::new("clang")
        .args(["-Wno-override-module", "-O1", "-c"])
        .arg(format!("--target={}", target.triple))
        .arg(&ll)
        .arg("-o")
        .arg(&obj)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let linker = if target == Target::AARCH64_LINUX { "aarch64-linux-gnu-gcc" } else { "cc" };
    let out =
        std::process::Command::new(linker).arg(&c).arg(&obj).arg("-o").arg(&exe).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }

    let run = if target == Target::AARCH64_LINUX {
        let mut q = std::process::Command::new("qemu-aarch64");
        q.args(["-L", "/usr/aarch64-linux-gnu"]).arg(&exe);
        q.output()
    } else {
        std::process::Command::new(&exe).output()
    }
    .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&run.stdout).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(text)
}

/// **Die Abnahme von Schritt 11.** Dasselbe Programm liefert auf beiden
/// Architekturen dieselben Outputs (Satz 9.4.4).
#[test]
fn x86_64_and_aarch64_agree() {
    if !cross_available() {
        eprintln!("uebersprungen: aarch64-Werkzeugkette fehlt (tools/Dockerfile.linux baut sie)");
        return;
    }
    let mut errors = Vec::new();
    let mut checked = 0;
    // Die Korpusprogramme: je eine Maschine, wie in `differential.rs`.
    for name in KORPUS {
        let p = corpus(name);
        let Some(machine) = p.machines.first().map(|m| m.name.clone()) else { continue };
        if compare_all(name, &p, Some(&machine), &mut errors) {
            checked += 1;
        }
    }
    // Die Referenzbeispiele: alle Maschinen, weil fuenf von ihnen ein
    // Plant-Modell haben (8.3).
    for name in EXAMPLES {
        let p = example(name);
        if compare_all(name, &p, None, &mut errors) {
            checked += 1;
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
    assert_eq!(checked, KORPUS.len() + EXAMPLES.len(), "es wurden nicht alle Programme auf beiden Zielen geprueft");
}

/// Laeuft ein Programm auf beiden Zielen und vergleicht die Traces.
///
/// Liefert `true`, wenn der Vergleich zustande kam — ein Programm, das
/// sich nicht bauen laesst, ist ein Fehler und kein Vergleich.
fn compare_all(name: &str, p: &takt_mir::Program, machine: Option<&str>, errors: &mut Vec<String>) -> bool {
    let a = match run_for(Target::X86_64_LINUX, p, name, machine) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("{name} (x86-64): {e}"));
            return false;
        }
    };
    let b = match run_for(Target::AARCH64_LINUX, p, name, machine) {
        Ok(t) => t,
        Err(e) => {
            errors.push(format!("{name} (aarch64): {e}"));
            return false;
        }
    };
    if a != b {
        let erste = a.lines().zip(b.lines()).position(|(x, y)| x != y).unwrap_or(0);
        errors.push(format!(
            "{name}: die Ziele weichen ab, zuerst in Zeile {}\n  x86-64  {}\n  aarch64 {}",
            erste + 1,
            a.lines().nth(erste).unwrap_or(""),
            b.lines().nth(erste).unwrap_or("")
        ));
    }
    true
}

/// 12.8: Beide Ziele gehoeren derselben Zielklasse an — dieselbe Breite,
/// dieselbe FPU. Darum ist auch die IR dieselbe.
#[test]
fn both_targets_are_in_the_same_class() {
    assert!(Target::X86_64_LINUX.same_class(Target::AARCH64_LINUX));
    assert_eq!(Target::X86_64_LINUX.pointer, Target::AARCH64_LINUX.pointer);
}

/// Die IR unterscheidet sich nur im Triple.
///
/// Das ist die Bedingung dafuer, dass 9.4.4 eine Aussage ueber *eine*
/// Uebersetzung ist und nicht ueber zwei Programme.
#[test]
fn the_ir_differs_only_in_the_triple() {
    let p = corpus("01_minimal.takt");
    let a = common::ir_for(&p, Target::X86_64_LINUX.triple);
    let b = common::ir_for(&p, Target::AARCH64_LINUX.triple);
    let ohne = |s: &str| s.lines().filter(|l| !l.starts_with("target triple")).collect::<Vec<_>>().join("\n");
    assert_eq!(ohne(&a), ohne(&b), "die IR unterscheidet sich in mehr als dem Triple");
}
