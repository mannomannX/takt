//! DfuSe zum Bootloader im ROM des F401 (ST AN3156), ohne `dfu-util` (FB-464).
//!
//! Der Bootloader verlangt nach jedem Befehl `bwPollTimeout` Wartezeit,
//! rund 100 ms. `dfu-util` setzte vor jedem Block von 2 KiB die Adresse
//! neu und zahlte sie je Block zweimal: 35 KB brauchten 5,2 s. Hier steht
//! die Adresse einmal, und die Bloecke folgen mit steigender Nummer. Danach
//! liest der Host das Abbild zurueck und vergleicht es Byte fuer Byte; ein
//! Fehler beim Schreiben faellt auf, bevor das Programm laeuft.

use std::time::Duration;

use nusb::MaybeFuture;
use nusb::transfer::{ControlIn, ControlOut, ControlType, Recipient};

const DNLOAD: u8 = 1;
const UPLOAD: u8 = 2;
const GETSTATUS: u8 = 3;
const CLRSTATUS: u8 = 4;
const ABORT: u8 = 6;

/// Zustaende aus DFU 1.1, Tabelle 6.2.
const IDLE: u8 = 2;
const DNLOAD_BUSY: u8 = 4;
const DNLOAD_IDLE: u8 = 5;
const MANIFEST: u8 = 7;
const ERROR: u8 = 10;

/// Befehle im Block 0 (AN3156, 6.1).
const SET_ADDRESS: u8 = 0x21;
const ERASE: u8 = 0x41;

/// Frist fuer eine einzelne Uebertragung.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Ob ein Geraet mit `vid:pid` am Bus haengt.
pub fn present(vid: u16, pid: u16) -> Result<bool, String> {
    let mut devices = nusb::list_devices().wait().map_err(|e| format!("USB: {e}"))?;
    Ok(devices.any(|d| d.vendor_id() == vid && d.product_id() == pid))
}

/// Das interne Flash eines DfuSe-Geraets: Interface 0, Lage 0.
pub struct Dfuse {
    interface: nusb::Interface,
    /// `wTransferSize` aus dem DFU-Funktionsdeskriptor.
    transfer: usize,
    /// Die Sektoren aus dem Namen der Lage: Anfang und Groesse.
    sectors: Vec<(u32, u32)>,
}

