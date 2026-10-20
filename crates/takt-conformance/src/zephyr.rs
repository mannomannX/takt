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

use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

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

/// Was ein Bau liefert: das ELF, das Rohabbild daneben, die Konfiguration,
/// die Kconfig daraus aufgeloest hat, und das Manifest des Programms.
#[derive(Clone, Debug)]
pub struct Image {
    /// `zephyr.elf`.
    pub elf: PathBuf,
    /// `zephyr.bin`, wie es ins Flash geht.
    pub bin: PathBuf,
    /// `zephyr/.config`.
    pub config: String,
    /// `takt/<name>/<name>.manifest`, das `takt_add_program` schreibt (12.11).
    pub manifest: String,
}

impl Image {
    /// Der Wert von `CONFIG_<name>` in der aufgeloesten Konfiguration.
    pub fn config(&self, name: &str) -> Option<&str> {
        let key = format!("CONFIG_{name}=");
        self.config.lines().find_map(|l| l.strip_prefix(key.as_str()))
    }

    /// Der Wert von `name` im Manifest (`name = wert`).
    pub fn manifest(&self, name: &str) -> Option<&str> {
        self.manifest.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == name).map(|(_, v)| v.trim()))
    }

    /// Wo der Tick des Programms im laufenden Abbild steht: in der Arena
    /// ([`is_arena`]) `tick_at` Byte hinter ihrem Anfang, wie das Manifest
    /// sagt. Die Probe des C6 liest ihn ueber JTAG, wenn die Leitung schweigt
    /// (`board::esp32c6::Tick`); ohne ihn haette sie nur die Symbole eines
    /// Bring-ups (`app_arena`, `__takt_tick_at`).
    pub fn tick_address(&self, board: Board) -> Result<u64, String> {
        let target = Target::by_host_triple(board.triple)?;
        let symbols = Binutils::best_for(target)
            .symbols(&self.elf)
            .ok_or_else(|| format!("`nm` liest {} nicht", self.elf.display()))?;
        let arenas: Vec<_> = symbols.iter().filter(|s| is_arena(&s.name)).collect();
        let [arena] = arenas[..] else {
            let names: Vec<&str> = arenas.iter().map(|s| s.name.as_str()).collect();
            return Err(format!("{}: verlangt genau eine Arena `arena`, gefunden {names:?}", self.elf.display()));
        };
        let at: u64 = self.manifest("tick_at").and_then(|v| v.parse().ok()).ok_or("`tick_at` fehlt im Manifest")?;
        // Der Tick ist ein `long long` in der Arena.
        if at.checked_add(8).is_none_or(|end| end > arena.size) {
            return Err(format!("`tick_at` {at} liegt nicht in der Arena `{}` ({} Byte)", arena.name, arena.size));
        }
        Ok(arena.address + at)
    }
}

