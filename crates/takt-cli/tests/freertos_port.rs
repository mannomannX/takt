//! Der Port mit Faeden unter FreeRTOS (12.11, M11 Schritt 20): Der C-Wirt
//! der Black Pill (`examples/boards/stm32f401/freertos.c`) bindet mit dem Port
//! `ports/rtos`, seinem Adapter `ports/freertos`, dem Kern von FreeRTOS
//! V11.3.1 (`FREERTOS_KERNEL_PATH`) und dem SDK-Archiv fuer
//! `thumbv7em-none-eabihf` zu einem Abbild, wie ein C-Projekt es baut; ohne
//! MPU und als FreeRTOS-MPU (Schritt 20b). Auf jedem Wirt, ohne Board: Ob es
//! auf dem Chip besteht, pruefen die Board-Tests
//! `the_freertos_port_passes_the_port_test`,
//! `the_freertos_mpu_port_passes_the_port_test` und
//! `the_freertos_mpu_turns_a_foreign_write_into_runtime_hardware`.

use std::path::{Path, PathBuf};

use takt_conformance::board::c_host::CHost;
use takt_conformance::board::stm32f401::{C_HOST, C_HOST_MPU, C_HOST_MPU_PROBE};
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

/// **Der C-Wirt unter FreeRTOS bindet**, mit `-Werror` ueber Wirt, Port,
/// Adapter und Kern, gegen das Linker-Skript des Boards: mit dem
/// Pruefprogramm der Portpruefung in Echtzeit (Jobs, `persist`, das Journal im
/// Flash) und mit einem Programm ohne Journal; unter FreeRTOS-MPU passen
/// Code und Daten des Kerns in ihre Regionen, sonst bricht der Linker ab.
#[test]
fn the_freertos_port_links_for_the_f401() {
    if takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren").is_none() {
        return;
    }
    let kernel = std::env::var_os("FREERTOS_KERNEL_PATH");
    let hint = "`FREERTOS_KERNEL_PATH` auf den Kern V11.3.1 setzen (lokal `D:\\takt-tools\\activate`, auf CI die Setup-Aktion)";
    if takt_testkit::require("freertos", kernel, hint).is_none() {
        return;
    }
    let tool = Path::new(env!("CARGO_BIN_EXE_takt"));
    let hosts: [&CHost; 3] = [&C_HOST, &C_HOST_MPU, &C_HOST_MPU_PROBE];
    let runs = [(port::CORPUS_NAME, Options::timed(port::TICKS)), ("28_scheduled.takt", Options::timed(60))];
    for host in hosts {
        for (name, options) in &runs {
            let out = scratch(&format!("takt-freertos-{}-{}", host.name, name.trim_end_matches(".takt")));
            let program = root().join("corpus-try").join(name);
            let elf = host
                .link(tool, &program, &options.clone().in_form(Form::Rtos), &out, "0")
                .unwrap_or_else(|e| panic!("{} {name}: {e}", host.name));
            assert!(elf.is_file(), "{} {name}: kein Abbild unter {}", host.name, elf.display());
        }
    }
}
