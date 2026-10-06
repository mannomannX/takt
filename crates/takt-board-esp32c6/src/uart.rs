//! Telemetrie ueber USB-Serial-JTAG (12.3, 13.8): die Leitung hinter
//! [`takt_rt_baremetal::Telemetry`].
//!
//! Das FIFO nimmt 64 Byte und gibt ein Paket ab, wenn der Host es abholt.
//! Wer darauf wartet, haelt den Tick an (FB-200, FB-227); darum hier nur
//! „nimmt das FIFO noch ein Byte?" und „Paket abschicken", ohne Zeit. Den
//! Ring, die Zahlen und das Verwerfen macht `takt-rt-baremetal` fuer
//! jedes Board gleich.
//!
//! **Der Vertrag der Leitung** (Espressif, `usb_serial_jtag_ll.h`; FB-267,
//! FB-272). Ein Byte nur, solange `serial_in_ep_data_free` steht. Ein
//! volles FIFO von 64 Byte schickt sich selbst ab, und ein 64-Byte-Paket
//! ist fuer den Host eine *unvollstaendige* USB-Transaktion, die er erst
//! mit einem kuerzeren oder einem leeren Paket ausliefert — genau das
//! Verstummen unter Last, bei dem alte Bytes spaeter nachkamen. Darum
//! bleibt ein Paket hier unter 64 Byte und geht immer ueber `wr_done`:
//! Jedes Paket ist eine ganze Transaktion. Und nach `wr_done` nimmt das
//! FIFO das naechste Paket erst, wenn das Rohflag `serial_in_empty` das
//! Abholen meldet; `serial_in_ep_data_free` allein trog unter Last. Der
//! Interrupt dazu maskiert nur und weckt den Kern (FB-264); das Flag liest
//! die Leitung selbst, so geht es auch ohne Interrupts, etwa im Panic-Pfad.
//!
//! Die Gegenrichtung liest die Leitung bei jedem Abschicken leer, damit
//! ein ungelesenes Paket den Host nicht blockiert; steht `TAKT` darin,
//! setzt sich der Chip zurueck (FB-266). Jedes Byte geht ausserdem in ein
//! FIFO, aus dem die Schleife Tunes liest ([`console_byte`], 8.4).

use esp_hal::Blocking;
use esp_hal::interrupt::Priority;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use takt_board_support::ByteFifo;
use takt_rt_baremetal::Port;

use crate::usb::{Magic, chip_reset};

/// Die Gegenrichtung fuer die Schleife. Ist es voll, verwirft die Leitung
/// das Byte; eine Tune-Zeile, der es fehlt, ist keine mehr.
static RX: ByteFifo<256> = ByteFifo::new();

/// Das naechste Byte der Gegenrichtung, `None`, wenn keines wartet: die
/// Quelle der Tunes ([`takt_rt_baremetal::Console`]).
pub fn console_byte() -> Option<u8> {
    RX.pop()
}

/// Der Ring vor der Leitung: 2 KiB fangen einen Host ab, der 20 ms
/// lang nicht liest, bei 500 us Tick und einer Zeitzeile je Tick.
const RING: usize = 2048;

/// Bytes je Paket, unter der Puffergroesse von 64.
const PACKET: u8 = 63;

/// Die Leitung.
pub struct UsbJtag {
    port: UsbSerialJtag<'static, Blocking>,
    /// Bytes im FIFO seit dem letzten Abschicken.
    filled: u8,
    /// Abgeschickt und vom Host noch nicht abgeholt.
    in_flight: bool,
    /// Seit einem Bus-Reset, bis der Host wieder abfragt (FB-311).
    stalled: bool,
    /// Der Wunsch nach einem Chip-Reset, byteweise aus der Gegenrichtung.
    magic: Magic,
}

impl UsbJtag {
    /// Bindet die Schnittstelle; sie ist mit dem Chip da.
    pub fn new(usb: USB_DEVICE<'static>) -> UsbJtag {
        let mut port = UsbSerialJtag::new(usb);
        // Nur, was die Leitung selbst armiert: Eine fremde Quelle, die der
        // Handler nicht quittiert, waere ein Sturm.
        USB_DEVICE::regs().int_ena().reset();
        port.set_interrupt_handler(on_packet_taken);
        UsbJtag { port, filled: 0, in_flight: false, stalled: false, magic: Magic::default() }
    }

