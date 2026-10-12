//! Board 1, STM32F401 (WeAct Black Pill V1.2), vom Host aus (plan/f401.md).
//!
//! Geschrieben wird ueber den DFU-Bootloader im ROM, und die Hand braucht
//! es dafuer nicht: `TAKT` auf der Trace-Leitung laesst die laufende
//! Anwendung in den Bootloader springen (FB-275), [`super::dfuse`] schreibt
//! das Abbild nach Sektor 4, liest es zurueck und startet es. Der Trace
//! kommt ueber USART1 an den Adapter.
//!
//! **Das Flash** (FB-385): Sektor 0 haelt der HID-Bootloader von WeAct und
//! wird nie geschrieben. Er springt nach Sektor 1, ins Sprungbrett
//! (`takt-trampoline-stm32f401`), das die Anwendung in Sektor 4 startet
//! oder ohne sie in den DFU-Bootloader geht. Sektor 2 und 3 haelt das
//! `persist`-Journal; kein Abbild beruehrt sie. Das Sprungbrett schreibt der
//! Lauf nur, wenn es fehlt oder abweicht.
//!
//! Antwortet das Board nicht — ein Programm ohne Leitung, ein Absturz —,
//! bleibt die Hand: BOOT0 halten, NRST druecken und loslassen, BOOT0
//! loslassen. Der Lauf wartet eine Minute darauf und sagt es.
//!
//! `TAKT_F401_PORT` nennt den Port des Adapters (`COM7`).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

use super::dfuse::Dfuse;
use super::{Board, Bringup, Builder, ConsoleLine, Form, Options, attached, capture, missed, publish_checked};

/// Das Bring-up; seine Eingaben nennen auch die Dateien des Sprungbretts
/// ausserhalb von `src`: Es liegt im selben Stand ([`trampoline`]).
pub(super) const BRINGUP: Bringup = Bringup {
    dir: "crates/takt-bringup-stm32f401",
    triple: "thumbv7em-none-eabihf",
    inputs: &[
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "memory.x",
        "takt_state.x",
        "takt_stack.x",
        "../../corpus-try/hw/stm32f401.hw",
        "../takt-trampoline-stm32f401/Cargo.lock",
        "../takt-trampoline-stm32f401/memory.x",
    ],
    env: &[],
};

/// Das Sprungbrett im Stand des Bring-ups.
const TRAMPOLINE_IMAGE: &str = "trampoline.bin";

/// Die Rate der Leitung, wie `takt-board-stm32f401::uart::BAUD`: eine fuer
/// jedes Programm, damit der Host das laufende erreicht, ohne es zu kennen.
pub const BAUD: u32 = 921_600;

/// Wo die Anwendung beginnt: Sektor 4, hinter HID-Bootloader, Sprungbrett
/// und Journal.
const APP: u32 = 0x0801_0000;

/// Wo das Sprungbrett liegt: Sektor 1, wohin der HID-Bootloader springt.
const TRAMPOLINE: u32 = 0x0800_4000;

/// Das Crate des Sprungbretts, relativ zur Wurzel.
const TRAMPOLINE_CRATE: &str = "crates/takt-trampoline-stm32f401";

/// Wo sie endet: 256 KB Flash der Variante CC.
const APP_END: u32 = 0x0804_0000;

/// Das RAM des F401CC.
const RAM: std::ops::RangeInclusive<u32> = 0x2000_0000..=0x2001_0000;

/// Die USB-Kennung des ROM-Bootloaders (AN2606).
const DFU: (u16, u16) = (0x0483, 0xdf11);

/// Wie lange der Bootloader nach dem Wunsch auf sich warten lassen darf.
const HANDBACK: Duration = Duration::from_secs(6);

/// Wie lange ein Lauf auf die Hand wartet, wenn das Board nicht antwortet.
const BY_HAND: Duration = Duration::from_secs(60);

