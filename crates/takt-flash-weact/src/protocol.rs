//! Das Protokoll des WeAct HID-Bootloaders.
//!
//! **Quelle:** `WeActStudio/WeAct_HID_Bootloader_F4x1`, Datei
//! `Cli/WeAct_HID_Flash_CLI.c`. Die Bootloader-Firmware selbst ist nicht
//! offengelegt; der veroeffentlichte Flasher ist damit die maassgebliche
//! Beschreibung des Drahtformats.
//!
//! **Warum das hier steht und nicht geraten ist.** Eine erste Fassung
//! dieses Moduls beruhte auf einer konstruierten Annahme — ein
//! Befehlscode `0x01`, ein Adressfeld, Seiten ohne Quittung. Sie war in
//! jedem Punkt falsch, und das Werkzeug meldete trotzdem „Geschrieben":
//! `hidapi` nimmt jedes Paket an, der Bootloader verwarf sie stumm. Die
//! Tests dazu waren wertlos, weil sie pruefen, ob der Code seiner eigenen
//! Annahme folgt.
//!
//! **Das Protokoll ist nicht das von Serasidis.** Der bekannte
//! STM32-HID-Bootloader nutzt das Magic `BTLDCMD` und VID/PID
//! `1209:BEBA`. WeAct hat abgezweigt: anderes Magic, anderer Befehlssatz,
//! andere Kennung. Ein Flasher fuer den einen spricht den anderen nicht.

/// Die Kennung des Bootloaders am USB.
pub const VENDOR_ID: u16 = 0x0483;
/// Die Produktkennung.
pub const PRODUCT_ID: u16 = 0x572A;

/// Die Firmware-Fassung, ab der das Werkzeug arbeitet.
///
/// Der Original-Flasher bricht darunter mit „Please update the firmware"
/// ab; die Pruefung steht hier, damit ein altes Board eine Aussage
/// bekommt statt eines stillen Fehlschlags.
pub const MIN_FIRMWARE: u16 = 0x0200;

/// Nutzdaten je HID-Bericht.
pub const PAYLOAD: usize = 64;

/// Ein Bericht: eine fuehrende Report-ID plus die Nutzdaten.
///
/// **Immer 65 Byte, nie 64.** Das erste Byte ist die Report-ID und
/// bleibt null; `hidapi` erwartet sie als Teil des Puffers.
pub const REPORT: usize = PAYLOAD + 1;

/// Groesse eines Flash-Sektors in Byte.
///
/// Der Bootloader schreibt sektorweise, also 16 Berichte je Sektor. Eine
/// angefangene Seite wird mit `0xFF` aufgefuellt — so sieht geloeschter
/// Flash aus.
pub const SECTOR: usize = 1024;

/// Berichte je Sektor.
pub const REPORTS_PER_SECTOR: usize = SECTOR / PAYLOAD;

/// Wohin die Anwendung geschrieben wird.
///
/// Der Bootloader belegt die ersten 16 KiB — auf dem F4 ist der erste
/// Flash-Sektor ohnehin so gross. **Das Protokoll kennt kein Adressfeld:**
/// Der Bootloader fuehrt seinen eigenen Seitenzaehler, den
/// [`Command::ResetPage`] zurueckstellt, und legt den Versatz selbst
/// darauf. Die Adresse steht hier nur, weil das Abbild dafuer gelinkt
/// sein muss — siehe [`check`].
pub const APP_ORIGIN: u32 = 0x0800_4000;

/// Die Kennung, mit der jeder Befehl beginnt.
pub const MAGIC: [u8; 6] = *b"WeAct:";

/// Die Befehle des Bootloaders.
///
/// Vollstaendig nach der Quelle, auch was dieses Werkzeug nicht braucht:
/// Eine Aufzaehlung mit Luecken laedt dazu ein, den fehlenden Wert beim
/// naechsten Mal zu raten — und genau daraus ist die erste, falsche
/// Fassung dieses Moduls entstanden.
#[allow(dead_code, reason = "vollstaendig nach der Spezifikation, nicht nach dem heutigen Bedarf")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Command {
    /// Setzt den Seitenzaehler zurueck und leitet das Schreiben ein.
    ResetPage = 0x00,
    /// Springt in die Anwendung.
    Reboot = 0x01,
    /// Fragt die Firmware-Fassung ab.
    FirmwareVersion = 0x02,
    /// Nur als Antwort: Der Sektor ist angekommen.
    Ack = 0x03,
    /// Loescht den Anwendungsbereich.
    Erase = 0x04,
}