    /// Nimmt das Abholen zur Kenntnis: Das FIFO ist wieder frei.
    ///
    /// **Nach einem Bus-Reset** (die Neuanmeldung nach dem Wecken, FB-311)
    /// ist der Endpunkt leer, meldet sich aber nicht frei; er nimmt kein
    /// Byte, und die Konsole bliebe stumm. Frei wird er durch ein
    /// `wr_done`, das aber erst wirkt, wenn der Host den Endpunkt abfragt —
    /// also beim ersten IN-Token danach, nicht beim Reset selbst. Was im
    /// FIFO stand, hat der Host nie gesehen.
    #[esp_hal::ram]
    fn settle(&mut self) {
        let regs = USB_DEVICE::regs();
        let raw = regs.int_raw().read();
        if raw.usb_bus_reset().bit_is_set() {
            regs.int_clr().write(|w| w.usb_bus_reset().clear_bit_by_one().in_token_rec_in_ep1().clear_bit_by_one());
            self.filled = 0;
            self.in_flight = false;
            self.stalled = true;
            return;
        }
        if self.stalled && raw.in_token_rec_in_ep1().bit_is_set() {
            self.stalled = false;
            if !regs.ep1_conf().read().serial_in_ep_data_free().bit_is_set() {
                regs.int_clr().write(|w| w.serial_in_empty().clear_bit_by_one());
                regs.ep1_conf().write(|w| w.wr_done().set_bit());
                self.in_flight = true;
                regs.int_ena().modify(|_, w| w.serial_in_empty().set_bit());
            }
            return;
        }
        if self.in_flight && raw.serial_in_empty().bit_is_set() {
            regs.int_clr().write(|w| w.serial_in_empty().clear_bit_by_one());
            self.in_flight = false;
        }
    }

    /// Leert den Empfangspuffer, hoechstens ein Paket je Aufruf (4.1).
    #[esp_hal::ram]
    fn drain_rx(&mut self) {
        let regs = USB_DEVICE::regs();
        for _ in 0..64 {
            if !regs.ep1_conf().read().serial_out_ep_data_avail().bit_is_set() {
                return;
            }
            let b = regs.ep1().read().rdwr_byte().bits();
            if self.magic.feed(b) {
                chip_reset();
            }
            let _ = RX.push(b);
        }
    }
}

/// Der Host hat das Paket abgeholt: nur wecken und maskieren, das Flag
/// liest [`UsbJtag::settle`].
#[esp_hal::handler(priority = Priority::Priority1)]
fn on_packet_taken() {
    USB_DEVICE::regs().int_ena().modify(|_, w| w.serial_in_empty().clear_bit());
}

impl Port for UsbJtag {
    #[esp_hal::ram]
    fn try_write(&mut self, b: u8) -> bool {
        self.settle();
        if self.in_flight || self.filled == PACKET || self.port.write_byte_nb(b).is_err() {
            return false;
        }
        self.filled += 1;
        true
    }

    #[esp_hal::ram]
    fn flush(&mut self) {
        self.settle();
        self.drain_rx();
        if self.filled == 0 || self.in_flight {
            return;
        }
        let regs = USB_DEVICE::regs();
        regs.int_clr().write(|w| w.serial_in_empty().clear_bit_by_one());
        self.in_flight = true;
        self.filled = 0;
        let _ = self.port.flush_tx_nb();
        regs.int_ena().modify(|_, w| w.serial_in_empty().set_bit());
    }

    /// Nichts mehr im FIFO, und der Host hat das letzte Paket abgeholt.
    #[esp_hal::ram]
    fn idle(&mut self) -> bool {
        self.settle();
        self.filled == 0 && !self.in_flight
    }
}

/// Die Telemetrie des Boards.
pub type Telemetry = takt_rt_baremetal::Telemetry<UsbJtag, RING>;

/// Die Telemetrie ueber USB-Serial-JTAG.
pub fn telemetry(usb: USB_DEVICE<'static>) -> Telemetry {
    Telemetry::new(UsbJtag::new(usb))
}
