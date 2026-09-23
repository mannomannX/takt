//! Telemetrie ueber USART1 (12.3): die Leitung hinter
//! [`takt_rt_baremetal::Telemetry`].
//!
//! 12.3 nennt „Telemetrie ueber UART/CAN/Ethernet, reduziert
//! (Zustandspfad, Faults, `pub var`)". Fuer den Bring-up ist das der
//! einzige Weg, vom Board etwas zu erfahren: Ohne Debugger sieht man
//! sonst nur, ob die LED blinkt.
//!
//! **Wartend, aber nicht unbegrenzt.** Ein USART ohne FIFO nimmt ein Byte
//! je `TXE`; die Leitung wartet darauf mit Schranke, denn eine
//! unbegrenzte Warteschleife auf ein Hardware-Flag ist der sicherste Weg,
//! ein Programm stillzulegen (FB-139). Ein verlorenes Byte ist besser
//! als ein stehendes Programm — die Telemetrie ist Diagnose, nicht
//! Steuerung (12.2); den Ring und das Zaehlen macht `takt-rt-baremetal`.
//!
//! Pins: PA9 (TX) und PA10 (RX), die Standardbelegung von USART1 auf
//! diesem Board.

use stm32f4::stm32f401::{GPIOA, RCC, USART1};
use takt_rt_baremetal::Port;

/// Der Ring vor der Leitung: eine Zeile je Tick bei 10 ms passt in
/// 115200 Baud, der Ring faengt die Buendel dazwischen.
const RING: usize = 1024;

/// Die Leitung.
pub struct Usart1 {
    usart: USART1,
}

impl Usart1 {
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
    ) -> Result<Usart1, takt_board_support::uart::BaudError> {
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

        Ok(Usart1 { usart })
    }
}

impl Port for Usart1 {
    fn try_write(&mut self, b: u8) -> bool {
        // Bei 115200 Baud dauert ein Byte rund 87 us, bei 84 MHz also
        // etwa 7300 Zyklen; 100 000 Durchlaeufe sind ein Vielfaches davon.
        for _ in 0..100_000u32 {
            if self.usart.sr().read().txe().bit_is_set() {
                self.usart.dr().write(|w| unsafe { w.dr().bits(u16::from(b)) });
                return true;
            }
        }
        false
    }
}

/// Die Telemetrie des Boards.
pub type Telemetry = takt_rt_baremetal::Telemetry<Usart1, RING>;

/// Die Telemetrie ueber USART1 auf PA9.
pub fn telemetry(
    usart: USART1,
    gpioa: &GPIOA,
    rcc: &RCC,
    pclk_hz: u32,
    baud: u32,
) -> Result<Telemetry, takt_board_support::uart::BaudError> {
    Ok(Telemetry::new(Usart1::new(usart, gpioa, rcc, pclk_hz, baud)?))
}