/// Baut einen Befehlsbericht.
///
/// Sieben Byte tragen Inhalt: die Report-ID, das Magic, der Befehl. Der
/// Rest bleibt null.
pub fn command(cmd: Command) -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    r[1..7].copy_from_slice(&MAGIC);
    r[7] = cmd as u8;
    r
}

/// Ist eine Antwort die Quittung eines Sektors?
///
/// **Die Quittung hat ein eigenes Format.** Sie kommt als sieben Byte,
/// und der Status steht an Stelle 6 — waehrend die Antworten auf `read`
/// und `erase` als voller Bericht kommen und mit dem Magic beginnen. Die
/// Unregelmaessigkeit stammt aus dem Original und ist kein Fehler dieser
/// Umsetzung.
pub fn is_ack(reply: &[u8]) -> bool {
    reply.get(6) == Some(&(Command::Ack as u8))
}

/// Wie viele Sektoren ein Abbild belegt.
pub fn sectors_for(len: usize) -> usize {
    len.div_ceil(SECTOR)
}

/// Teilt ein Abbild in Berichte, den letzten Sektor mit `0xFF` gefuellt.
///
/// `0xFF` und nicht null, weil geloeschter Flash so aussieht.
pub fn reports(image: &[u8]) -> Vec<[u8; REPORT]> {
    let padded = sectors_for(image.len()) * SECTOR;
    let mut out = Vec::with_capacity(padded / PAYLOAD);
    for start in (0..padded).step_by(PAYLOAD) {
        let mut r = [0u8; REPORT];
        // r[0] bleibt die Report-ID.
        for (i, slot) in r[1..].iter_mut().enumerate() {
            *slot = image.get(start + i).copied().unwrap_or(0xFF);
        }
        out.push(r);
    }
    out
}

/// Prueft, ob ein Abbild plausibel ist, bevor es geschrieben wird.
///
/// **Der Schutz vor dem teuersten Fehler.** Ein Abbild, das fuer
/// `0x0800_0000` gelinkt wurde, traegt eine Vektortabelle, die im
/// Bootloaderbereich laege — der Bootloader schriebe es an seinen eigenen
/// Platz, und danach hilft nur noch SWD. Die ersten acht Byte sagen
/// genug.
pub fn check(image: &[u8]) -> Result<(), ImageError> {
    if image.len() < 8 {
        return Err(ImageError::TooShort);
    }
    let sp = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    let reset = u32::from_le_bytes([image[4], image[5], image[6], image[7]]);

    if !(0x2000_0000..=0x2001_0000).contains(&sp) {
        return Err(ImageError::BadStackPointer(sp));
    }
    if reset < APP_ORIGIN {
        return Err(ImageError::ResetBeforeApp(reset));
    }
    if reset % 2 == 0 {
        return Err(ImageError::ResetNotThumb(reset));
    }
    Ok(())
}

/// Warum ein Abbild nicht geschrieben wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Kuerzer als eine Vektortabelle.
    TooShort,
    /// Der Stackzeiger zeigt nicht ins RAM.
    BadStackPointer(u32),
    /// Der Resetvektor liegt vor dem Anwendungsbereich.
    ResetBeforeApp(u32),
    /// Der Resetvektor ist gerade; der Cortex-M erwartet Thumb.
    ResetNotThumb(u32),
}

