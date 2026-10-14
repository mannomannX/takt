//! Takt unter ESP-IDF (12.11, M11 Schritt 21a): `examples/esp-idf` mit
//! `idf.py` fuer den ESP32-C6, ein Programm ueber `takt_add_program` aus der
//! Komponente (`esp-idf/takt` an der Wurzel) in der Interruptform, mit dem
//! SDK-Archiv fuer `riscv32imac-unknown-none-elf`.
//!
//! **Das Werkzeug liegt ausserhalb des Baums:** `TAKT_IDF` nennt `idf.py` in
//! einer Umgebung, die ESP-IDF eingerichtet hat (`export.sh`,
//! `export.ps1`; auf CI `.github/actions/esp-idf`). Die Fassung ist
//! [`IDF_VERSION`].
//!
//! **Das Abbild ist eines am Stueck ab 0x0** ([`Image::bin`]): Bootloader,
//! Partitionstabelle und Anwendung aus demselben Bau, zusammengefuegt von
//! `idf.py merge-bin`. So schreibt `probe-rs` genau, was `idf.py flash`
//! schriebe, und baut kein eigenes Abbild aus dem ELF; das Journal in seiner
//! Partition hinter der Anwendung bleibt unberuehrt.
//!
//! **Die Residenz im RAM belegt der Bau am Abbild** (12.3, plan/m11.md 1.7):
//! `takt check-image` prueft den Tick-Pfad des Programms gegen sein Manifest,
//! und [`RESIDENT`] nennt, was der Schritt ausserhalb des Programms ruft —
//! ISRs und Primitive des Adapters, Kern, Geraet des Journals, Leitung und
//! Watchdog des Wirts; jedes davon muss im IRAM liegen. Ein Abbild, das das
//! nicht haelt, kommt nicht in den Zwischenspeicher.
//!
//! **Zwischengespeichert wie die Bring-ups** (FB-270): Jedes Abbild liegt
//! unter `esp-idf-<tripel>-<stand>` im Zwischenspeicher der Abbilder
//! ([`crate::board::IMAGES`]) als `<schluessel>.elf`, `.bin` und `.manifest`;
//! der Job der Board-Abbilder baut sie dort mit, und `tools/board-run.ps1
//! -FromCi` faehrt sie ohne ESP-IDF.

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(feature = "board")]
use crate::board::esp32c6::Esp32c6;
#[cfg(feature = "board")]
use crate::board::{Bin, Board, Builder, Form, Options};

/// Die Fassung von ESP-IDF, gegen die das Beispiel baut: dieselbe unter
/// `G:\esp\v6.1-beta1` und in `.github/actions/esp-idf`. Sie geht in den
/// Stand der Abbilder ein.
pub const IDF_VERSION: &str = "v6.1-beta1";

/// Das Tripel des C6, fuer das das SDK-Archiv gebaut ist.
pub const TRIPLE: &str = "riscv32imac-unknown-none-elf";

/// `idf.py` aus `TAKT_IDF`.
pub fn idf() -> Option<PathBuf> {
    std::env::var_os("TAKT_IDF").map(PathBuf::from)
}

/// Was ein Bau liefert.
#[derive(Clone, Debug)]
pub struct Image {
    /// Das ELF der Anwendung.
    pub elf: PathBuf,
    /// Das Abbild am Stueck ab 0x0: Bootloader, Partitionstabelle, Anwendung.
    pub bin: PathBuf,
    /// Das Manifest des Programms (`program.manifest`).
    pub manifest: PathBuf,
}

