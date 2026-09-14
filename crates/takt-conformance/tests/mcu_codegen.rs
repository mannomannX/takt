//! Erzeugt der Codegen brauchbaren Code fuer die MCU-Ziele? (M5, 12.8)
//!
//! **Die Luecke, die dieser Test schliesst.** `Target::THUMBV7EM` und
//! `Target::RISCV32IMAC` stehen seit M5 in der Zielliste, und ein
//! Bring-up-Programm laeuft auf echter Hardware — aber das ist *Rust*.
//! Kein Takt-Programm war je fuer eine MCU uebersetzt worden; dass die IR
//! zielunabhaengig ist, war eine Annahme, kein Befund.
//!
//! Der Test nimmt sie ernst: Er uebersetzt Korpusprogramme fuer beide
//! MCU-Ziele und assembliert sie. Was dabei schiefgeht, faellt hier auf
//! und nicht erst, wenn jemand ein Board anschliesst.
//!
//! **Was er nicht prueft.** Ausfuehren kann er nicht — dafuer braeuchte es
//! einen Emulator oder Hardware mit Debugger. Die Bit-Gleichheit ueber
//! Zielklassen hinweg (Satz 9.4.4) bleibt damit offen; dieser Test belegt
//! die Stufe davor: dass es ueberhaupt Code gibt, den man ausfuehren
//! koennte, und dass die IR fuer alle vier Ziele dieselbe ist.
//!
//! Die beiden Zielklassen unterscheiden sich (12.8): Cortex-M4F rechnet
//! `f32` in Hardware, RV32IMAC in Software. Beide Pfade entstehen hier
//! zum ersten Mal.

use takt_llvm::Target;
use takt_llvm::target::Class;
use takt_llvm::toolchain::Clang;

mod common;

/// Korpusprogramme, die fuer die MCU uebersetzen muessen.
///
/// Dieselben, die `targets.rs` fuer x86-64 und aarch64 fuehrt — was auf
/// der Box uebersetzt, muss es auch auf dem Chip.
const KORPUS: [&str; 5] =
    ["01_minimal.takt", "02_units_and_data.takt", "16_timing.takt", "19_faults.takt", "20_native.takt"];

fn corpus(name: &str) -> takt_mir::Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    out.program.unwrap_or_else(|| panic!("{path}: uebersetzt nicht"))
}

/// Uebersetzt ein Programm fuer ein Ziel und assembliert es.
fn compile_for(clang: &Clang, target: Target, p: &takt_mir::Program, name: &str) -> Result<u64, String> {
    let dir = std::env::temp_dir().join(format!("takt-mcu-{}-{}", target.name, name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ll = dir.join("programm.ll");
    let obj = dir.join("programm.o");
    std::fs::write(&ll, common::ir_for(p, target.triple)).map_err(|e| e.to_string())?;

    let Clang::At(path) = clang else {
        return Err("clang fehlt".into());
    };
    let mut cmd = std::process::Command::new(path);
    let cmd = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1", "-c"])
        .arg(format!("--target={}", target.triple))
        // Freistehend: Auf der MCU gibt es keine libc, und der erzeugte
        // Code ruft auch keine — was er braucht, steht in der ABI (11.2)
        // und kommt von der Runtime.
        .args(["-ffreestanding", "-nostdlib"]);
    if !target.march.is_empty() {
        cmd.arg(format!("-march={}", target.march));
    }
    let out = cmd.arg(&ll).arg("-o").arg(&obj).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let size = std::fs::metadata(&obj).map_err(|e| e.to_string())?.len();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(size)
}

/// **Der Korpus uebersetzt fuer beide MCU-Ziele.**
///
/// Die Stufe, die vor jedem Hardwarelauf steht: Ohne uebersetzbaren Code
/// gibt es nichts zu flashen.
#[test]
fn the_corpus_compiles_for_both_mcu_targets() {
    let clang = takt_llvm::toolchain::find();
    if matches!(clang, Clang::Missing) {
        eprintln!("clang fehlt; uebersprungen");
        return;
    }
    let mut errors = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        for target in [Target::THUMBV7EM, Target::RISCV32IMAC] {
            match compile_for(&clang, target, &p, name) {
                Ok(size) => eprintln!("{name} fuer {}: {size} Byte", target.name),
                Err(e) => errors.push(format!("{name} fuer {} ({}): {e}", target.name, target.class.name())),
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
}

/// Die vier Ziele der Abnahme erzeugen *dieselbe* IR.
///
/// **Das ist die Grundlage von Satz 9.4.4.** Waere die IR je Ziel eine
/// andere, waere Bit-Gleichheit eine Aussage ueber vier Programme statt
/// ueber vier Uebersetzungen desselben. Der Unterschied darf allein im
/// Triple stehen — und genau das prueft der Vergleich, indem er es
/// herausrechnet.
#[test]
fn every_target_gets_the_same_ir() {
    let p = corpus("16_timing.takt");
    let mut reference: Option<(String, &str)> = None;
    for target in Target::ALL {
        let ir = common::ir_for(&p, target.triple).replace(target.triple, "<triple>");
        match &reference {
            None => reference = Some((ir, target.name)),
            Some((first, first_name)) => {
                assert_eq!(&ir, first, "{} weicht von {first_name} ab", target.name);
            }
        }
    }
}

/// Die MCU-Ziele liegen in verschiedenen Zielklassen — und das ist der
/// Grund, warum gerade diese beiden das Paar bilden (12.8, plan/m5.md 2.1).
#[test]
fn the_mcu_targets_are_in_different_classes() {
    assert!(
        !Target::THUMBV7EM.same_class(Target::RISCV32IMAC),
        "M4F hat eine f32-FPU, RV32IMAC nicht — dieselbe Rechnung einmal in Hardware, einmal in Software"
    );
    assert_eq!(Target::THUMBV7EM.class, Class::Mcu32F32);
    assert_eq!(Target::RISCV32IMAC.class, Class::Mcu32NoFpu);
    assert!(Target::THUMBV7EM.is_bare_metal());
    assert!(!Target::X86_64_LINUX.is_bare_metal());
    // Beide 32-bittig: Die Darstellungsverengung (3.4) rechnet damit.
    assert_eq!(Target::THUMBV7EM.pointer, 4);
    assert_eq!(Target::RISCV32IMAC.pointer, 4);
}