impl core::fmt::Display for ImageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ImageError::TooShort => write!(f, "Abbild ist kuerzer als eine Vektortabelle"),
            ImageError::BadStackPointer(sp) => {
                write!(f, "Stackzeiger {sp:#010x} zeigt nicht ins RAM — ist das ein ELF statt eines Rohabbilds?")
            }
            ImageError::ResetBeforeApp(r) => write!(
                f,
                "Resetvektor {r:#010x} liegt vor {APP_ORIGIN:#010x}: Das Abbild ist fuer 0x08000000 gelinkt und \
                 wuerde im Bootloaderbereich landen. `memory.x` pruefen."
            ),
            ImageError::ResetNotThumb(r) => write!(f, "Resetvektor {r:#010x} ist gerade; Thumb verlangt ungerade"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_image(len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len.max(8)];
        v[0..4].copy_from_slice(&0x2001_0000u32.to_le_bytes());
        v[4..8].copy_from_slice(&0x0800_4195u32.to_le_bytes());
        v
    }

    /// Der Befehlsrahmen, wie der Original-Flasher ihn baut.
    #[test]
    fn a_command_carries_the_magic_and_the_code() {
        let c = command(Command::ResetPage);
        assert_eq!(c[0], 0x00, "Report-ID");
        assert_eq!(&c[1..7], b"WeAct:");
        assert_eq!(c[7], 0x00, "ResetPage");
        assert!(c[8..].iter().all(|b| *b == 0), "der Rest bleibt null");
        assert_eq!(c.len(), 65, "immer 65 Byte, nie 64");
    }

    #[test]
    fn every_command_has_its_code() {
        assert_eq!(command(Command::Reboot)[7], 0x01);
        assert_eq!(command(Command::FirmwareVersion)[7], 0x02);
        assert_eq!(command(Command::Erase)[7], 0x04);
    }

    /// Die Quittung steht an Stelle 6, nicht am Anfang.
    #[test]
    fn an_ack_is_recognised_at_offset_six() {
        let mut reply = [0u8; 7];
        reply[6] = Command::Ack as u8;
        assert!(is_ack(&reply));

        reply[6] = 0x00;
        assert!(!is_ack(&reply), "ResetPage ist keine Quittung");
        assert!(!is_ack(&[]), "eine leere Antwort auch nicht");
    }

    #[test]
    fn sectors_round_up() {
        assert_eq!(sectors_for(1), 1);
        assert_eq!(sectors_for(SECTOR), 1);
        assert_eq!(sectors_for(SECTOR + 1), 2);
        assert_eq!(sectors_for(3816), 4);
    }

    /// Jeder Bericht traegt die Report-ID und 64 Nutzbytes.
    #[test]
    fn reports_have_a_leading_report_id() {
        let rs = reports(&good_image(3816));
        assert_eq!(rs.len(), 4 * REPORTS_PER_SECTOR, "vier Sektoren zu je 16 Berichten");
        assert!(rs.iter().all(|r| r[0] == 0x00), "jede Report-ID ist null");
        assert_eq!(rs[0][1], 0x00, "erstes Nutzbyte: Stackzeiger, niedrigstes Byte");
        assert_eq!(rs[0][4], 0x20, "viertes: Stackzeiger, hoechstes Byte");
    }

    #[test]
    fn the_tail_is_filled_with_erased_flash() {
        let rs = reports(&good_image(10));
        let last = rs.last().expect("Berichte");
        assert!(last[1..].iter().all(|b| *b == 0xFF), "die Nutzdaten des letzten Berichts sind leer");
        assert_eq!(last[0], 0x00, "die Report-ID bleibt null");
    }

    /// Der Inhalt steht unveraendert in den Berichten, um eins versetzt.
    #[test]
    fn the_image_survives_the_split() {
        let img = good_image(100);
        let rs = reports(&img);
        let flat: Vec<u8> = rs.iter().flat_map(|r| r[1..].iter().copied()).collect();
        assert_eq!(&flat[..100], &img[..], "die ersten 100 Byte sind das Abbild");
    }

    #[test]
    fn a_well_linked_image_passes() {
        assert_eq!(check(&good_image(3816)), Ok(()));
    }

    /// **Der Fehler, der das Board unbrauchbar machte.**
    #[test]
    fn an_image_linked_for_the_bootloader_area_is_refused() {
        let mut v = good_image(1024);
        v[4..8].copy_from_slice(&0x0800_0195u32.to_le_bytes());
        assert_eq!(check(&v), Err(ImageError::ResetBeforeApp(0x0800_0195)));
    }

    #[test]
    fn an_elf_is_refused() {
        let elf = [0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00];
        assert!(matches!(check(&elf), Err(ImageError::BadStackPointer(_))));
    }

    #[test]
    fn an_even_reset_vector_is_refused() {
        let mut v = good_image(1024);
        v[4..8].copy_from_slice(&0x0800_4194u32.to_le_bytes());
        assert_eq!(check(&v), Err(ImageError::ResetNotThumb(0x0800_4194)));
    }

    #[test]
    fn a_short_image_is_refused() {
        assert_eq!(check(&[0u8; 4]), Err(ImageError::TooShort));
    }
}