/// Was der Schritt ausserhalb des Programms ruft und darum im IRAM liegen
/// muss: die ISRs und Primitive des Adapters (`ports/irq/esp-idf`), der Port
/// der Interruptform, der Kern, das Geraet des Journals (`ports/esp-idf-nvm`),
/// Leitung und Watchdog des Wirts (`examples/esp-idf/main`) und was sie von
/// ESP-IDF rufen. Dass dessen Funktionen im IRAM liegen, haengt an der
/// Konfiguration (`CONFIG_GPTIMER_CTRL_FUNC_IN_IRAM` in `sdkconfig.defaults`,
/// `CONFIG_FREERTOS_PLACE_ISR_FUNCTIONS_INTO_FLASH` aus); die Liste faengt
/// eine, die sie verschiebt.
pub const RESIDENT: &[&str] = &[
    "gptimer_get_raw_count",
    "gptimer_set_alarm_action",
    "vTaskGenericNotifyGiveFromISR",
    "xPortInIsrContext",
    "takt_chip_on_alarm",
    "takt_chip_on_pend",
    "takt_irq_on_step",
    "takt_irq_chip_now",
    "takt_irq_chip_arm",
    "takt_irq_chip_disarm",
    "takt_irq_chip_pend_step",
    "takt_irq_chip_pend_jobs",
    "takt_core_service",
    "takt_idf_nvm_erase",
    "takt_idf_nvm_write",
    "takt_idf_nvm_poll",
    "line_isr",
    "line_try_write",
    "line_flush",
    "kick",
];

/// Wo der C6 Code aus dem RAM rechnet: das HP-SRAM, 512 KiB.
const IRAM: std::ops::Range<u64> = 0x4080_0000..0x4088_0000;

