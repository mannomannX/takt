//! Der MCU-Rahmen uebersetzt zusammen mit dem erzeugten Code (M5, 12.1).
//!
//! **Was hier belegt wird.** `mcu_codegen.rs` zeigt, dass der Codegen
//! Objekte fuer die MCU-Ziele erzeugt. Das genuegt nicht: Ein Objekt ohne
//! Aufrufer laeuft nicht. Dieser Test setzt beides zusammen — erzeugten
//! Code und Rahmen — und laesst den Linker urteilen. Was er annimmt, hat
//! alle Symbole; was ihm fehlt, nennt er.
//!
//! Das ist die Stufe vor dem Hardwarelauf: Ein Programm, das linkt,
//! laesst sich flashen.

use takt_llvm::Target;
use takt_llvm::toolchain::Clang;

mod common;

const KORPUS: [&str; 3] = ["01_minimal.takt", "16_timing.takt", "19_faults.takt"];

fn corpus(name: &str) -> takt_mir::Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    takt_sema::compile(&src, &options).program.unwrap_or_else(|| panic!("{path}: uebersetzt nicht"))
}

/// Uebersetzt Programm und Rahmen fuer ein Ziel und linkt beides.
///
/// Der Linker laeuft mit `-r` (teilweise Bindung): Ein vollstaendiges
/// Binary braeuchte Startcode und Linker-Skript, die zum Board gehoeren
/// und nicht hierher. Was `-r` prueft, ist genau die Frage dieses Tests —
/// passen die Symbole zusammen?
fn link_for(clang: &Clang, target: Target, p: &takt_mir::Program, name: &str) -> Result<u64, String> {
    let Clang::At(path) = clang else {
        return Err("clang fehlt".into());
    };
    let dir = std::env::temp_dir().join(format!("takt-mcuh-{}-{}", target.name, name.replace('.', "_")));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let ll = dir.join("programm.ll");
    let c = dir.join("rahmen.c");
    let obj_ir = dir.join("programm.o");
    let obj_c = dir.join("rahmen.o");
    let linked = dir.join("zusammen.o");

    std::fs::write(&ll, common::ir_for(p, target.triple)).map_err(|e| e.to_string())?;
    std::fs::write(&c, takt_conformance::mcu::build(p).source).map_err(|e| e.to_string())?;

    for (src, out) in [(&ll, &obj_ir), (&c, &obj_c)] {
        let mut cmd = std::process::Command::new(path);
        let cmd = Clang::deterministic(&mut cmd)
            .args(["-Wno-override-module", "-O1", "-c", "-ffreestanding", "-nostdlib"])
            .arg(format!("--target={}", target.triple));
        if !target.march.is_empty() {
            cmd.arg(format!("-march={}", target.march));
        }
        let out = cmd.arg(src).arg("-o").arg(out).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("{}: {}", src.display(), String::from_utf8_lossy(&out.stderr)));
        }
    }

    // `-r`: teilweise Bindung, ohne Startcode. Sie beantwortet die Frage
    // dieses Tests — passen die Symbole? — ohne ein Linker-Skript zu
    // verlangen, das zum Board gehoert.
    let mut cmd = std::process::Command::new(path);
    let cmd = Clang::deterministic(&mut cmd).args(["-r", "-nostdlib"]).arg(format!("--target={}", target.triple));
    if !target.march.is_empty() {
        cmd.arg(format!("-march={}", target.march));
    }
    let out = cmd.arg(&obj_ir).arg(&obj_c).arg("-o").arg(&linked).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let size = std::fs::metadata(&linked).map_err(|e| e.to_string())?.len();
    let _ = std::fs::remove_dir_all(&dir);
    Ok(size)
}

