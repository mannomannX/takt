//! Das Protokoll des WeAct HID-Bootloaders.
//!
//! **Warum es eigenen Code bekommt.** Der Bootloader spricht ein sehr
//! kleines Protokoll: ein Kommando, dann Seiten zu 1024 Byte, jede in
//! 64-Byte-Paketen. Ein fertiges Werkzeug dafuer gibt es in Rust nicht,
//! und die Alternative — ein Python-Skript aus dem Netz — waere eine
//! Abhaengigkeit, die niemand liest und niemand testet.
//!
//! Die Rahmenlogik steht hier und ist auf dem Wirt pruefbar; der
//! USB-Zugriff liegt eine Ebene darueber. Dieselbe Trennung wie zwischen
//! `takt-board-support` und dem Board-Crate, aus demselben Grund: Ein
//! Fehler in der Paketaufteilung faellt sonst erst am Board auf, und dort
//! sieht man nur, dass nichts passiert.

/// Die Kennung des Bootloaders am USB.
pub const VENDOR_ID: u16 = 0x0483;
/// Die Produktkennung.
pub const PRODUCT_ID: u16 = 0x572A;

/// Groesse eines HID-Pakets in Byte.
///
/// Der Bootloader nimmt genau so viel je Uebertragung; kuerzere Pakete
/// verwirft er, laengere kennt HID nicht.
pub const PACKET: usize = 64;

/// Groesse einer Flash-Seite in Byte.
///
/// Der Bootloader schreibt seitenweise. Eine unvollstaendige Seite muss
/// aufgefuellt werden — sonst steht im Flash, was vorher da war.
pub const PAGE: usize = 1024;

/// Wohin die Anwendung geschrieben wird.
///
/// Der Bootloader belegt die ersten 16 KiB; alles davor ist er selbst,
/// und ein Schreibversuch dorthin macht das Board unbrauchbar. Die
/// Adresse steht auch in `memory.x` des Bring-up-Programms — beide
/// muessen uebereinstimmen, sonst laeuft die Anwendung an der falschen
/// Stelle.
pub const APP_ORIGIN: u32 = 0x0800_4000;

/// Das Kommando, das den Schreibvorgang einleitet.
///
/// Acht Bytes: die Kennung `0x01`, dann die Zieladresse in Little Endian,
/// dann die Laenge in Seiten. Der Rest des Pakets bleibt null.
pub fn write_command(origin: u32, pages: u32) -> [u8; PACKET] {
    let mut p = [0u8; PACKET];
    p[0] = 0x01;
    p[1..5].copy_from_slice(&origin.to_le_bytes());
    p[5..9].copy_from_slice(&pages.to_le_bytes());
    p
}

/// Wie viele Seiten ein Abbild belegt.
///
/// Aufgerundet: Eine angefangene Seite ist eine ganze, weil der
/// Bootloader nicht feiner schreibt.
pub fn pages_for(len: usize) -> u32 {
    u32::try_from(len.div_ceil(PAGE)).unwrap_or(u32::MAX)
}

/// Teilt ein Abbild in Pakete, die letzte Seite mit `0xFF` aufgefuellt.
///
/// **`0xFF` und nicht null**, weil geloeschter Flash so aussieht: Eine
/// mit Nullen aufgefuellte Seite zwingt den Bootloader, Bits zu
/// schreiben, die er nicht schreiben muss — und auf manchen Chips ist
/// das Programmieren einer Null in eine bereits geloeschte Zelle
/// unnoetiger Verschleiss.
pub fn packets(image: &[u8]) -> Vec<[u8; PACKET]> {
    let padded = pages_for(image.len()) as usize * PAGE;
    let mut out = Vec::with_capacity(padded / PACKET);
    for chunk_start in (0..padded).step_by(PACKET) {
        let mut p = [0xFFu8; PACKET];
        for (i, slot) in p.iter_mut().enumerate() {
            if let Some(b) = image.get(chunk_start + i) {
                *slot = *b;
            }
        }
        out.push(p);
    }
    out
}

