//! Die Adapter aus `takt-embed` als Treiber-Crate (plan/m11.md 2.7, 12.6,
//! 13.8; M11 Schritt 22a).
//!
//! Ein Treiberobjekt aus Adaptern besteht aus den Peripherie-Objekten eines
//! HAL; hier sind sie nachgebaut, damit `takt driver-test --crate` die
//! Adapter auf dem Wirt durch den Treiberrand faehrt (`adapters.takt`):
//!
//! - `pins/button` ist ein [`takt_embed::hal::Pin`] ueber einem Eingang,
//!   der bei jedem Lesen umschaltet; `pins/led` einer ueber einem Ausgang.
//! - `uart/rx` ist eine Schnittstelle aus `embedded-io`
//!   ([`takt_embed::io::Serial`]), gerahmt nach `lines`: je Tick eine Zeile.
//! - `isr/rx` ist ein [`takt_embed::ring::Ring`], den eine nachgebaute ISR
//!   zwischen den Ticks fuellt, gerahmt nach `cobs`: je Tick ein Rahmen.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![no_std]

use core::convert::Infallible;

use embedded_hal::digital::{ErrorType, InputPin, OutputPin};
use takt_embed::Device;
use takt_embed::frame::{Cobs, Framed, Lines, Source};
use takt_embed::hal::Pin;
use takt_embed::io::Serial;
use takt_embed::ring::Ring;

/// Ein Eingang, der bei jedem Lesen umschaltet.
#[derive(Debug, Default)]
pub struct Toggling {
    high: bool,
}

impl ErrorType for Toggling {
    type Error = Infallible;
}

impl InputPin for Toggling {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        self.high = !self.high;
        Ok(self.high)
    }

    fn is_low(&mut self) -> Result<bool, Infallible> {
        self.is_high().map(|high| !high)
    }
}

/// Ein Ausgang, der seinen Pegel haelt.
#[derive(Debug, Default)]
pub struct Latch {
    /// Der zuletzt gesetzte Pegel.
    pub high: bool,
}

impl ErrorType for Latch {
    type Error = Infallible;
}

impl OutputPin for Latch {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.high = false;
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Infallible> {
        self.high = true;
        Ok(())
    }
}

/// `pins/button`.
pub type Button = Pin<Toggling>;

/// `pins/led`.
pub type Led = Pin<Latch>;

/// Das Geraet `pins`: Ein HAL-Pin hat keinen Heartbeat, er lebt (12.4).
#[derive(Debug, Default)]
pub struct Pins;

impl Device for Pins {
    fn alive(&mut self, _now: i64) -> bool {
        true
    }
}

/// Eine Schnittstelle, die je Tick eine Zeile `ping` bereithaelt: Nach dem
/// Zeilenende liegt bis zum naechsten Tick nichts mehr bereit.
#[derive(Debug, Default)]
pub struct Uart {
    /// Wie viele Bytes der laufenden Zeile schon gelesen sind.
    at: usize,
    /// Ob die Zeile dieses Ticks schon ganz gelesen ist.
    done: bool,
}

/// Die Zeile, die [`Uart`] je Tick bringt.
const LINE: &[u8] = b"ping\n";

impl embedded_io::ErrorType for Uart {
    type Error = Infallible;
}

impl embedded_io::ReadReady for Uart {
    fn read_ready(&mut self) -> Result<bool, Infallible> {
        let ready = !self.done;
        // Der naechste Tick fragt wieder; bis dahin war der Leser einmal leer.
        self.done = false;
        Ok(ready)
    }
}

impl embedded_io::Read for Uart {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
        let Some((byte, out)) = LINE.get(self.at).zip(buf.first_mut()) else { return Ok(0) };
        *out = *byte;
        self.at = (self.at + 1) % LINE.len();
        self.done = self.at == 0;
        Ok(1)
    }
}

/// `uart/rx`: Zeilen aus der Schnittstelle.
pub type UartRx = Framed<Serial<Uart>, Lines<16>>;

/// Das Geraet `uart`.
#[derive(Debug, Default)]
pub struct UartDevice;

impl Device for UartDevice {
    fn alive(&mut self, _now: i64) -> bool {
        true
    }
}

/// Der Ring, den die nachgebaute ISR fuellt.
static RX: Ring<64> = Ring::new();

/// Der Rahmen, den die ISR je Tick empfaengt: `[0x11, 0x22]` nach COBS.
const FRAME: [u8; 4] = [0x03, 0x11, 0x22, 0x00];

/// Die Quelle von `isr/rx`: der Ring. Ist er leer und kam in diesem Tick
/// noch nichts, empfaengt die nachgebaute ISR einen Rahmen, so wie es eine
/// echte zwischen zwei Ticks taete.
#[derive(Debug, Default)]
pub struct Isr {
    /// Ob die ISR in diesem Tick schon empfing.
    fed: bool,
}

impl Source for Isr {
    fn take(&mut self) -> Option<u8> {
        if RX.is_empty() && !core::mem::replace(&mut self.fed, true) {
            for byte in FRAME {
                RX.push(byte);
            }
        }
        let byte = RX.pop();
        if byte.is_none() {
            self.fed = false;
        }
        byte
    }
}

/// `isr/rx`: Rahmen aus dem Ring.
pub type IsrRx = Framed<Isr, Cobs<8>>;
