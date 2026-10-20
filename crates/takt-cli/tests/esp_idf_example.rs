//! Takt unter ESP-IDF (12.11, M11 Schritte 21a und 21b): Die Komponente
//! (`esp-idf/takt`) mit dem Port der Interruptform und dem Adapter fuer
//! ESP-IDF baut `examples/esp-idf` fuer den C6, ohne fremde Last und mit
//! jeder Last neben Takt (WLAN, BLE, die Messung des automatischen
//! Aussetzens). Was ohne Board zu pruefen ist, prueft dieser Test: Programm,
//! SDK-Archiv, Port, Journal und Last binden mit der GCC von ESP-IDF, das
//! Abbild am Stueck beginnt mit dem Bootloader, und alles, was der Schritt
//! ruft, liegt im IRAM — der Tick-Pfad des Programms nach `takt check-image`,
//! Adapter, Kern, Journal und Leitung nach ihren Adressen
//! ([`takt_conformance::esp_idf::RESIDENT`]), auch neben Funk. Die
//! Portpruefung auf dem Board faehrt `board_esp32c6.rs`.
//!
//! ESP-IDF liegt ausserhalb des Baums: Der Test laeuft mit `--ignored`, und
//! `TAKT_IDF` nennt `idf.py` in einer Umgebung, die ESP-IDF eingerichtet hat
//! (auf CI `.github/actions/esp-idf`). Die Baue bleiben in `target/tmp` fuer
//! die Analyse des C-Teils liegen (`c_analysis.rs`).

use std::path::{Path, PathBuf};

use takt_conformance::esp_idf::{self, Load, Spec};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// Das Bauverzeichnis unter `target/tmp` fuer die Last `load`; `c_analysis.rs`
/// liest dieselben.
fn build_dir(load: Load) -> String {
    match load {
        Load::Quiet => "takt-esp-idf-port-test".to_string(),
        _ => format!("takt-esp-idf-{}", load.name()),
    }
}

/// **Das Pruefprogramm baut unter ESP-IDF mit jeder Last, und der Schritt
/// liegt im RAM** (12.3, 12.11): mit Jobs, `persist` und dem Journal in seiner
/// Partition. `build` scheitert, wenn eine Funktion des Schritts im Flash
/// liegt.
#[test]
#[ignore = "ESP-IDF: TAKT_IDF nennt idf.py; mit --ignored"]
fn the_esp_idf_example_builds_with_its_step_in_ram() {
    let Some(idf) = esp_idf::idf() else { panic!("TAKT_IDF nennt kein idf.py (ESP-IDF {})", esp_idf::IDF_VERSION) };
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let program = root.join("corpus-try").join(takt_conformance::port::CORPUS_NAME);
    for load in Load::ALL {
        let dir = scratch(&build_dir(load));
        let spec = Spec { program: &program, ticks: takt_conformance::port::TICKS, fresh: true, logical: false, load };
        let image = esp_idf::build(&idf, &spec, &dir, Path::new(env!("CARGO_BIN_EXE_takt")))
            .unwrap_or_else(|e| panic!("{}: {e}", load.name()));
        let bin = std::fs::read(&image.bin).expect("Abbild am Stueck");
        // Der Bootloader des C6 beginnt bei 0x0 mit dem Kopf fuer den ROM-Lader.
        assert_eq!(bin.first(), Some(&0xE9), "kein Bootloader am Anfang von {}", image.bin.display());
        assert!(image.manifest.is_file(), "kein Manifest unter {}", image.manifest.display());
    }
}