/// **Erzeugter Code und Rahmen passen zusammen.**
///
/// Der Linker ist hier der Pruefer: Ein fehlendes Symbol nennt er beim
/// Namen, und genau das ist die Frage — ruft der Rahmen, was der Codegen
/// erzeugt, und stellt er bereit, was der Codegen ruft?
#[test]
fn the_harness_links_with_the_generated_code() {
    let clang = takt_llvm::toolchain::find();
    if matches!(clang, Clang::Missing) {
        eprintln!("clang fehlt; uebersprungen");
        return;
    }
    let mut errors = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        for target in [Target::THUMBV7EM, Target::RISCV32IMAC] {
            match link_for(&clang, target, &p, name) {
                Ok(size) => eprintln!("{name} fuer {}: {size} Byte gebunden", target.name),
                Err(e) => errors.push(format!("{name} fuer {}: {e}", target.name)),
            }
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n\n"));
}

/// Der Rahmen nennt die Funktionen, die die Tickschleife braucht.
///
/// `takt-rt-baremetal` ruft sie ueber den `Program`-Trait; fehlt eine,
/// faellt es erst beim Binden des Bring-up-Programms auf — also spaet.
#[test]
fn the_harness_exports_what_the_loop_needs() {
    let p = corpus("01_minimal.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for name in ["takt_mcu_init", "takt_mcu_tick", "takt_mcu_dump"] {
        assert!(src.contains(&format!("void {name}")), "`{name}` fehlt im Rahmen");
    }
}

/// Der Rahmen bedient jede Funktion, die der erzeugte Code ruft.
///
/// Die ABI-Liste (`takt-llvm/src/abi.rs`) ist die eine Stelle, an der
/// beides zusammensteht; sie war schon einmal unvollstaendig (FB-117).
#[test]
fn the_harness_answers_the_whole_abi() {
    let p = corpus("19_faults.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for name in ["takt_now", "takt_alert", "takt_log", "takt_measure", "takt_verify", "takt_abort", "takt_verdict"] {
        assert!(src.contains(name), "`{name}` fehlt im Rahmen — der erzeugte Code ruft es (abi.rs)");
    }
    assert!(src.contains("takt_fn_fault"), "das Fault-Flag reiner Funktionen fehlt (4.1)");
}

/// **Kein `stdio`, kein Heap.**
///
/// 12.3 verlangt beides: Auf der MCU gibt es keine libc, und „gesamter
/// Zustand statisch in `.bss`; kein Heap". Ein Rahmen, der `printf`
/// ruft, linkt dort nicht — und ein `malloc` waere ein Verstoss gegen
/// die Zusage der Sprache, nicht nur gegen eine Konvention.
#[test]
fn the_harness_is_freestanding() {
    let p = corpus("16_timing.takt");
    let src = takt_conformance::mcu::build(&p).source;
    for forbidden in ["stdio.h", "printf", "malloc", "stdlib.h"] {
        assert!(!src.contains(forbidden), "`{forbidden}` gehoert nicht in einen MCU-Rahmen (12.3)");
    }
    assert!(src.contains("unsigned char image"), "das Prozessabbild steht statisch");
}

/// **Jeder ABI-Puffer ist auf acht Byte ausgerichtet.**
///
/// Der erzeugte Code sieht diese Puffer als Strukturen mit `i64`-Feldern
/// und greift darauf mit `LDRD` zu. Ein `unsigned char[]` hat in C aber
/// Ausrichtung 1, und der Linker legt es dahin, wo Platz ist: Auf einem
/// STM32F401 landete `state_blink` so auf 0x2000_0036, und das erste
/// `LDRD` loeste einen UsageFault aus, der zum HardFault eskalierte.
///
/// **Der Wirt kann diesen Fehler nicht finden**, weil x86-64
/// unausgerichtete Zugriffe traegt — darum steht die Pruefung am Text des
/// Rahmens und nicht an einem Lauf. 12.8 trennt die Zielklassen aus genau
/// diesem Grund.
#[test]
fn every_abi_buffer_is_aligned() {
    for name in ["16_timing.takt", "19_faults.takt", "29_heartbeat.takt"] {
        let p = corpus(name);
        let src = takt_conformance::mcu::build(&p).source;
        for line in src.lines().filter(|l| l.starts_with("static") && l.contains('[')) {
            assert!(line.contains("_Alignas(8)"), "{name}: unausgerichteter Puffer: {line}");
        }
    }
}