/// Ob `name` die Arena eines Beispiels unter Zephyr ist: Jedes nennt sie
/// `arena`, auf Dateiebene steht sie so im ELF, in einer Funktion nennt GCC
/// sie `arena.<n>`. Den Namen des Manifests (`<praefix>_arena`) traegt dort
/// der Typ; ein Objekt desselben Namens verletzte MISRA C:2012 5.7.
fn is_arena(name: &str) -> bool {
    name.strip_prefix("arena").is_some_and(|rest| {
        rest.is_empty()
            || rest.strip_prefix('.').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Das Manifest des einen Programms, das ein Beispiel baut:
/// `takt_add_program` legt es unter `takt/<name>/<name>.manifest` im
/// Bauverzeichnis ab (`tools/cmake/TaktConfig.cmake`).
fn manifest_in(dir: &Path) -> Result<PathBuf, String> {
    let takt = dir.join("takt");
    let found: Vec<PathBuf> = std::fs::read_dir(&takt)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(format!("{}.manifest", e.file_name().to_string_lossy())))
        .filter(|p| p.is_file())
        .collect();
    match &found[..] {
        [one] => Ok(one.clone()),
        _ => Err(format!("{}: verlangt genau ein Manifest, gefunden {found:?}", takt.display())),
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

/// Das Beispiel mit einem analogen Eingang aus dem Devicetree: dieselbe
/// Messschleife, gelesen ueber den ADC (`examples/zephyr-adc`, 8.10,
/// FB-735); am Board nur auf dem F401, wo die Bruecke an einem Eingang des
/// ADC endet.
pub const ZEPHYR_ADC: &str = "zephyr-adc";

/// Was ein Bau eines Beispiels ueber CMake waehlt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Variant {
    /// Das Journal vor dem Lauf loeschen (`TAKT_FRESH_JOURNAL`): Die
    /// Portpruefung vergleicht mit dem Interpreter ohne gespeicherte Werte.
    pub fresh: bool,
    /// Die Partition auch im Lauf proben (`TAKT_PROBE_IN_RUN`,
    /// `examples/zephyr`): Der Kern meldet die Schutzverletzung als
    /// `Runtime(Hardware)` (12.3).
    pub probe_in_run: bool,
}

impl Variant {
    /// Der Bau, der das Journal behaelt.
    pub const KEPT: Variant = Variant { fresh: false, probe_in_run: false };

    /// Der Bau der Portpruefung: ein frisches Journal.
    pub const FRESH: Variant = Variant { fresh: true, probe_in_run: false };

    /// Der Bau, der die Partition im Lauf probt, mit frischem Journal.
    pub const PROBE_IN_RUN: Variant = Variant { fresh: true, probe_in_run: true };

    /// Die Endung des Bauverzeichnisses: leer, `-fresh`, `-fresh-probe`.
    pub fn suffix(self) -> String {
        format!("{}{}", if self.fresh { "-fresh" } else { "" }, if self.probe_in_run { "-probe" } else { "" })
    }
}

/// Baut `examples/<example>` fuer `board` in `dir`, mit dem Werkzeug `takt`
/// und dem SDK-Archiv fuer das Tripel des Boards, in der Spielart `variant`.
/// Mit zwei Jobs: Ein Zephyr-Bau ist klein, und der Rechner neben ihm baut
/// oft Rust.
pub fn build(
    west: &Path,
    example: &str,
    board: Board,
    variant: Variant,
    dir: &Path,
    takt: &Path,
) -> Result<Image, String> {
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
        .arg(format!("-DTAKT_FRESH_JOURNAL={}", if variant.fresh { "ON" } else { "OFF" }))
        .arg(format!("-DTAKT_PROBE_IN_RUN={}", if variant.probe_in_run { "ON" } else { "OFF" }))
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
    let manifest = manifest_in(dir)?;
    let manifest = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    Ok(Image { elf: zephyr.join("zephyr.elf"), bin: zephyr.join("zephyr.bin"), config, manifest })
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
    "examples/zephyr-adc",
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

/// Der Schluessel eines Abbilds in seinem Stand: Beispiel, Board und
/// Spielart.
#[cfg(feature = "board")]
fn key(example: &str, board: Board, variant: Variant) -> String {
    let mut f = crate::board::Fingerprint::new();
    f.str(example);
    f.str(board.west);
    f.u64(u64::from(variant.fresh));
    f.u64(u64::from(variant.probe_in_run));
    format!("{:016x}", f.finish())
}

/// Das Abbild von `examples/<example>` fuer `board`, aus dem Zwischenspeicher
/// der Abbilder (`<stand>/<schluessel>.elf`, `.bin`, `.config`, `.manifest`) oder mit
/// `west` frisch gebaut in `dir` ([`build`]) und dort abgelegt. Ohne `west`
/// und ohne Abbild im Zwischenspeicher sagt der Fehler, was fehlt; `west`
/// ist `None`, wo Zephyr nicht eingerichtet ist, etwa am Rechner der Boards
/// mit den Abbildern aus CI (`tools/board-run.ps1 -FromCi`).
#[cfg(feature = "board")]
pub fn image(
    west: Option<&Path>,
    example: &str,
    board: Board,
    variant: Variant,
    dir: &Path,
    takt: &Path,
) -> Result<Image, String> {
    let stand = stand(board)?;
    let key = key(example, board, variant);
    let cached = |ext: &str| stand.join(format!("{key}.{ext}"));
    let elf = cached("elf");
    if elf.is_file() {
        let read = |ext: &str| std::fs::read_to_string(cached(ext)).map_err(|e| format!("{}: {e}", stand.display()));
        return Ok(Image { elf, bin: cached("bin"), config: read("config")?, manifest: read("manifest")? });
    }
    let Some(west) = west else {
        return Err(format!(
            "examples/{example} fuer {}: kein Abbild im Zwischenspeicher ({}) und kein west (TAKT_WEST)",
            board.west,
            elf.display()
        ));
    };
    crate::board::missed(&stand, &elf, &format!("examples/{example} {}", board.west))?;
    let built = build(west, example, board, variant, dir, takt)?;
    crate::board::enter_stand(&kind(board), &stand)?;
    let config = dir.join("zephyr").join(".config");
    let manifest = manifest_in(dir)?;
    // Das ELF zuletzt: Wer es findet, findet auch die anderen.
    for (from, to) in [
        (&config, cached("config")),
        (&manifest, cached("manifest")),
        (&built.bin, cached("bin")),
        (&built.elf, elf.clone()),
    ] {
        crate::board::publish_checked(from, &to, |_| Ok(true))?;
    }
    Ok(Image { elf, bin: cached("bin"), config: built.config, manifest: built.manifest })
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
            manifest: "# takt-manifest 1\nprefix = port_test\ntick_at = 14272\n".into(),
        };
        assert_eq!(image.config("TAKT"), Some("y"));
        assert_eq!(image.config("TAKT_TARGET"), Some("\"thumbv7em-none-eabihf\""));
        assert_eq!(image.config("FOO"), None);
        assert_eq!(image.manifest("tick_at"), Some("14272"));
        assert_eq!(image.manifest("tick"), None);
    }

    /// **Jedes Board nennt ein Ziel, dessen `nm` die Probe fragt**
    /// ([`Image::tick_address`]): das Tripel des Rust-Ziels, nicht das von LLVM.
    #[test]
    fn every_board_names_a_known_target() {
        for board in [STM32F401, ESP32C6] {
            assert!(Target::by_host_triple(board.triple).is_ok(), "{}", board.triple);
        }
    }

    /// **Die Arena eines Beispiels heisst `arena`**, auf Dateiebene so und in
    /// einer Funktion `arena.<n>`; nichts sonst, was so anfaengt.
    #[test]
    fn the_arena_is_found_at_file_scope_and_in_a_function() {
        for name in ["arena", "arena.0", "arena.12"] {
            assert!(is_arena(name), "{name}");
        }
        for name in ["arena.", "arena.x", "arenas", "port_test_arena", "z_data_smem_takt_arena_part_start"] {
            assert!(!is_arena(name), "{name}");
        }
    }
}