/// Board 1 am Host.
#[derive(Clone, Debug)]
pub struct Stm32f401 {
    /// Der Port des Adapters; keiner im Lauf, der nur baut.
    port: Option<String>,
}

impl Stm32f401 {
    /// Das Board an `TAKT_F401_PORT`; `None` ohne die Variable. Im Lauf, der
    /// nur baut ([`super::build_only`]), eines ohne Port: Es baut Abbilder und
    /// Sprungbrett, und jeder Zugriff auf das Board endet mit
    /// [`super::BUILT_ONLY`].
    pub fn from_env() -> Option<Stm32f401> {
        if super::build_only() {
            return Some(Stm32f401 { port: None });
        }
        let port = std::env::var("TAKT_F401_PORT").ok()?;
        Some(Stm32f401 { port: Some(port) })
    }

    /// Der Port des Adapters; [`super::BUILT_ONLY`] im Lauf, der nur baut.
    pub fn port(&self) -> Result<&str, String> {
        attached(self.port.as_deref())
    }

    /// Liest, was das Board von sich aus schreibt, ohne es neu zu starten —
    /// nach dem Wecken aus dem Tiefschlaf (12.7). Die Leitung bleibt dabei
    /// offen, nur das Board schweigt, solange es schlaeft.
    pub fn listen(&self, within: Duration) -> Result<String, String> {
        capture(self.port()?, BAUD, serialport::FlowControl::Software, within, false, &[], || Ok(()))
    }

    /// Schreibt das Abbild, startet es und liest bis `takt end`, hoechstens
    /// `within` lang — auch einen Lauf, der nicht endet oder ueber Resets
    /// hinweg geht (12.3, 12.7).
    pub fn run_for(&mut self, elf: &Path, within: Duration) -> Result<String, String> {
        self.run_with(elf, within, &[])
    }

    /// Wie [`Stm32f401::run_for`]; `console` geht waehrenddessen an die
    /// Konsole des Boards. Rohabbild und Sprungbrett entstehen vor dem ersten
    /// Zugriff auf das Board: Der Lauf, der nur baut, legt sie so mit in den
    /// Zwischenspeicher.
    fn run_with(&mut self, elf: &Path, within: Duration, console: &[ConsoleLine]) -> Result<String, String> {
        let bin = self.image(elf)?;
        let bytes = std::fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        let trampoline = trampoline()?;
        let port = self.port()?;
        self.to_bootloader()?;
        // Der Adapter haelt das Board an, statt Bytes zu verlieren, wenn der
        // Wirt nicht abholt (FB-306).
        capture(port, BAUD, serialport::FlowControl::Software, within, true, console, || {
            let dfu = Dfuse::open(DFU.0, DFU.1)?;
            if !dfu.holds(TRAMPOLINE, trampoline)? {
                dfu.write(TRAMPOLINE, trampoline)?;
            }
            dfu.write(APP, &bytes)?;
            dfu.leave(APP)
        })
    }

    /// Steht das Board im DFU-Bootloader?
    pub fn in_bootloader(&self) -> Result<bool, String> {
        self.port()?;
        super::dfuse::present(DFU.0, DFU.1)
    }

    /// Bringt das Board in den DFU-Bootloader.
    ///
    /// Erst der Wunsch ueber die Leitung; antwortet die Anwendung nicht,
    /// wartet der Lauf [`BY_HAND`] lang auf die Hand und sagt es.
    pub fn to_bootloader(&self) -> Result<(), String> {
        if self.in_bootloader()? {
            return Ok(());
        }
        self.hand_back()?;
        if self.wait_for_bootloader(HANDBACK)? {
            return Ok(());
        }
        eprintln!(
            "{}: das Programm antwortet nicht auf `TAKT`. Von Hand: BOOT0 halten, NRST druecken und loslassen, \
             BOOT0 loslassen (warte {} s)",
            self.port()?,
            BY_HAND.as_secs()
        );
        if self.wait_for_bootloader(BY_HAND)? {
            return Ok(());
        }
        Err(format!("das Board kam nicht in den DFU-Bootloader ({:04x}:{:04x}); siehe plan/f401.md 2", DFU.0, DFU.1))
    }

