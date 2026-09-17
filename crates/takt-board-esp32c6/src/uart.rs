//! Telemetrie ueber USB-Serial-JTAG (12.3, 13.8).
//!
//! **Verlustfrei, solange ein Host liest.** `esp-println` gibt Bytes nach
//! einer kurzen Wartezeit auf, wenn der Host das FIFO nicht schnell genug
//! leert — im Konformitaetslauf fehlte so das Ende einer Zeile (FB-200).
//! Hier wartet jedes Byte, bis das FIFO Platz hat, bis zu 50 ms; erst dann
//! gilt der Host als abwesend, und eine Sekunde lang wird nichts
//! geschrieben, damit ein Board ohne Leser nicht an jeder Zeile steht.
//!
//! Gesendet wird an Zeilenenden und wenn das FIFO voll ist: ein USB-Paket
//! je Zeile statt je Textstueck, denn jedes Paket braucht einen Abruf des
//! Hosts. Dieselben Aufrufe wie beim F401 (`write`, `write_i64`,
//! `newline`), damit die Bring-up-Programme beider Boards gleich lesen.

use esp_hal::Blocking;
use esp_hal::peripherals::USB_DEVICE;
use esp_hal::timer::systimer::{SystemTimer, Unit};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;

/// Warten auf Platz im FIFO, in SYSTIMER-Schritten zu 62,5 ns: 50 ms.
const WAIT: u64 = 800_000;
/// Schweigen nach einem Timeout: eine Sekunde.
const SILENCE: u64 = 16_000_000;

/// Die serielle Ausgabe.
pub struct Telemetry {
    port: UsbSerialJtag<'static, Blocking>,
    silent_until: u64,
    dropped: u32,
}

impl Telemetry {
    /// Bindet die Schnittstelle; sie ist mit dem Chip da.
    pub fn new(usb: USB_DEVICE<'static>) -> Telemetry {
        Telemetry { port: UsbSerialJtag::new(usb), silent_until: 0, dropped: 0 }
    }

    /// Ein Byte; am Zeilenende geht das Paket ab.
    pub fn write_byte(&mut self, b: u8) {
        let start = SystemTimer::unit_value(Unit::Unit0);
        if start < self.silent_until {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        while self.port.write_byte_nb(b).is_err() {
            // Das FIFO ist voll: abschicken, dann auf den Host warten.
            let _ = self.port.flush_tx_nb();
            if SystemTimer::unit_value(Unit::Unit0).wrapping_sub(start) > WAIT {
                self.silent_until = start + WAIT + SILENCE;
                self.dropped = self.dropped.saturating_add(1);
                return;
            }
        }
        if b == b'\n' {
            let _ = self.port.flush_tx_nb();
        }
    }

    /// Wie viele Bytes ohne Host verworfen wurden.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Ein Text.
    pub fn write(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_byte(b);
        }
    }

    /// Eine Zahl, dezimal.
    pub fn write_u64(&mut self, mut n: u64) {
        let mut buf = [0u8; 20];
        let mut i = buf.len();
        loop {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        for b in &buf[i..] {
            self.write_byte(*b);
        }
    }

    /// Eine Zahl mit Vorzeichen, dezimal.
    pub fn write_i64(&mut self, n: i64) {
        if n < 0 {
            self.write_byte(b'-');
        }
        self.write_u64(n.unsigned_abs());
    }

    /// Zeilenende, wie beim F401.
    pub fn newline(&mut self) {
        self.write("\r\n");
    }
}

/// Fuer `write!`: Fliesskommazahlen im Trace kommen aus `core::fmt`, das
/// die kuerzeste Ziffernfolge druckt, die den Wert eindeutig zurueckgibt.
impl core::fmt::Write for Telemetry {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write(s);
        Ok(())
    }
}