impl Dfuse {
    /// Das erste Geraet mit `vid:pid`.
    pub fn open(vid: u16, pid: u16) -> Result<Dfuse, String> {
        let usb = |e: &dyn std::fmt::Display| format!("DfuSe {vid:04x}:{pid:04x}: {e}");
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| usb(&e))?
            .find(|d| d.vendor_id() == vid && d.product_id() == pid)
            .ok_or_else(|| usb(&"kein Geraet"))?;
        let device = info.open().wait().map_err(|e| usb(&e))?;
        let interface = device.claim_interface(0).wait().map_err(|e| usb(&e))?;
        interface.set_alt_setting(0).wait().map_err(|e| usb(&e))?;
        let alt = interface.descriptors().find(|d| d.alternate_setting() == 0).ok_or_else(|| usb(&"keine Lage 0"))?;
        // Der Funktionsdeskriptor (Typ 0x21) traegt `wTransferSize` in Byte 5
        // und 6; er gilt fuer alle Lagen und steht hinter der letzten.
        let transfer = interface
            .descriptors()
            .flat_map(|alt| alt.descriptors())
            .find(|d| d.descriptor_type() == 0x21 && d.len() >= 7)
            .map(|d| usize::from(u16::from_le_bytes([d[5], d[6]])))
            .ok_or_else(|| usb(&"kein DFU-Funktionsdeskriptor"))?;
        let index = alt.string_index().ok_or_else(|| usb(&"die Lage hat keinen Namen"))?;
        let name = device.get_string_descriptor(index, 0x0409, TIMEOUT).wait().map_err(|e| usb(&e))?;
        let sectors = sectors(&name).ok_or_else(|| usb(&format!("Speicherplan `{name}` unlesbar")))?;
        let dfu = Dfuse { interface, transfer, sectors };
        dfu.to_idle()?;
        Ok(dfu)
    }

    /// Loescht die Sektoren unter `image`, schreibt es ab `address` und
    /// liest es zurueck.
    pub fn write(&self, address: u32, image: &[u8]) -> Result<(), String> {
        let end = address + u32::try_from(image.len()).map_err(|_| "Abbild zu gross".to_string())?;
        let touched: Vec<(u32, u32)> =
            self.sectors.iter().copied().filter(|(start, size)| *start < end && start + size > address).collect();
        // Geloescht wird sektorweise: Ein Abbild, das mitten in einem Sektor
        // begaenne, loeschte, was davor liegt.
        let starts = touched.first().is_some_and(|(start, _)| *start == address);
        let fits = touched.last().is_some_and(|(start, size)| start + size >= end);
        if !starts || !fits {
            return Err(format!("{address:#010x}..{end:#010x} liegt nicht sektorbuendig im Flash des Geraets"));
        }
        let touched: Vec<u32> = touched.into_iter().map(|(start, _)| start).collect();
        for sector in touched {
            self.command(ERASE, sector)?;
        }
        self.command(SET_ADDRESS, address)?;
        for (i, block) in image.chunks(self.transfer).enumerate() {
            self.download(block_number(i)?, block)?;
            self.settle()?;
        }
        self.verify(address, image)
    }

    /// Springt nach `address` und verlaesst den Bootloader.
    pub fn leave(&self, address: u32) -> Result<(), String> {
        self.command(SET_ADDRESS, address)?;
        self.download(2, &[])?;
        // Der Bootloader meldet `dfuMANIFEST` und trennt sich; eine Antwort
        // danach gibt es nicht immer.
        match self.status() {
            Ok(s) if s.state == MANIFEST || s.state == DNLOAD_IDLE => Ok(()),
            Ok(s) => Err(format!("DfuSe: verlassen im Zustand {}", s.state)),
            Err(_) => Ok(()),
        }
    }

    /// Liest `image.len()` Byte ab `address` und vergleicht.
    fn verify(&self, address: u32, image: &[u8]) -> Result<(), String> {
        self.command(SET_ADDRESS, address)?;
        self.control_out(ABORT, 0, &[])?;
        for (i, block) in image.chunks(self.transfer).enumerate() {
            let read = self.control_in(UPLOAD, block_number(i)?, block.len())?;
            if read != block {
                let at = read.iter().zip(block).position(|(a, b)| a != b).unwrap_or(read.len().min(block.len()));
                return Err(format!(
                    "DfuSe: zurueckgelesen weicht ab bei {:#010x}",
                    u64::from(address) + (i * self.transfer + at) as u64
                ));
            }
        }
        self.control_out(ABORT, 0, &[])
    }

    /// Ein Befehl im Block 0 mit einer Adresse, ausgefuehrt.
    fn command(&self, code: u8, address: u32) -> Result<(), String> {
        let mut data = vec![code];
        data.extend_from_slice(&address.to_le_bytes());
        self.download(0, &data)?;
        self.settle()
    }

    fn download(&self, block: u16, data: &[u8]) -> Result<(), String> {
        self.control_out(DNLOAD, block, data)
    }

    /// Wartet, bis das Geraet den letzten Download ausgefuehrt hat: Der erste
    /// GETSTATUS startet ihn, jeder weitere kommt fruehestens nach der
    /// verlangten Wartezeit (DFU 1.1, 6.1.2).
    fn settle(&self) -> Result<(), String> {
        loop {
            let s = self.status()?;
            match s.state {
                DNLOAD_BUSY => std::thread::sleep(s.poll),
                DNLOAD_IDLE | IDLE => return Ok(()),
                state => return Err(format!("DfuSe: Status {} im Zustand {state}", s.status)),
            }
        }
    }

    /// Bringt das Geraet in `dfuIDLE`, auch aus einem Fehler.
    fn to_idle(&self) -> Result<(), String> {
        let s = self.status()?;
        if s.state == ERROR {
            self.control_out(CLRSTATUS, 0, &[])?;
        } else if s.state != IDLE {
            self.control_out(ABORT, 0, &[])?;
        }
        match self.status()? {
            s if s.state == IDLE => Ok(()),
            s => Err(format!("DfuSe: nicht bereit, Zustand {}", s.state)),
        }
    }

    fn status(&self) -> Result<Status, String> {
        let s = self.control_in(GETSTATUS, 0, 6)?;
        let [status, p0, p1, p2, state, _] = s[..] else { return Err("DfuSe: GETSTATUS zu kurz".into()) };
        Ok(Status { status, state, poll: Duration::from_millis(u64::from(u32::from_le_bytes([p0, p1, p2, 0]))) })
    }

    fn control_out(&self, request: u8, value: u16, data: &[u8]) -> Result<(), String> {
        let out = ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request,
            value,
            index: 0,
            data,
        };
        self.interface.control_out(out, TIMEOUT).wait().map_err(|e| format!("DfuSe: Anfrage {request}: {e}"))
    }

    fn control_in(&self, request: u8, value: u16, length: usize) -> Result<Vec<u8>, String> {
        let length = u16::try_from(length).map_err(|_| "DfuSe: Block zu gross".to_string())?;
        let input = ControlIn {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request,
            value,
            index: 0,
            length,
        };
        self.interface.control_in(input, TIMEOUT).wait().map_err(|e| format!("DfuSe: Anfrage {request}: {e}"))
    }
}

