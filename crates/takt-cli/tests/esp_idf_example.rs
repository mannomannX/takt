//! Takt unter ESP-IDF (12.11, M11 Schritt 21a): Die Komponente
//! (`esp-idf/takt`) mit dem Port der Interruptform und dem Adapter fuer
//! ESP-IDF baut `examples/esp-idf` fuer den C6. Was ohne Board zu pruefen
//! ist, prueft dieser Test: Programm, SDK-Archiv, Port und Journal binden mit
//! der GCC von ESP-IDF, das Abbild am Stueck beginnt mit dem Bootloader, und
//! alles, was der Schritt ruft, liegt im IRAM — der Tick-Pfad des Programms
//! nach `takt check-image`, Adapter, Kern, Journal und Leitung nach ihren
//! Adressen ([`takt_conformance::esp_idf::RESIDENT`]). Die Portpruefung auf
//! dem Board faehrt `board_esp32c6.rs`.
//!
//! ESP-IDF liegt ausserhalb des Baums: Der Test laeuft mit `--ignored`, und
//! `TAKT_IDF` nennt `idf.py` in einer Umgebung, die ESP-IDF eingerichtet hat
//! (auf CI `.github/actions/esp-idf`).

use std::path::{Path, PathBuf};

use takt_conformance::esp_idf;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// **Das Pruefprogramm baut unter ESP-IDF, und der Schritt liegt im RAM**
/// (12.3, 12.11): mit Jobs, `persist` und dem Journal in seiner Partition.
/// `build` scheitert, wenn eine Funktion des Schritts im Flash liegt.
#[test]
#[ignore = "ESP-IDF: TAKT_IDF nennt idf.py; mit --ignored"]
fn the_esp_idf_example_builds_with_its_step_in_ram() {
    let Some(idf) = esp_idf::idf() else { panic!("TAKT_IDF nennt kein idf.py (ESP-IDF {})", esp_idf::IDF_VERSION) };
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let program = root.join("corpus-try").join(takt_conformance::port::CORPUS_NAME);
    let dir = scratch("takt-esp-idf-port-test");
    let image = esp_idf::build(
        &idf,
        &program,
        takt_conformance::port::TICKS,
        true,
        &dir,
        Path::new(env!("CARGO_BIN_EXE_takt")),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let bin = std::fs::read(&image.bin).expect("Abbild am Stueck");
    // Der Bootloader des C6 beginnt bei 0x0 mit dem Kopf fuer den ROM-Lader.
    assert_eq!(bin.first(), Some(&0xE9), "kein Bootloader am Anfang von {}", image.bin.display());
    assert!(image.manifest.is_file(), "kein Manifest unter {}", image.manifest.display());
}