    /// `TAKT` auf die Leitung: Die laufende Anwendung springt in den Bootloader.
    fn hand_back(&self) -> Result<(), String> {
        let port = self.port()?;
        let mut serial =
            serialport::new(port, BAUD).timeout(Duration::from_secs(1)).open().map_err(|e| format!("{port}: {e}"))?;
        serial.write_all(b"TAKT").and_then(|()| serial.flush()).map_err(|e| format!("{port}: {e}"))
    }

    /// Wartet hoechstens `within`, bis der Bootloader sich meldet.
    fn wait_for_bootloader(&self, within: Duration) -> Result<bool, String> {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if self.in_bootloader()? {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Ok(false)
    }

    /// Das Rohabbild neben dem ELF, geprueft.
    fn image(&self, elf: &Path) -> Result<PathBuf, String> {
        let bin = elf.with_extension("bin");
        if !bin.is_file() && !Binutils::best_for(Target::THUMBV7EM).raw_image(elf, &bin) {
            return Err(format!("{}: kein Rohabbild (objcopy fehlt?)", elf.display()));
        }
        let bytes = std::fs::read(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        check_image(&bytes).map_err(|e| format!("{}: {e}", bin.display()))?;
        Ok(bin)
    }
}

impl Board for Stm32f401 {
    fn name(&self) -> &'static str {
        "stm32f401"
    }

    fn target(&self) -> &'static str {
        "thumbv7em"
    }

    fn builder(&self, slot: usize) -> Builder {
        Box::new(move |program, options| BRINGUP.build(program, options, slot))
    }

    fn run(&mut self, elf: &Path, options: &Options) -> Result<String, String> {
        let within = options.within();
        let text = self.run_with(elf, within, &options.console)?;
        if text.contains(super::END) {
            super::complete(text, options)
        } else {
            Err(format!("kein `takt end` binnen {} s; gelesen:\n{text}", within.as_secs()))
        }
    }

    /// Der F401 fuehrt aus dem Flash ueber den ART-Beschleuniger aus, und
    /// eine Messung haengt an der Lage des Codes (FB-367): ungeschoben, um
    /// eine halbe Zeile von 16 Byte und um anderthalb, damit sich Zeilengrenzen
    /// und Zeilen zugleich verschieben.
    fn placements(&self) -> &'static [u32] {
        &[0, 8, 24]
    }

    /// Interrupt- und Pollform auf TIM2 und RTIC (12.8): Takt als Aufgabe,
    /// darueber Funk-ISR und Treiber-Aufgabe.
    fn forms(&self) -> &'static [Form] {
        &[Form::Own, Form::Interrupt, Form::Poll, Form::Rtos, Form::Embassy]
    }
}

/// Das Sprungbrett als Rohabbild, einmal je Prozess gelesen und geprueft wie
/// ein Abbild der Anwendung. Es liegt im Stand des Bring-ups im
/// Zwischenspeicher der Abbilder; fehlt es dort, wird es gebaut.
fn trampoline() -> Result<&'static [u8], String> {
    static IMAGE: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    IMAGE.get_or_init(cached_trampoline).as_deref().map_err(Clone::clone)
}

fn cached_trampoline() -> Result<Vec<u8>, String> {
    let read = |bin: &Path| -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(bin).map_err(|e| format!("{}: {e}", bin.display()))?;
        check_table(&bytes, TRAMPOLINE..APP).map_err(|e| format!("Sprungbrett {}: {e}", bin.display()))?;
        Ok(bytes)
    };
    let stand = BRINGUP.stand()?;
    let cached = stand.join(TRAMPOLINE_IMAGE);
    if !cached.is_file() {
        missed(&stand, &cached, TRAMPOLINE_CRATE)?;
        let built = build_trampoline()?;
        let published =
            BRINGUP.enter(&stand).and_then(|()| publish_checked(&built, &cached, |b| read(b).map(|_| true)));
        let _ = std::fs::remove_file(&built);
        published?;
    }
    read(&cached)
}

