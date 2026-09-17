//! Telemetrie ueber USB-Serial-JTAG (12.3).
//!
//! Der Chip bringt die Schnittstelle mit; `esp-println` bedient ihr
//! FIFO. Dieselben Aufrufe wie beim F401 (`write`, `write_i64`,
//! `newline`), damit die Bring-up-Programme beider Boards gleich lesen.

use esp_println::Printer;

/// Die serielle Ausgabe.
#[derive(Clone, Copy, Debug, Default)]
pub struct Telemetry;

impl Telemetry {
    /// Die Schnittstelle ist mit dem Chip da; hier gibt es nichts einzurichten.
    pub fn new() -> Telemetry {
        Telemetry
    }

    /// Ein Byte.
    pub fn write_byte(&mut self, b: u8) {
        Printer::write_bytes(&[b]);
    }

    /// Ein Text.
    pub fn write(&mut self, s: &str) {
        Printer::write_bytes(s.as_bytes());
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
        Printer::write_bytes(&buf[i..]);
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
