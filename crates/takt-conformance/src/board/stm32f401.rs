//! Board 1, STM32F401 (WeAct Black Pill V1.2), vom Host aus (plan/f401.md).
//!
//! Geschrieben wird ueber den DFU-Bootloader im ROM, und die Hand braucht
//! es dafuer nicht: `TAKT` auf der Trace-Leitung laesst die laufende
//! Anwendung in den Bootloader springen (FB-275), `dfu-util` schreibt das
//! Abbild hinter den HID-Bootloader von WeAct und startet es. Der Trace
//! kommt ueber USART1 an den Adapter.
//!
//! Antwortet das Board nicht — ein Programm ohne Leitung, ein Absturz —,
//! bleibt die Hand: BOOT0 halten, NRST druecken und loslassen, BOOT0
//! loslassen. Der Lauf wartet eine Minute darauf und sagt es.
//!
//! `TAKT_F401_PORT` nennt den Port des Adapters (`COM7`), `TAKT_DFU_UTIL`
//! den Pfad zu `dfu-util`, wenn er nicht im `PATH` steht.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

use super::{Board, Bringup, Options, capture, run_bounded};

const BRINGUP: Bringup = Bringup {
    dir: "crates/takt-bringup-stm32f401",
    triple: "thumbv7em-none-eabihf",
    inputs: &["build.rs", "Cargo.toml", "Cargo.lock", "memory.x"],
    env: &[],
};

/// Die Rate der Leitung, wie `takt-board-stm32f401::uart::BAUD`: eine fuer
/// jedes Programm, damit der Host das laufende erreicht, ohne es zu kennen.
pub const BAUD: u32 = 921_600;

/// Wo die Anwendung beginnt: hinter dem HID-Bootloader in Sektor 0.
const APP: u32 = 0x0800_4000;

/// Wo sie endet: 256 KB Flash der Variante CC.
const APP_END: u32 = 0x0804_0000;

/// Das RAM des F401CC.
const RAM: std::ops::RangeInclusive<u32> = 0x2000_0000..=0x2001_0000;

/// Die DFU-Kennung des ROM-Bootloaders (AN2606).
const DFU_ID: &str = "0483:df11";

/// Wie lange der Bootloader nach dem Wunsch auf sich warten lassen darf.
const HANDBACK: Duration = Duration::from_secs(6);

/// Wie lange ein Lauf auf die Hand wartet, wenn das Board nicht antwortet.
const BY_HAND: Duration = Duration::from_secs(60);

/// Frist fuer Schreiben und Starten.
const DOWNLOAD: Duration = Duration::from_secs(120);

/// Frist fuer den Trace nach dem Start.
const TRACE: Duration = Duration::from_secs(30);

/// Board 1 am Host.
#[derive(Clone, Debug)]
pub struct Stm32f401 {
    port: String,
    dfu_util: String,
}

impl Stm32f401 {
    /// Das Board an `TAKT_F401_PORT`; `None` ohne die Variable.
    pub fn from_env() -> Option<Stm32f401> {
        let port = std::env::var("TAKT_F401_PORT").ok()?;
        let dfu_util = std::env::var("TAKT_DFU_UTIL").unwrap_or_else(|_| "dfu-util".to_string());
        Some(Stm32f401 { port, dfu_util })
    }

    /// Der Port des Adapters.
    pub fn port(&self) -> &str {
        &self.port
    }

    /// Steht das Board im DFU-Bootloader?
    pub fn in_bootloader(&self) -> Result<bool, String> {
        let listed = run_bounded(&self.dfu_util, &["-l"], Duration::from_secs(15))
            .map_err(|e| format!("{} -l: {e}", self.dfu_util))?;
        Ok(listed.contains(&format!("[{DFU_ID}]")))
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
            self.port,
            BY_HAND.as_secs()
        );
        if self.wait_for_bootloader(BY_HAND)? {
            return Ok(());
        }
        Err(format!("das Board kam nicht in den DFU-Bootloader ({DFU_ID}); siehe plan/f401.md 2"))
    }

    /// `TAKT` auf die Leitung: Die laufende Anwendung springt in den Bootloader.
    fn hand_back(&self) -> Result<(), String> {
        let port = &self.port;
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

    fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        BRINGUP.build(program, options)
    }

    fn run(&mut self, elf: &Path, _options: &Options) -> Result<String, String> {
        let bin = self.image(elf)?;
        self.to_bootloader()?;
        let address = format!("{APP:#010x}:leave");
        // Der Adapter haelt das Board an, statt Bytes zu verlieren, wenn der
        // Wirt nicht abholt (FB-306).
        let text = capture(&self.port, BAUD, serialport::FlowControl::Software, TRACE, || {
            let args = ["-a", "0", "-d", DFU_ID, "-s", &address, "-D", &bin.to_string_lossy()];
            run_bounded(&self.dfu_util, &args, DOWNLOAD).map(|_| ()).map_err(|e| format!("{}: {e}", self.dfu_util))
        })?;
        if text.contains(super::END) {
            super::complete(text)
        } else {
            Err(format!("kein `takt end` binnen {} s; gelesen:\n{text}", TRACE.as_secs()))
        }
    }
}

/// Ob ein Rohabbild in den Anwendungsbereich gehoert.
///
/// Die ersten acht Byte sind die Vektortabelle: Stackzeiger im RAM,
/// Resetvektor im Anwendungsbereich und ungerade (Thumb). Ein ELF statt
/// eines Rohabbilds oder ein Abbild fuer `0x0800_0000` faellt hier auf,
/// bevor `dfu-util` es schreibt.
fn check_image(image: &[u8]) -> Result<(), String> {
    let word = |at: usize| image.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(sp), Some(reset)) = (word(0), word(4)) else {
        return Err("kuerzer als eine Vektortabelle".to_string());
    };
    if !RAM.contains(&sp) {
        return Err(format!("Stackzeiger {sp:#010x} zeigt nicht ins RAM"));
    }
    if !(APP..APP_END).contains(&reset) || reset % 2 == 0 {
        return Err(format!("Resetvektor {reset:#010x} liegt nicht im Anwendungsbereich oder ist nicht Thumb"));
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
        assert_eq!(check_image(&table(0x2001_0000, 0x0800_4195)), Ok(()));
    }

    #[test]
    fn an_image_for_the_bootloader_sector_is_refused() {
        assert!(check_image(&table(0x2001_0000, 0x0800_01ad)).is_err());
    }

    #[test]
    fn an_elf_is_not_a_raw_image() {
        assert!(check_image(b"\x7fELF\x01\x01\x01\x00").is_err());
        assert!(check_image(&table(0x2001_0000, 0x0800_4194)).is_err(), "gerade: kein Thumb");
        assert!(check_image(&[0; 4]).is_err());
    }
}
