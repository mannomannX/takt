//! Die Nutzer-LED (12.3).
//!
//! Kein Sprachkonstrukt, sondern das Werkzeug des Bring-up: Eine LED, die
//! im Tickrhythmus blinkt, sagt ohne Debugger, ob der Timer laeuft. Ein
//! stehendes Licht heisst Absturz, ein falscher Rhythmus heisst falsche
//! Periode — beides sieht man aus drei Metern.

use stm32f4::stm32f401::{GPIOC, RCC};

/// Die LED an einem GPIO-Pin.
pub struct Led {
    gpioc: GPIOC,
    pin: u8,
    active_low: bool,
}

impl Led {
    /// Richtet den Pin als Ausgang ein.
    ///
    /// Nimmt den Port aus [`crate::Board`]; heute ist nur Port C
    /// unterstuetzt, weil die bekannten F401-Boards ihre LED dort haben.
    pub fn new(gpioc: GPIOC, rcc: &RCC, board: crate::Board) -> Led {
        rcc.ahb1enr().modify(|_, w| w.gpiocen().set_bit());
        let pin = board.led.1;
        // Ausgang (0b01) im Moder-Register, zwei Bits je Pin.
        gpioc.moder().modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << (pin * 2))) | (0b01 << (pin * 2))) });
        let led = Led { gpioc, pin, active_low: board.led_active_low };
        led.off();
        led
    }

    /// Schaltet die LED ein.
    pub fn on(&self) {
        self.drive(!self.active_low);
    }

    /// Schaltet sie aus.
    pub fn off(&self) {
        self.drive(self.active_low);
    }

    /// Kehrt den Zustand um.
    pub fn toggle(&self) {
        let high = self.gpioc.odr().read().bits() & (1 << self.pin) != 0;
        self.drive(!high);
    }

    /// Setzt den Pin.
    ///
    /// Ueber `BSRR` statt `ODR`: Das Register setzt und loescht einzelne
    /// Bits in einem Schreibvorgang, ohne den Rest zu lesen. Ein
    /// Read-Modify-Write auf `ODR` waere hier harmlos, aber die Gewohnheit
    /// ist schlecht — an einem Register mit W1C-Bits (FB-11) zerstoert
    /// sie Ereignisse.
    fn drive(&self, high: bool) {
        let shift = if high { self.pin } else { self.pin + 16 };
        self.gpioc.bsrr().write(|w| unsafe { w.bits(1 << shift) });
    }
}
