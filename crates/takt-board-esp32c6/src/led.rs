//! Die RGB-LED des DevKitM-1: ein WS2812 an IO8 (12.3).
//!
//! Kein Pin, den man setzt, sondern ein serielles Protokoll: 24 Bit in
//! GRB-Reihenfolge, jedes Bit ein Puls von 1,25 µs, dessen High-Anteil das
//! Bit traegt (0: 0,4 µs, 1: 0,8 µs), danach mindestens 50 µs Ruhe. Der
//! RMT-Baustein sendet die Pulse ohne Zutun des Kerns; die Schleife
//! wartet nur die 30 µs, bis das Bild draussen ist.

use esp_hal::Blocking;
use esp_hal::gpio::Level;
use esp_hal::gpio::interconnect::PeripheralOutput;
use esp_hal::peripherals::RMT;
use esp_hal::rmt::{Channel, ConfigError, PulseCode, Rmt, Tx, TxChannelConfig, TxChannelCreator};
use esp_hal::time::Rate;

/// RMT-Takt: 80 MHz, ein Schritt 12,5 ns.
const RMT_MHZ: u32 = 80;
/// High- und Low-Zeit eines 0-Bits in Schritten (0,4 µs, 0,85 µs).
const ZERO: (u16, u16) = (32, 68);
/// High- und Low-Zeit eines 1-Bits in Schritten (0,8 µs, 0,45 µs).
const ONE: (u16, u16) = (64, 36);
/// Ruhe nach dem Bild: 2 × 25 µs.
const RESET: u16 = 2_000;

/// Die LED hinter einem RMT-Sendekanal.
pub struct Ws2812 {
    channel: Option<Channel<'static, Blocking, Tx>>,
    /// Die zuletzt gesendete Farbe: Ein Bild kostet 80 µs Warten.
    last: Option<[u8; 3]>,
}

impl Ws2812 {
    /// Bindet den RMT-Kanal 0 an den Daten-Pin der LED.
    ///
    /// # Errors
    /// Wenn der RMT-Takt oder die Kanalkonfiguration abgelehnt wird.
    pub fn new(rmt: RMT<'static>, pin: impl PeripheralOutput<'static>) -> Result<Ws2812, ConfigError> {
        let rmt = Rmt::new(rmt, Rate::from_mhz(RMT_MHZ))?;
        let config = TxChannelConfig::default().with_clk_divider(1).with_idle_output_level(Level::Low);
        let channel = rmt.channel0.configure_tx(&config)?.with_pin(pin);
        Ok(Ws2812 { last: None, channel: Some(channel) })
    }

    /// Setzt die Farbe; `[0, 0, 0]` ist aus.
    pub fn set(&mut self, rgb: [u8; 3]) {
        if self.last == Some(rgb) {
            return;
        }
        self.last = Some(rgb);
        let Some(channel) = self.channel.take() else { return };
        let frame = frame(rgb);
        self.channel = Some(match channel.transmit(&frame) {
            Ok(tx) => match tx.wait() {
                Ok(channel) | Err((_, channel)) => channel,
            },
            Err((_, channel)) => channel,
        });
    }

    /// Ein gedaempftes Gruen: sichtbar, ohne zu blenden.
    pub fn on(&mut self) {
        self.set([0, 16, 0]);
    }

    /// Aus.
    pub fn off(&mut self) {
        self.set([0, 0, 0]);
    }
}

/// Das Bild einer LED: 24 Bit GRB, hoechstwertiges Bit zuerst, Ruhe, Ende.
fn frame([r, g, b]: [u8; 3]) -> [PulseCode; 26] {
    let mut out = [PulseCode::end_marker(); 26];
    let grb = (u32::from(g) << 16) | (u32::from(r) << 8) | u32::from(b);
    for (i, code) in out.iter_mut().take(24).enumerate() {
        let (high, low) = if grb & (1 << (23 - i)) != 0 { ONE } else { ZERO };
        *code = PulseCode::new(Level::High, high, Level::Low, low);
    }
    out[24] = PulseCode::new(Level::Low, RESET, Level::Low, RESET);
    out
}
