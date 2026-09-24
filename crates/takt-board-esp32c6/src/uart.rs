//! Telemetrie ueber USB-Serial-JTAG (12.3, 13.8): die Leitung hinter
//! [`takt_rt_baremetal::Telemetry`].
//!
//! Das FIFO nimmt 64 Byte und gibt ein Paket ab, wenn der Host es abholt.
//! Wer darauf wartet, haelt den Tick an (FB-200, FB-227); darum hier nur
//! „nimmt das FIFO noch ein Byte?" und „Paket abschicken", ohne Zeit. Den
//! Ring, die Zahlen und das Verwerfen macht `takt-rt-baremetal` fuer
//! jedes Board gleich.

use esp_hal::Blocking;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use takt_rt_baremetal::Port;

/// Der Ring vor der Leitung: 2 KiB fangen einen Host ab, der 20 ms
/// lang nicht liest, bei 500 us Tick und einer Zeitzeile je Tick.
const RING: usize = 2048;

/// Die Leitung.
pub struct UsbJtag {
    port: UsbSerialJtag<'static, Blocking>,
    /// Seit dem letzten Abschicken kam ein Byte ins FIFO.
    pending: bool,
}

impl UsbJtag {
    /// Bindet die Schnittstelle; sie ist mit dem Chip da.
    pub fn new(usb: USB_DEVICE<'static>) -> UsbJtag {
        UsbJtag { port: UsbSerialJtag::new(usb), pending: false }
    }
}

impl Port for UsbJtag {
    fn try_write(&mut self, b: u8) -> bool {
        let taken = self.port.write_byte_nb(b).is_ok();
        self.pending |= taken;
        taken
    }

    fn flush(&mut self) {
        // `wr_done` nur fuer ein angefangenes Paket bei freiem FIFO: Bei
        // belegtem Endpunkt — der Host hat das letzte Paket noch nicht
        // abgeholt — setzt es den Endpunkt fest, bis der Block zurueckgesetzt
        // wird (FB-267); genau das passiert unter Last, wenn der Ring voll
        // ist und vor jedem Byte die Leitung versucht wird.
        if self.pending && USB_DEVICE::regs().ep1_conf().read().serial_in_ep_data_free().bit_is_set() {
            let _ = self.port.flush_tx_nb();
            self.pending = false;
        }
    }
}

/// Die Telemetrie des Boards.
pub type Telemetry = takt_rt_baremetal::Telemetry<UsbJtag, RING>;

/// Die Telemetrie ueber USB-Serial-JTAG.
pub fn telemetry(usb: USB_DEVICE<'static>) -> Telemetry {
    Telemetry::new(UsbJtag::new(usb))
}
