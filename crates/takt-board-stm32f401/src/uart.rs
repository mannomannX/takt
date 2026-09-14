//! Telemetrie ueber USART1 (12.3).
//!
//! 12.3 nennt „Telemetrie ueber UART/CAN/Ethernet, reduziert
//! (Zustandspfad, Faults, `pub var`)". Fuer den Bring-up ist das der
//! einzige Weg, vom Board etwas zu erfahren: Ohne Debugger sieht man
//! sonst nur, ob die LED blinkt.
//!
//! **Blockierend, und das ist hier richtig.** Ein `Sink` der Runtime darf
//! nie blockieren (12.2: „wer nicht mitkommt, verwirft und zaehlt"), weil
//! er sonst den naechsten Tick verschiebt. Dieses Modul ist kein `Sink`,
//! sondern das Werkzeug fuer den Bring-up — dort will man jede Zeile
//! sehen, auch die letzte vor einem Absturz. Wenn die Tickschleife steht,
//! kommt der nicht-blockierende Aufsatz darueber.
//!
//! Pins: PA9 (TX) und PA10 (RX), die Standardbelegung von USART1 auf
//! diesem Board.

use stm32f4::stm32f401::{GPIOA, RCC, USART1};

/// Der Telemetriekanal.
pub struct Telemetry {
    usart: USART1,
}

impl Telemetry {
    /// Richtet USART1 auf PA9/PA10 ein.
    ///
    /// `baud` ist die Zielbaudrate, `pclk_hz` der Takt des Busses, an dem
    /// USART1 haengt (APB2, auf diesem Board gleich dem Kerntakt).
    pub fn new(
        usart: USART1,
        gpioa: &GPIOA,
        rcc: &RCC,
        pclk_hz: u32,
        baud: u32,
    ) -> Result<Telemetry, takt_board_support::uart::BaudError> {
        rcc.ahb1enr().modify(|_, w| w.gpioaen().set_bit());
        rcc.apb2enr().modify(|_, w| w.usart1en().set_bit());
        // Errata: zwei APB-Takte Verzoegerung, siehe lib.rs.
        let _ = rcc.apb2enr().read();

        // **Nur PA9 (TX).** PA10 (RX) bleibt unberuehrt: Die Telemetrie
        // sendet, sie empfaengt nichts. Einen Pin als Alternativfunktion
        // zu belegen, ohne den Empfaenger einzuschalten, hiesse ihn zu
        // besetzen, ohne ihn zu benutzen — und ein floating RX erzeugt
        // Framing-Fehler, sobald jemand das Register liest.
        gpioa.moder().modify(|_, w| unsafe { w.moder9().bits(0b10) });
        gpioa.afrh().modify(|_, w| unsafe { w.afrh9().bits(7) });
        // Hohe Flankensteilheit: Bei 115200 Baud unnoetig, bei 921600
        // (die `takt-board-support` als erreichbar fuehrt) nicht mehr.
        gpioa.ospeedr().modify(|_, w| unsafe { w.ospeedr9().bits(0b10) });

        // Der Teiler steht als Festkommazahl: obere 12 Bit ganzzahlig,
        // untere 4 als Sechzehntel. Die Rechnung liegt in
        // `takt-board-support`, wo sie getestet ist — und wo auffiel, dass
        // 9600 und 19200 Baud bei 84 MHz gar nicht darstellbar sind.
        let brr = takt_board_support::uart::divisor(pclk_hz, baud)?;
        usart.brr().write(|w| unsafe { w.bits(brr) });
        usart.cr1().modify(|_, w| {
            w.te().set_bit();
            w.ue().set_bit()
        });

        Ok(Telemetry { usart })
    }

    /// Schreibt ein Byte und wartet, bis es heraus ist.
    pub fn write_byte(&mut self, b: u8) {
        while self.usart.sr().read().txe().bit_is_clear() {}
        self.usart.dr().write(|w| unsafe { w.dr().bits(u16::from(b)) });
    }

    /// Schreibt eine Zeichenkette.
    pub fn write(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_byte(b);
        }
    }

    /// Schreibt eine Zahl in Dezimalschreibweise.
    ///
    /// Ohne `format!`: Das braeuchte einen Allokator, und 12.3 sagt „kein
    /// Heap". Die Ziffern entstehen darum in einem Puffer auf dem Stack.
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

    /// Schreibt eine Zahl mit Vorzeichen.
    pub fn write_i64(&mut self, n: i64) {
        if n < 0 {
            self.write_byte(b'-');
        }
        self.write_u64(n.unsigned_abs());
    }

    /// Schliesst eine Zeile ab.
    pub fn newline(&mut self) {
        self.write("\r\n");
    }
}