/// Baut `examples/esp-idf` mit `program` in `dir`: das Werkzeug `takt`, das
/// SDK-Archiv fuer [`TRIPLE`], `ticks` Ticks und, mit `fresh`, ein frisches
/// Journal. Danach prueft es die Residenz im RAM am Abbild.
pub fn build(idf: &Path, program: &Path, ticks: u64, fresh: bool, dir: &Path, takt: &Path) -> Result<Image, String> {
    let sdk = crate::harness::sdk_library_for(Some(TRIPLE))?;
    // CMake liest `\` in einem Pfad als Escape.
    let cmake_path = |p: &Path| p.display().to_string().replace('\\', "/");
    let project = crate::repo_root().join("examples").join("esp-idf");
    let out = Command::new(idf)
        // Der Komponentenmanager legte sonst `dependencies.lock` ins Beispiel.
        .env("IDF_COMPONENT_MANAGER", "0")
        .arg("-C")
        .arg(&project)
        .arg("-B")
        .arg(dir)
        // Die Konfiguration im Bauverzeichnis, nicht im Beispiel: Zwei Baue
        // zugleich schrieben sonst dieselbe Datei.
        .arg(format!("-DSDKCONFIG={}", cmake_path(&dir.join("sdkconfig"))))
        .arg(format!("-DTAKT_EXECUTABLE={}", cmake_path(takt)))
        .arg(format!("-DTAKT_SDK_LIBRARY={}", cmake_path(&sdk)))
        .arg(format!("-DTAKT_PROGRAM={}", cmake_path(program)))
        .arg(format!("-DTAKT_TICKS={ticks}"))
        .arg(format!("-DTAKT_FRESH_JOURNAL={}", if fresh { "ON" } else { "OFF" }))
        .args(["build", "merge-bin"])
        .output()
        .map_err(|e| format!("{}: {e}", idf.display()))?;
    if !out.status.success() {
        return Err(format!(
            "idf.py build von examples/esp-idf mit {}:\n{}{}",
            program.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let image = Image {
        elf: dir.join("takt_esp_idf.elf"),
        bin: dir.join("merged-binary.bin"),
        manifest: dir.join("esp-idf").join("main").join("takt").join("program").join("program.manifest"),
    };
    resident(takt, &image)?;
    Ok(image)
}

/// Die Residenz im RAM am Abbild: `takt check-image` fuer den Tick-Pfad des
/// Programms, [`RESIDENT`] fuer das, was der Schritt sonst ruft.
pub fn resident(takt: &Path, image: &Image) -> Result<(), String> {
    let checked = Command::new(takt)
        .arg("check-image")
        .arg(&image.elf)
        .arg("--manifest")
        .arg(&image.manifest)
        .output()
        .map_err(|e| format!("{}: {e}", takt.display()))?;
    if !checked.status.success() {
        return Err(format!(
            "takt check-image {}:\n{}{}",
            image.elf.display(),
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        ));
    }
    let symbols = takt_llvm::inspect::Binutils::best_for(takt_llvm::target::Target::RISCV32IMAC)
        .symbols(&image.elf)
        .ok_or_else(|| "`nm` fehlt: kein Blick ins Abbild".to_string())?;
    let outside = outside_iram(&symbols, RESIDENT);
    if outside.is_empty() { Ok(()) } else { Err(format!("nicht im IRAM: {}", outside.join(", "))) }
}

/// Was von `names` fehlt oder ausserhalb des IRAM liegt, mit Adresse.
fn outside_iram(symbols: &[takt_llvm::inspect::Symbol], names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter_map(|name| match symbols.iter().find(|s| s.name == *name) {
            Some(s) if IRAM.contains(&s.address) => None,
            Some(s) => Some(format!("{name} bei {:#010x}", s.address)),
            None => Some(format!("{name} fehlt")),
        })
        .collect()
}

/// Was ausser den Crates in den Stand eingeht: die Komponente, das
/// CMake-Paket, die Ports, der Kopf des SDK und das Beispiel.
#[cfg(feature = "board")]
const TREES: &[&str] =
    &["esp-idf", "tools/cmake", "crates/takt-embed-c/ports", "crates/takt-embed-c/include", "examples/esp-idf"];

/// Die Dateien, die das Beispiel ausserhalb seines Verzeichnisses liest: die
/// Hardware-Konfiguration und die Versionen des SDK-Archivs. Das Programm
/// geht in den Schluessel ein ([`crate::board::image_key`]).
#[cfg(feature = "board")]
const FILES: &[&str] = &["corpus-try/hw/esp32c6-idf.hw", "crates/takt-embed-c/Cargo.lock"];

/// Die Art des Stands im Zwischenspeicher: `esp-idf-<tripel>`.
#[cfg(feature = "board")]
fn kind() -> String {
    format!("esp-idf-{TRIPLE}")
}

/// Das Verzeichnis des Stands: `<art>-<stand>` unter
/// [`crate::board::IMAGES`], nur aus Inhalten.
#[cfg(feature = "board")]
fn stand() -> Result<PathBuf, String> {
    let root = crate::repo_root();
    let mut f = crate::board::Fingerprint::new();
    f.str(IDF_VERSION);
    for tree in TREES {
        f.str(tree);
        crate::board::hash_tree(&root.join(tree), &mut f)?;
    }
    for file in FILES {
        f.str(file);
        f.file_if_any(&root.join(file));
    }
    crate::board::hash_crates(&mut f)?;
    crate::board::hash_build_inputs(&root, &mut f)?;
    Ok(crate::board::images().join(format!("{}-{:016x}", kind(), f.finish())))
}

/// Was der Wirt unter ESP-IDF nicht kann, scheitert beim Bau, statt still zu
/// fehlen: Er faehrt das Programm selbst in Echtzeit in der Interruptform,
/// mit seiner eigenen Hardware-Konfiguration, ohne Treiber, falschen Port,
/// verstellte FPU oder Tunes von der Konsole.
#[cfg(feature = "board")]
fn supported(options: &Options) -> Result<(), String> {
    let missing = [
        (options.bin != Bin::Takt, "nur das Programm selbst, kein Mess- oder Native-Programm"),
        (!options.timed, "nur Echtzeit; logische Zeit hat der Wirt nicht"),
        (options.form != Form::Interrupt, "nur die Interruptform (plan/m11.md 2.5)"),
        (options.hardware.is_some(), "nur seine Hardware-Konfiguration (esp32c6-idf.hw)"),
        (options.flaw.is_some(), "keinen absichtlich falschen Port"),
        (options.hostile_fpu, "keine verstellte FPU; der C6 hat keine"),
        (!options.console.is_empty(), "keine Tunes von der Konsole"),
    ];
    match missing.iter().find(|(asked, _)| *asked) {
        Some((_, what)) => Err(format!("der Wirt unter ESP-IDF kennt {what}")),
        None => Ok(()),
    }
}

/// Das Abbild fuer `program`, aus dem Zwischenspeicher der Abbilder oder mit
/// `idf.py` frisch gebaut ([`build`]) und dort abgelegt; liefert sein ELF,
/// neben dem `.bin` und `.manifest` liegen. Gebaut wird in einem Verzeichnis
/// je Prozess. Ohne `idf.py` und ohne Abbild im Zwischenspeicher sagt der
/// Fehler, was fehlt; `idf` ist `None`, wo ESP-IDF nicht eingerichtet ist,
/// etwa am Rechner der Boards mit den Abbildern aus CI.
#[cfg(feature = "board")]
pub fn image(idf: Option<&Path>, program: &Path, options: &Options) -> Result<PathBuf, String> {
    supported(options)?;
    let stand = stand()?;
    let key = format!("{:016x}", crate::board::image_key(TRIPLE, program, options)?);
    let cached = |ext: &str| stand.join(format!("{key}.{ext}"));
    let elf = cached("elf");
    if elf.is_file() {
        return Ok(elf);
    }
    let Some(idf) = idf else {
        return Err(format!(
            "{}: kein Abbild im Zwischenspeicher ({}) und kein idf.py (TAKT_IDF)",
            program.display(),
            elf.display()
        ));
    };
    crate::board::missed(&stand, &elf, &format!("examples/esp-idf {}", program.display()))?;
    let target = crate::target_dir();
    let dir = target.join("takt-idf-build").join(format!("{key}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tool = target.join("release").join(if cfg!(windows) { "takt.exe" } else { "takt" });
    let built = build(idf, program, options.ticks, options.fresh, &dir, &tool);
    let published = built.and_then(|image| {
        crate::board::enter_stand(&kind(), &stand)?;
        // Das ELF zuletzt: Wer es findet, findet auch die anderen beiden.
        for (from, to) in
            [(&image.manifest, cached("manifest")), (&image.bin, cached("bin")), (&image.elf, elf.clone())]
        {
            crate::board::publish_checked(from, &to, |_| Ok(true))?;
        }
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&dir);
    published.map(|()| elf)
}

/// Board 2 mit dem Wirt unter ESP-IDF (`examples/esp-idf`): geschrieben,
/// gestartet und gelesen wie das Bring-up in Rust ([`Esp32c6`]), das Abbild
/// am Stueck ab 0x0 ([`Image::bin`]).
#[cfg(feature = "board")]
#[derive(Debug)]
pub struct Esp32c6Idf {
    board: Esp32c6,
}

#[cfg(feature = "board")]
impl Esp32c6Idf {
    /// Das Board `board` mit dem Wirt unter ESP-IDF.
    pub fn on(board: Esp32c6) -> Esp32c6Idf {
        Esp32c6Idf { board }
    }
}

#[cfg(feature = "board")]
impl Board for Esp32c6Idf {
    fn name(&self) -> &'static str {
        "esp32c6-idf"
    }

    fn target(&self) -> &'static str {
        "riscv32imac"
    }

    /// Ein Bau in ein Verzeichnis je Prozess: Jeder Bauplatz baut gleich.
    fn builder(&self, _slot: usize) -> Builder {
        Box::new(|program, options| image(idf().as_deref(), program, options))
    }

    /// Das Abbild am Stueck ab 0x0, sein ELF fuer den Blick ueber JTAG.
    fn run(&mut self, elf: &Path, options: &Options) -> Result<String, String> {
        self.board.run_raw(&elf.with_extension("bin"), 0, elf, options)
    }

    fn forms(&self) -> &'static [Form] {
        &[Form::Interrupt]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, address: u64) -> takt_llvm::inspect::Symbol {
        takt_llvm::inspect::Symbol { name: name.to_string(), address, size: 4, kind: 'T' }
    }

    /// **Was der Schritt ruft, liegt im IRAM, oder der Bau nennt es** (12.3):
    /// eine Funktion im Flash (0x42…) und eine, die fehlt.
    #[test]
    fn a_function_of_the_step_outside_iram_is_named() {
        let symbols = [symbol("kick", 0x4080_1234), symbol("line_isr", 0x4200_0100)];
        let outside = outside_iram(&symbols, &["kick", "line_isr", "takt_core_service"]);
        assert_eq!(outside, vec!["line_isr bei 0x42000100".to_string(), "takt_core_service fehlt".to_string()]);
    }
}