/// Prueft, ob ein Abbild plausibel ist, bevor es geschrieben wird.
///
/// **Der Schutz vor dem teuersten Fehler.** Ein Abbild, das an der
/// falschen Adresse gelinkt wurde, ueberschreibt entweder den Bootloader
/// oder laeuft nie an. Beides sieht man erst danach — und im ersten Fall
/// hilft nur noch SWD. Die ersten acht Byte sagen genug: Stackzeiger ins
/// RAM, Resetvektor in den Anwendungsbereich.
pub fn check(image: &[u8]) -> Result<(), ImageError> {
    if image.len() < 8 {
        return Err(ImageError::TooShort);
    }
    let sp = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    let reset = u32::from_le_bytes([image[4], image[5], image[6], image[7]]);

    // Der Stackzeiger zeigt ins SRAM (0x2000_0000 .. +64 KiB).
    if !(0x2000_0000..=0x2001_0000).contains(&sp) {
        return Err(ImageError::BadStackPointer(sp));
    }
    // Der Resetvektor zeigt hinter den Bootloader und ist ungerade
    // (Thumb-Modus; eine gerade Adresse waere ein Sprung in den
    // ARM-Modus, den der Cortex-M nicht hat).
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
    /// Der Resetvektor liegt vor dem Anwendungsbereich — das Abbild ist
    /// fuer 0x0800_0000 gelinkt und wuerde den Bootloader ueberschreiben.
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
                 wuerde den Bootloader ueberschreiben. `memory.x` pruefen."
            ),
            ImageError::ResetNotThumb(r) => write!(f, "Resetvektor {r:#010x} ist gerade; Thumb verlangt ungerade"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Abbild, wie `llvm-objcopy` es aus dem Bring-up-Programm macht.
    fn good_image(len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len.max(8)];
        v[0..4].copy_from_slice(&0x2001_0000u32.to_le_bytes());
        v[4..8].copy_from_slice(&0x0800_4195u32.to_le_bytes());
        v
    }

    #[test]
    fn a_well_linked_image_passes() {
        assert_eq!(check(&good_image(3704)), Ok(()));
    }

    /// **Der Fehler, der das Board unbrauchbar machte.**
    ///
    /// Ein Abbild fuer 0x0800_0000 ueberschriebe den Bootloader — danach
    /// hilft nur noch SWD. Genau davor schuetzt die Pruefung.
    #[test]
    fn an_image_linked_for_the_bootloader_area_is_refused() {
        let mut v = good_image(1024);
        v[4..8].copy_from_slice(&0x0800_0195u32.to_le_bytes());
        assert_eq!(check(&v), Err(ImageError::ResetBeforeApp(0x0800_0195)));
    }

    /// Ein ELF statt eines Rohabbilds faellt am Stackzeiger auf: Die
    /// ersten Bytes sind dort `7f 45 4c 46`.
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

    #[test]
    fn pages_round_up() {
        assert_eq!(pages_for(1), 1);
        assert_eq!(pages_for(PAGE), 1);
        assert_eq!(pages_for(PAGE + 1), 2);
        assert_eq!(pages_for(3704), 4, "3704 Byte sind vier Seiten");
    }

    /// Die Pakete decken ganze Seiten ab, auch wenn das Abbild
    /// dazwischen endet.
    #[test]
    fn packets_fill_whole_pages() {
        let ps = packets(&good_image(3704));
        assert_eq!(ps.len(), 4 * PAGE / PACKET, "vier Seiten zu je 16 Paketen");
    }

    /// **Aufgefuellt wird mit `0xFF`.**
    ///
    /// So sieht geloeschter Flash aus. Nullen zu schreiben waere
    /// unnoetiger Verschleiss.
    #[test]
    fn the_tail_is_filled_with_erased_flash() {
        let ps = packets(&good_image(10));
        let last = ps.last().expect("Pakete");
        assert!(last.iter().all(|b| *b == 0xFF), "das letzte Paket ist leer");
    }

    /// Der Inhalt steht unveraendert in den Paketen.
    #[test]
    fn the_image_survives_the_split() {
        let img = good_image(100);
        let ps = packets(&img);
        let flat: Vec<u8> = ps.iter().flatten().copied().collect();
        assert_eq!(&flat[..100], &img[..], "die ersten 100 Byte sind das Abbild");
    }

    #[test]
    fn the_write_command_carries_address_and_length() {
        let c = write_command(APP_ORIGIN, 4);
        assert_eq!(c[0], 0x01);
        assert_eq!(&c[1..5], &APP_ORIGIN.to_le_bytes());
        assert_eq!(&c[5..9], &4u32.to_le_bytes());
        assert!(c[9..].iter().all(|b| *b == 0), "der Rest bleibt null");
    }
}
