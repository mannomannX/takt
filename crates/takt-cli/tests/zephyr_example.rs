//! Takt unter Zephyr (12.11, M11 Schritt 19): Das Modul (`zephyr/`) mit dem
//! Port mit Faeden und seinem Adapter fuer Zephyr baut `examples/zephyr` fuer
//! beide Boards. Was ohne Board zu pruefen ist, prueft dieser Test: Kconfig
//! loest auf, was der Port braucht, die Programmbibliothek, das SDK-Archiv
//! und der Port binden mit der GCC des Zephyr-SDK, und das Abbild liegt, wo
//! das Board es startet. Die Portpruefung auf den Boards fahren
//! `board_stm32f401.rs` und `board_esp32c6.rs` (`the_zephyr_port_passes_the_port_test`).
//!
//! Zephyr und sein SDK liegen ausserhalb des Baums (`D:\takt-tools`): Der
//! Test laeuft mit `--ignored`, `TAKT_WEST` nennt `west`, und die Umgebung
//! traegt, was `activate.ps1` setzt ([`takt_conformance::zephyr`]).

use std::path::{Path, PathBuf};

use takt_conformance::zephyr::{self, Board, ESP32C6, Image, STM32F401};
use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// Die Symbole, ohne die das Abbild das Pruefprogramm nicht ueber den Port
/// faehrt: die Tabelle des Programms, der Kern des SDK, der Port mit Faeden
/// und sein Adapter, die Probe der Partition.
const LINKED: &[&str] = &[
    "port_test_ops",
    "takt_core_init_journaled",
    "takt_core_service",
    "takt_rtos_start",
    "takt_os_spawn",
    "partition_probe",
    "main",
];

/// Was die Konfiguration eines Boards aufgeloest haben muss: das Modul, das
/// Tripel des SDK-Archivs, die Bedingungen des Ports und der Benutzermodus,
/// gegen den die Partition der Arena schuetzt.
fn configured(board: Board, image: &Image) -> Vec<String> {
    let triple = format!("\"{}\"", board.triple);
    let mut want = vec![("TAKT", "y"), ("TAKT_TARGET", triple.as_str()), ("USERSPACE", "y")];
    if board == STM32F401 {
        // Die FPU-Register jedes Fadens gesichert (12.11), die ABI des Tripels,
        // die Anwendung ab Sektor 4 hinter dem Sprungbrett.
        want.extend([("FPU_SHARING", "y"), ("FP_HARDABI", "y"), ("FLASH_LOAD_OFFSET", "0x10000")]);
    }
    want.into_iter()
        .filter(|(name, value)| image.config(name) != Some(value))
        .map(|(name, value)| format!("{}: CONFIG_{name} ist {:?}, verlangt {value}", board.west, image.config(name)))
        .collect()
}

/// Was im ELF stehen muss ([`LINKED`]).
fn linked(board: Board, image: &Image) -> Vec<String> {
    let target = if board == STM32F401 { Target::THUMBV7EM } else { Target::RISCV32IMAC };
    let Some(symbols) = Binutils::best_for(target).symbols(&image.elf) else {
        return vec![format!("{}: `nm` liest {} nicht", board.west, image.elf.display())];
    };
    let at = |name: &str| symbols.iter().find(|s| s.name == name).map(|s| s.address);
    let mut failed: Vec<String> = LINKED
        .iter()
        .filter(|name| at(name).is_none())
        .map(|name| format!("{}: `{name}` fehlt im Abbild", board.west))
        .collect();
    // Die Arena liegt in ihrer Partition (12.11, `examples/zephyr/src/partition.h`).
    let partition = (at("z_data_smem_takt_arena_part_start"), at("z_data_smem_takt_arena_part_end"), at("arena"));
    match partition {
        (Some(start), Some(end), Some(arena)) if (start..end).contains(&arena) => {}
        (start, end, arena) => failed.push(format!(
            "{}: die Arena ({arena:x?}) liegt nicht in der Partition `takt_arena` ({start:x?}..{end:x?})",
            board.west
        )),
    }
    failed
}

/// Das Rohabbild startet, wo das Board es sucht: Auf dem F401 beginnt es mit
/// der Vektortabelle der Anwendung in Sektor 4 (Stackzeiger im RAM,
/// Resetvektor ab 0x0801_0000 und Thumb), auf dem C6 mit dem Kopf eines
/// Abbilds fuer den ROM-Lader (0xE9).
fn placed(board: Board, image: &Image) -> Vec<String> {
    let bytes = match std::fs::read(&image.bin) {
        Ok(b) => b,
        Err(e) => return vec![format!("{}: {e}", image.bin.display())],
    };
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let ok = if board == STM32F401 {
        matches!((word(0), word(4)), (Some(sp), Some(reset))
            if (0x2000_0000..=0x2001_0000).contains(&sp) && (0x0801_0000..0x0804_0000).contains(&reset) && reset % 2 == 1)
    } else {
        bytes.first() == Some(&0xE9)
    };
    if ok {
        Vec::new()
    } else {
        vec![format!("{}: {} beginnt nicht, wo das Board startet", board.west, image.bin.display())]
    }
}

/// **Das Zephyr-Beispiel baut fuer beide Boards** (12.11, M11 Schritt 19):
/// `west build` mit dem Modul, `takt_add_program` und dem SDK-Archiv des
/// Ziels; Konfiguration, Symbole und Lage des Abbilds wie oben.
#[test]
#[ignore = "Zephyr: TAKT_WEST und die Umgebung aus activate.ps1 (D:/takt-tools); mit --ignored"]
fn the_zephyr_example_builds_for_both_boards() {
    let west = zephyr::west().expect("TAKT_WEST nennt west");
    let takt = Path::new(env!("CARGO_BIN_EXE_takt"));
    let mut failed = Vec::new();
    for board in [STM32F401, ESP32C6] {
        let dir = scratch(&format!("takt-zephyr-{}", board.triple));
        match zephyr::build(&west, board, &dir, takt) {
            Ok(image) => {
                failed.extend(configured(board, &image));
                failed.extend(linked(board, &image));
                failed.extend(placed(board, &image));
            }
            Err(e) => failed.push(e),
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
