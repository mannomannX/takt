//! Takt unter Zephyr (12.11, M11 Schritt 19): Das Modul (`zephyr/`) mit dem
//! Port mit Faeden baut `examples/zephyr` (Adapter auf den Faeden des
//! Zephyr-Kerns) fuer beide Boards, `examples/cmsis-rtos2` (Adapter auf
//! CMSIS-RTOS2) fuer den F401 und `examples/zephyr-gpio` (Treiber aus dem
//! Devicetree, 8.10) fuer beide Boards. Was ohne Board zu pruefen ist,
//! prueft dieser Test: Kconfig loest auf, was der Port braucht, die
//! Programmbibliothek, das
//! SDK-Archiv und der Port binden mit der GCC des Zephyr-SDK, und das Abbild
//! liegt, wo das Board es startet. Die Portpruefung auf den Boards fahren
//! `board_stm32f401.rs` und `board_esp32c6.rs`.
//!
//! Zephyr und sein SDK liegen ausserhalb des Baums (`D:\takt-tools`): Der
//! Test laeuft mit `--ignored`, `TAKT_WEST` nennt `west`, und die Umgebung
//! traegt, was `activate.ps1` setzt ([`takt_conformance::zephyr`]).

use std::path::{Path, PathBuf};

use takt_conformance::zephyr::{self, Board, CMSIS_RTOS2, ESP32C6, Image, STM32F401, Variant, ZEPHYR, ZEPHYR_GPIO};
use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// Was ein Beispiel verlangt: Symbole im Abbild, Werte der aufgeloesten
/// Konfiguration und ob die Arena in ihrer Partition liegt.
struct Expect {
    example: &'static str,
    linked: &'static [&'static str],
    config: &'static [(&'static str, &'static str)],
    partition: bool,
    /// Die Spielart des Baus; die Boards bauen die uebrigen.
    variant: Variant,
}

/// Der Kern des SDK und der Port mit Faeden: ohne sie faehrt kein Abbild ein
/// Programm.
const PORT: &[&str] = &["takt_core_service", "takt_rtos_start", "main"];

/// `examples/zephyr`: der Adapter auf den Faeden des Zephyr-Kerns, die
/// Probe der Partition und der Benutzermodus, gegen den sie schuetzt; mit
/// der Probe im Lauf, deren Zweig sonst nur die Board-Abbilder uebersetzten.
const ZEPHYR_EXPECT: Expect = Expect {
    example: ZEPHYR,
    linked: &["takt_os_spawn", "partition_probe", "port_test_ops", "takt_core_init_journaled", "takt_zephyr_nvm_open"],
    config: &[
        ("TAKT", "y"),
        ("TAKT_PORT_ZEPHYR", "y"),
        ("USERSPACE", "y"),
        ("TAKT_JOURNAL_FLASH", "y"),
        ("TAKT_TIMER_COUNTER", "y"),
    ],
    partition: true,
    variant: Variant::PROBE_IN_RUN,
};

/// `examples/cmsis-rtos2`: der Adapter auf CMSIS-RTOS2 unter Zephyrs
/// CMSIS-Schicht (M11 Schritt 19c).
const CMSIS_EXPECT: Expect = Expect {
    example: CMSIS_RTOS2,
    linked: &[
        "takt_os_spawn",
        "osThreadNew",
        "osSemaphoreAcquire",
        "osThreadGetStackSpace",
        "port_test_ops",
        "takt_core_init_journaled",
        "takt_zephyr_nvm_open",
    ],
    config: &[("TAKT", "y"), ("TAKT_PORT_CMSIS_RTOS2", "y"), ("CMSIS_RTOS_V2", "y"), ("TAKT_JOURNAL_FLASH", "y")],
    partition: false,
    variant: Variant::KEPT,
};

/// `examples/zephyr-gpio`: die Treiber beider Pins und der Heartbeat ihres
/// Geraets aus `wire_zephyr.c`, das `takt build --zephyr` schreibt (8.10).
const GPIO_EXPECT: Expect = Expect {
    example: ZEPHYR_GPIO,
    linked: &[
        "wire_ops",
        "takt_core_init",
        "takt_os_spawn",
        "wire_zephyr_init",
        "wire_in_wire_in",
        "wire_out_wire_out",
        "wire_alive_wire",
    ],
    config: &[("TAKT", "y"), ("TAKT_PORT_ZEPHYR", "y"), ("GPIO", "y")],
    partition: false,
    variant: Variant::KEPT,
};

/// Was die Konfiguration aufgeloest haben muss: was das Beispiel verlangt,
/// das Tripel des SDK-Archivs und auf dem F401 die FPU-Register jedes Fadens
/// gesichert (12.11), die ABI des Tripels und die Anwendung ab Sektor 4
/// hinter dem Sprungbrett.
fn configured(expect: &Expect, board: Board, image: &Image) -> Vec<String> {
    let triple = format!("\"{}\"", board.triple);
    let mut want: Vec<(&str, &str)> = expect.config.to_vec();
    want.push(("TAKT_TARGET", triple.as_str()));
    if board == STM32F401 {
        want.extend([("FPU_SHARING", "y"), ("FP_HARDABI", "y"), ("FLASH_LOAD_OFFSET", "0x10000")]);
    }
    want.into_iter()
        .filter(|(name, value)| image.config(name) != Some(value))
        .map(|(name, value)| {
            format!("{} {}: CONFIG_{name} ist {:?}, verlangt {value}", expect.example, board.west, image.config(name))
        })
        .collect()
}

