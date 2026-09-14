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
    let Some(tools) = Binutils::llvm() else {
        eprintln!("llvm-tools fehlt; mit 'rustup component add llvm-tools' nachruesten");
        return;
    };
    assert!(tools.available(), "gefunden, aber nicht ausfuehrbar");
}

/// Jedes Ziel bekommt Werkzeuge, auch ohne installierte GNU-Kette.
#[test]
fn every_target_gets_tools() {
    let have_llvm = Binutils::llvm().is_some();
    for t in Target::ALL {
        let tools = Binutils::best_for(t);
        if have_llvm {
            assert!(tools.available(), "{}: LLVM ist da, also muessen es die Werkzeuge auch sein", t.name);
        }
    }
}
