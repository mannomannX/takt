//! Der Port der Interruptform in C (12.11, M11 Schritt 18b): Der C-Wirt der
//! Black Pill (`examples/boards/stm32f401`) bindet mit dem Port
//! (`ports/irq`), seinem Adapter fuer STM32F4 und dem SDK-Archiv fuer
//! `thumbv7em-none-eabihf` zu einem Abbild, wie ein C-Projekt es baut. Auf
//! jedem Wirt, ohne Board: Ob es auf dem Chip besteht, pruefen die
//! Board-Tests (`takt-conformance/tests/board_stm32f401.rs`).

use std::path::{Path, PathBuf};

use takt_conformance::board::stm32f401::C_HOST;
use takt_conformance::board::{Form, Options};
use takt_conformance::port;
use takt_llvm::toolchain::find;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// **Der C-Wirt der Interruptform bindet**, mit `-Werror` und gegen das
/// Linker-Skript des Boards, das Flash, RAM und den Stack aus der Lieferform
/// prueft: einmal mit dem Pruefprogramm der Portpruefung in Echtzeit (Jobs,
/// `persist`, das Journal im Flash), einmal mit einem Programm ohne Journal in
/// logischer Zeit (der Wirt ohne Geraet, die verlustfreie Leitung).
#[test]
fn the_c_interrupt_port_links_for_the_f401() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let tool = Path::new(env!("CARGO_BIN_EXE_takt"));
    let runs = [(port::CORPUS_NAME, Options::timed(port::TICKS)), ("28_scheduled.takt", Options::fresh(60))];
    for (name, options) in runs {
        let out = scratch(&format!("takt-irq-port-{}", name.trim_end_matches(".takt")));
        let program = root().join("corpus-try").join(name);
        let elf = C_HOST
            .link(tool, &program, &options.in_form(Form::Interrupt), &out, "0")
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(elf.is_file(), "{name}: kein Abbild unter {}", elf.display());
    }
}