/// Was im ELF stehen muss, und mit Partition die Arena zwischen ihren
/// Grenzen (12.11, `examples/zephyr/src/partition.h`).
fn linked(expect: &Expect, board: Board, image: &Image) -> Vec<String> {
    let target = if board == STM32F401 { Target::THUMBV7EM } else { Target::RISCV32IMAC };
    let Some(symbols) = Binutils::best_for(target).symbols(&image.elf) else {
        return vec![format!("{}: `nm` liest {} nicht", board.west, image.elf.display())];
    };
    let at = |name: &str| symbols.iter().find(|s| s.name == name).map(|s| s.address);
    let mut failed: Vec<String> = PORT
        .iter()
        .chain(expect.linked)
        .filter(|name| at(name).is_none())
        .map(|name| format!("{} {}: `{name}` fehlt im Abbild", expect.example, board.west))
        .collect();
    if expect.partition {
        let partition = (at("z_data_smem_takt_arena_part_start"), at("z_data_smem_takt_arena_part_end"), at("arena"));
        match partition {
            (Some(start), Some(end), Some(arena)) if (start..end).contains(&arena) => {}
            (start, end, arena) => failed.push(format!(
                "{}: die Arena ({arena:x?}) liegt nicht in der Partition `takt_arena` ({start:x?}..{end:x?})",
                board.west
            )),
        }
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

/// Die Probe findet den Tick: genau eine Arena im ELF und `tick_at` des
/// Manifests in ihr. Auf dem C6 liest sie ihn ueber JTAG, wenn die Leitung
/// schweigt; fehlte er, endete der Lauf dort statt mit dem Befund
/// (`board::esp32c6::Tick`).
fn ticks_found(expect: &Expect, board: Board, image: &Image) -> Vec<String> {
    match image.tick_address(board) {
        Ok(_) => Vec::new(),
        Err(e) => vec![format!("{} {}: die Probe findet den Tick nicht: {e}", expect.example, board.west)],
    }
}

/// Baut `expect.example` fuer jedes Board und nennt, was fehlt.
fn builds(expect: &Expect, boards: &[Board]) -> Vec<String> {
    let west = zephyr::west().expect("TAKT_WEST nennt west");
    let takt = Path::new(env!("CARGO_BIN_EXE_takt"));
    let mut failed = Vec::new();
    for &board in boards {
        let dir = scratch(&format!("takt-{}-{}", expect.example, board.triple));
        match zephyr::build(&west, expect.example, board, expect.variant, &dir, takt) {
            Ok(image) => {
                failed.extend(configured(expect, board, &image));
                failed.extend(linked(expect, board, &image));
                failed.extend(placed(board, &image));
                failed.extend(ticks_found(expect, board, &image));
            }
            Err(e) => failed.push(e),
        }
    }
    failed
}

/// **Das Zephyr-Beispiel baut fuer beide Boards** (12.11, M11 Schritt 19):
/// `west build` mit dem Modul, `takt_add_program` und dem SDK-Archiv des
/// Ziels; Konfiguration, Symbole, Partition und Lage des Abbilds wie oben.
#[test]
#[ignore = "Zephyr: TAKT_WEST und die Umgebung aus activate.ps1 (D:/takt-tools); mit --ignored"]
fn the_zephyr_example_builds_for_both_boards() {
    let failed = builds(&ZEPHYR_EXPECT, &[STM32F401, ESP32C6]);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Das CMSIS-RTOS2-Beispiel baut fuer den F401** (12.11, M11 Schritt 19c):
/// derselbe Port mit dem generischen Adapter unter Zephyrs CMSIS-Schicht.
#[test]
#[ignore = "Zephyr: TAKT_WEST und die Umgebung aus activate.ps1 (D:/takt-tools); mit --ignored"]
fn the_cmsis_rtos2_example_builds_for_the_f401() {
    let failed = builds(&CMSIS_EXPECT, &[STM32F401]);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// **Das Beispiel mit Pins aus dem Devicetree baut fuer beide Boards** (8.10,
/// 12.11): `takt_add_program` gibt unter Zephyr `--zephyr`, das Modul
/// uebersetzt `wire_zephyr.c` mit den Koepfen von Zephyr, und die Knoten der
/// Drahtbruecke stehen im Devicetree, sonst scheiterte der Bau an ihrer
/// Zusicherung.
#[test]
#[ignore = "Zephyr: TAKT_WEST und die Umgebung aus activate.ps1 (D:/takt-tools); mit --ignored"]
fn the_zephyr_gpio_example_builds_for_both_boards() {
    let failed = builds(&GPIO_EXPECT, &[STM32F401, ESP32C6]);
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
