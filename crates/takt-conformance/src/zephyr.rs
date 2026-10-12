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
//!
//! **Zwischengespeichert wie die Bring-ups** (FB-270, M11 Schritt 34d):
//! [`image`] legt jedes Abbild unter `zephyr-<tripel>-<stand>` in den
//! Zwischenspeicher der Abbilder ([`crate::board::IMAGES`]). Der Job der
//! Board-Abbilder baut sie dort mit (`.github/workflows/boards.yml`), und
//! `tools/board-run.ps1 -FromCi` faehrt sie ohne `west`.

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

/// Das Beispiel mit den Treibern aus dem Devicetree, auf beiden Boards: die
/// Messschleife auf der Drahtbruecke (`examples/zephyr-gpio`, 8.10).
pub const ZEPHYR_GPIO: &str = "zephyr-gpio";

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

/// Die Fassung von Zephyr, gegen die jedes Beispiel baut: dieselbe in
/// `D:\takt-tools` und im Job `zephyr` von `.github/workflows/boards.yml`.
/// Sie geht in den Stand der Abbilder ein; welche Fassung wirklich baut,
/// prueft der Bau nicht.
pub const ZEPHYR_VERSION: &str = "v4.4.2";

/// Die Fassung des Zephyr-SDK, ebenso.
pub const SDK_VERSION: &str = "1.0.1";

/// Was ausser den Crates in den Stand eingeht: das Modul, das CMake-Paket,
/// die Ports, der Kopf des SDK und die Beispiele unter Zephyr.
#[cfg(feature = "board")]
const TREES: &[&str] = &[
    "zephyr",
    "tools/cmake",
    "crates/takt-embed-c/ports",
    "crates/takt-embed-c/include",
    "examples/zephyr",
    "examples/cmsis-rtos2",
    "examples/zephyr-gpio",
];

/// Die Dateien, die die Beispiele ausserhalb ihrer Verzeichnisse lesen: das
/// Pruefprogramm, die Hardware-Konfigurationen der Boards und die Versionen
/// des SDK-Archivs, das ein eigener Workspace ist.
#[cfg(feature = "board")]
const FILES: &[&str] = &[
    "corpus-try/124_port_test.takt",
    "corpus-try/hw/stm32f401.hw",
    "corpus-try/hw/esp32c6.hw",
    "crates/takt-embed-c/Cargo.lock",
];

/// Die Art des Stands eines Boards im Zwischenspeicher der Abbilder:
/// `zephyr-<tripel>`, neben Bring-up und C-Wirt desselben Tripels.
#[cfg(feature = "board")]
fn kind(board: Board) -> String {
    format!("zephyr-{}", board.triple)
}

/// Das Verzeichnis des Stands: `<art>-<stand>` unter
/// [`crate::board::IMAGES`]. Der Stand haengt nur an Inhalten
/// ([`crate::board::Fingerprint`]), so findet ein Lauf unter Windows das
/// Abbild, das ein Runner unter Linux baute.
#[cfg(feature = "board")]
fn stand(board: Board) -> Result<PathBuf, String> {
    let root = crate::repo_root();
    let mut f = crate::board::Fingerprint::new();
    f.str(ZEPHYR_VERSION);
    f.str(SDK_VERSION);
    for tree in TREES {
        f.str(tree);
        let dir = root.join(tree);
        if dir.is_dir() {
            f.u64(1);
            crate::board::hash_tree(&dir, &mut f)?;
        } else {
            f.u64(0);
        }
    }
    for file in FILES {
        f.str(file);
        f.file_if_any(&root.join(file));
    }
    crate::board::hash_crates(&mut f)?;
    crate::board::hash_build_inputs(&root, &mut f)?;
    Ok(crate::board::images().join(format!("{}-{:016x}", kind(board), f.finish())))
}

/// Der Schluessel eines Abbilds in seinem Stand: Beispiel, Board und ob es
/// sein Journal loescht.
#[cfg(feature = "board")]
fn key(example: &str, board: Board, fresh: bool) -> String {
    let mut f = crate::board::Fingerprint::new();
    f.str(example);
    f.str(board.west);
    f.u64(u64::from(fresh));
    format!("{:016x}", f.finish())
}

/// Das Abbild von `examples/<example>` fuer `board`, aus dem Zwischenspeicher
/// der Abbilder (`<stand>/<schluessel>.elf`, `.bin`, `.config`) oder mit
/// `west` frisch gebaut in `dir` ([`build`]) und dort abgelegt. Ohne `west`
/// und ohne Abbild im Zwischenspeicher sagt der Fehler, was fehlt; `west`
/// ist `None`, wo Zephyr nicht eingerichtet ist, etwa am Rechner der Boards
/// mit den Abbildern aus CI (`tools/board-run.ps1 -FromCi`).
#[cfg(feature = "board")]
pub fn image(
    west: Option<&Path>,
    example: &str,
    board: Board,
    fresh: bool,
    dir: &Path,
    takt: &Path,
) -> Result<Image, String> {
    let stand = stand(board)?;
    let key = key(example, board, fresh);
    let cached = |ext: &str| stand.join(format!("{key}.{ext}"));
    let elf = cached("elf");
    if elf.is_file() {
        let config = std::fs::read_to_string(cached("config")).map_err(|e| format!("{}: {e}", stand.display()))?;
        return Ok(Image { elf, bin: cached("bin"), config });
    }
    let Some(west) = west else {
        return Err(format!(
            "examples/{example} fuer {}: kein Abbild im Zwischenspeicher ({}) und kein west (TAKT_WEST)",
            board.west,
            elf.display()
        ));
    };
    crate::board::missed(&stand, &elf, &format!("examples/{example} {}", board.west))?;
    let built = build(west, example, board, fresh, dir, takt)?;
    crate::board::enter_stand(&kind(board), &stand)?;
    let config = dir.join("zephyr").join(".config");
    // Das ELF zuletzt: Wer es findet, findet auch die anderen beiden.
    for (from, to) in [(&config, cached("config")), (&built.bin, cached("bin")), (&built.elf, elf.clone())] {
        crate::board::publish_checked(from, &to, |_| Ok(true))?;
    }
    Ok(Image { elf, bin: cached("bin"), config: built.config })
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