/// Baut das Sprungbrett im eigenen Zielverzeichnis und liefert sein
/// Rohabbild, je Prozess unter eigenem Namen: Ein anderer Lauf liest
/// womoeglich gerade.
fn build_trampoline() -> Result<PathBuf, String> {
    let mut cargo = Command::new("cargo");
    cargo
        .args(["build", "--release", "--target", BRINGUP.triple])
        .arg("--message-format=json-render-diagnostics")
        .arg("--manifest-path")
        .arg(super::root().join(TRAMPOLINE_CRATE).join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", crate::target_dir().join("takt-trampoline"));
    let elf = super::built_executable(&mut cargo).map_err(|e| format!("Sprungbrett: {e}"))?;
    let bin = elf.with_extension(format!("{}.bin", std::process::id()));
    if !Binutils::best_for(Target::THUMBV7EM).raw_image(&elf, &bin) {
        return Err(format!("{}: kein Rohabbild (objcopy fehlt?)", elf.display()));
    }
    Ok(bin)
}

/// Ob ein Rohabbild in den Anwendungsbereich gehoert.
fn check_image(image: &[u8]) -> Result<(), String> {
    check_table(image, APP..APP_END)
}

/// Die ersten acht Byte eines Rohabbilds sind die Vektortabelle:
/// Stackzeiger im RAM, Resetvektor in `area` und ungerade (Thumb). Ein ELF
/// statt eines Rohabbilds oder ein Abbild fuer eine andere Lage faellt hier
/// auf, bevor es geschrieben wird.
fn check_table(image: &[u8], area: std::ops::Range<u32>) -> Result<(), String> {
    let word = |at: usize| image.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(sp), Some(reset)) = (word(0), word(4)) else {
        return Err("kuerzer als eine Vektortabelle".to_string());
    };
    if !RAM.contains(&sp) {
        return Err(format!("Stackzeiger {sp:#010x} zeigt nicht ins RAM"));
    }
    if !area.contains(&reset) || reset % 2 == 0 {
        return Err(format!(
            "Resetvektor {reset:#010x} liegt nicht in {:#010x}..{:#010x} oder ist nicht Thumb",
            area.start, area.end
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(sp: u32, reset: u32) -> Vec<u8> {
        [sp.to_le_bytes(), reset.to_le_bytes()].concat()
    }

    #[test]
    fn an_application_image_passes() {
        assert_eq!(check_image(&table(0x2001_0000, 0x0801_0195)), Ok(()));
    }

    /// Sektor 0 haelt der HID-Bootloader, Sektor 1 das Sprungbrett, 2 und 3
    /// das Journal (FB-385): Ein Abbild, das dorthin gelinkt ist, gehoert
    /// nicht in den Bereich der Anwendung.
    #[test]
    fn an_image_for_the_sectors_before_the_application_is_refused() {
        for reset in [0x0800_01ad, 0x0800_4195, 0x0800_8001, 0x0800_c001] {
            assert!(check_image(&table(0x2001_0000, reset)).is_err(), "{reset:#010x}");
        }
        assert_eq!(check_table(&table(0x2001_0000, 0x0800_4195), TRAMPOLINE..APP), Ok(()));
    }

    #[test]
    fn an_elf_is_not_a_raw_image() {
        assert!(check_image(b"\x7fELF\x01\x01\x01\x00").is_err());
        assert!(check_image(&table(0x2001_0000, 0x0801_0194)).is_err(), "gerade: kein Thumb");
        assert!(check_image(&[0; 4]).is_err());
    }
}
