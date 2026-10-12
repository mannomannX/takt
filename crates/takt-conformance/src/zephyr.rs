//! Takt unter Zephyr (12.11, M11 Schritt 19): ein Beispiel ([`ZEPHYR`] mit
//! dem Adapter auf den Faeden des Zephyr-Kerns, [`CMSIS_RTOS2`] mit dem auf
//! CMSIS-RTOS2) mit `west build` fuer ein Board, das Pruefprogramm ueber
//! `takt_add_program` aus dem Modul (`zephyr/` an der Wurzel) und mit dem
//! SDK-Archiv seines Ziels.
//!
//! **Die Werkzeuge liegen ausserhalb des Baums** (plan/m11.md 6: Zephyr und
//! sein SDK sind gross): `TAKT_WEST` nennt `west`, und die Umgebung traegt,
//! was `D:\takt-tools\activate.ps1` setzt — `ZEPHYR_BASE`,
//! `ZEPHYR_SDK_INSTALL_DIR`, CMake, Ninja und die Toolchains im PATH. Die
//! Tests, die bauen, laufen darum nur mit `--ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Ein Board des Beispiels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Board {
    /// Der Name fuer `west build --board`.
    pub west: &'static str,
    /// Das Tripel aus `CONFIG_TAKT_TARGET` (`zephyr/Kconfig`); dafuer ist
    /// das SDK-Archiv gebaut.
    pub triple: &'static str,
}

/// Board 1, die Black Pill mit dem STM32F401CC.
pub const STM32F401: Board = Board { west: "blackpill_f401cc", triple: "thumbv7em-none-eabihf" };

/// Board 2, der ESP32-C6, sein Hochleistungskern.
pub const ESP32C6: Board = Board { west: "esp32c6_devkitc/esp32c6/hpcore", triple: "riscv32imac-unknown-none-elf" };

/// `west` aus `TAKT_WEST`.
pub fn west() -> Option<PathBuf> {
    std::env::var_os("TAKT_WEST").map(PathBuf::from)
}

/// Was ein Bau liefert: das ELF, das Rohabbild daneben und die
/// Konfiguration, die Kconfig daraus aufgeloest hat.
#[derive(Clone, Debug)]
pub struct Image {
    /// `zephyr.elf`.
    pub elf: PathBuf,
    /// `zephyr.bin`, wie es ins Flash geht.
    pub bin: PathBuf,
    /// `zephyr/.config`.
    pub config: String,
}

impl Image {
    /// Der Wert von `CONFIG_<name>` in der aufgeloesten Konfiguration.
    pub fn config(&self, name: &str) -> Option<&str> {
        let key = format!("CONFIG_{name}=");
        self.config.lines().find_map(|l| l.strip_prefix(key.as_str()))
    }
}

/// Das Beispiel mit dem Adapter auf den Faeden des Zephyr-Kerns, auf beiden
/// Boards (`examples/zephyr`).
pub const ZEPHYR: &str = "zephyr";

/// Das Beispiel mit dem Adapter auf CMSIS-RTOS2 unter Zephyrs CMSIS-Schicht,
/// auf dem F401 (`examples/cmsis-rtos2`, M11 Schritt 19c).
pub const CMSIS_RTOS2: &str = "cmsis-rtos2";

/// Baut `examples/<example>` fuer `board` in `dir`, mit dem Werkzeug `takt`
/// und dem SDK-Archiv fuer das Tripel des Boards. Mit `fresh` loescht das
/// Abbild sein Journal vor dem Lauf (`TAKT_FRESH_JOURNAL`): Die
/// Portpruefung vergleicht mit dem Interpreter ohne gespeicherte Werte. Mit
/// zwei Jobs: Ein Zephyr-Bau ist klein, und der Rechner neben ihm baut oft
/// Rust.
pub fn build(west: &Path, example: &str, board: Board, fresh: bool, dir: &Path, takt: &Path) -> Result<Image, String> {
    let sdk = crate::harness::sdk_library_for(Some(board.triple))?;
    // CMake liest `\` in einem Pfad als Escape.
    let cmake_path = |p: &Path| p.display().to_string().replace('\\', "/");
    let out = Command::new(west)
        .args(["build", "--pristine", "always", "--board", board.west, "--build-dir"])
        .arg(dir)
        .arg(crate::repo_root().join("examples").join(example))
        .arg("-o=-j2")
        .arg("--")
        .arg(format!("-DTAKT_EXECUTABLE={}", cmake_path(takt)))
        .arg(format!("-DTAKT_SDK_LIBRARY={}", cmake_path(&sdk)))
        .arg(format!("-DTAKT_FRESH_JOURNAL={}", if fresh { "ON" } else { "OFF" }))
        .output()
        .map_err(|e| format!("{}: {e}", west.display()))?;
    if !out.status.success() {
        return Err(format!(
            "west build von examples/{example} fuer {}:\n{}{}",
            board.west,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let zephyr = dir.join("zephyr");
    let config = std::fs::read_to_string(zephyr.join(".config")).map_err(|e| format!("{}: {e}", zephyr.display()))?;
    Ok(Image { elf: zephyr.join("zephyr.elf"), bin: zephyr.join("zephyr.bin"), config })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_value_is_read_from_the_resolved_configuration() {
        let image = Image {
            elf: PathBuf::new(),
            bin: PathBuf::new(),
            config: "# CONFIG_FOO is not set\nCONFIG_TAKT=y\nCONFIG_TAKT_TARGET=\"thumbv7em-none-eabihf\"\n".into(),
        };
        assert_eq!(image.config("TAKT"), Some("y"));
        assert_eq!(image.config("TAKT_TARGET"), Some("\"thumbv7em-none-eabihf\""));
        assert_eq!(image.config("FOO"), None);
    }
}