/// Eine Antwort auf GETSTATUS.
struct Status {
    status: u8,
    state: u8,
    poll: Duration,
}

/// Die Nummer des Blocks `i` einer Folge: Block 0 sind Befehle, 1 ist frei,
/// ab 2 liegen Daten ab der gesetzten Adresse (AN3156, 6.2).
fn block_number(i: usize) -> Result<u16, String> {
    u16::try_from(i + 2).map_err(|_| "DfuSe: zu viele Bloecke".to_string())
}

/// Die Sektoren aus dem Namen einer Lage, etwa
/// `@Internal Flash  /0x08000000/04*016Kg,01*064Kg,03*128Kg` (AN3156, 4.3).
pub fn sectors(name: &str) -> Option<Vec<(u32, u32)>> {
    let mut parts = name.split('/');
    let _label = parts.next()?;
    let mut at = u32::from_str_radix(parts.next()?.trim().trim_start_matches("0x"), 16).ok()?;
    let mut out = Vec::new();
    for group in parts.next()?.split(',') {
        let (count, size) = group.trim().split_once('*')?;
        let digits: String = size.chars().take_while(char::is_ascii_digit).collect();
        let unit = match size[digits.len()..].chars().next()? {
            'K' => 1024,
            'M' => 1024 * 1024,
            ' ' | 'B' => 1,
            _ => return None,
        };
        let size = digits.parse::<u32>().ok()?.checked_mul(unit)?;
        for _ in 0..count.parse::<u32>().ok()? {
            out.push((at, size));
            at = at.checked_add(size)?;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sectors_of_the_f401_are_read_from_the_name() {
        let s = sectors("@Internal Flash  /0x08000000/04*016Kg,01*064Kg,03*128Kg").expect("lesbar");
        assert_eq!(s.len(), 8);
        assert_eq!(s[1], (0x0800_4000, 16 * 1024));
        assert_eq!(s[4], (0x0801_0000, 64 * 1024));
        assert_eq!(s[5], (0x0802_0000, 128 * 1024));
    }

    #[test]
    fn a_name_without_a_plan_is_none() {
        assert!(sectors("@Option Bytes").is_none());
    }
}
