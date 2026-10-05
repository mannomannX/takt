//! Findet und benutzt die LLVM-Werkzeuge der aktiven Toolchain.
//!
//! Sie ersparen M5 die Cross-binutils: Ein `llvm-objdump` liest ARM,
//! ARM64 und RISC-V, waehrend die GNU-Kette je Ziel eine eigene braucht
//! (`arm-none-eabi-*`, `riscv32-unknown-elf-*`). Dieselbe LLVM-Version,
//! die den Code erzeugt hat, liest ihn damit auch wieder.

use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

/// Wo `rustup component add llvm-tools` gelaufen ist, findet sie das
/// Modul auch — unabhaengig davon, wo `RUSTUP_HOME` liegt.
///
/// Das ist der Grund, warum die Suche ueber `rustc --print sysroot`
/// laeuft und nicht ueber einen geratenen Pfad unter `~/.rustup`: Ein
/// Arbeitsplatz, der die Toolchain auf ein anderes Laufwerk legt, ist
/// nicht die Ausnahme.
#[test]
fn the_llvm_tools_are_found_when_installed() {
    let Some(tools) = takt_testkit::require("llvm-tools", Binutils::llvm(), "`rustup component add llvm-tools`") else {
        return;
    };
    assert!(tools.available(), "gefunden, aber nicht ausfuehrbar");
}

/// GEN-015: Jedes Ziel bekommt Werkzeuge, auch ohne installierte
/// GNU-Kette, und sie lesen ein Objekt dieses Ziels: Abschnitte und
/// Symbole kommen heraus.
#[test]
fn every_target_gets_tools() {
    let Some(_) = takt_testkit::require("llvm-tools", Binutils::llvm(), "`rustup component add llvm-tools`") else {
        return;
    };
    let Some(clang) =
        takt_testkit::require("clang", takt_llvm::toolchain::find().path().cloned(), "`TAKT_CLANG` setzen")
    else {
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-llvm-tools");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    for t in Target::ALL {
        let tools = Binutils::best_for(t);
        assert!(tools.available(), "{}: LLVM ist da, also muessen es die Werkzeuge auch sein", t.name);
        let (ll, obj) = (dir.join(format!("{}.ll", t.name)), dir.join(format!("{}.o", t.name)));
        let ir = format!("target triple = \"{}\"\n\ndefine i32 @takt_probe(i32 %x) {{\n  ret i32 %x\n}}\n", t.triple);
        std::fs::write(&ll, ir).expect("IR");
        let mut cmd = std::process::Command::new(&clang);
        cmd.args(["-c", "-Wno-override-module"]).arg(format!("--target={}", t.triple));
        if !t.march.is_empty() {
            cmd.arg(format!("-march={}", t.march));
        }
        let ok = cmd.arg(&ll).arg("-o").arg(&obj).status().is_ok_and(|s| s.success());
        assert!(ok, "{}: clang uebersetzt das Probeobjekt nicht", t.name);
        let sections = tools.sections(&obj).unwrap_or_else(|| panic!("{}: keine Abschnitte", t.name));
        assert!(sections.text > 0, "{}: `.text` leer: {sections:?}", t.name);
        let symbols = tools.symbols(&obj).unwrap_or_else(|| panic!("{}: keine Symbole", t.name));
        assert!(symbols.iter().any(|s| s.name.ends_with("takt_probe")), "{}: {symbols:?}", t.name);
    }
}
