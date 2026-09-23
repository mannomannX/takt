//! Signale der Peripherie auf Pins legen (12.3, 12.10).
//!
//! **Warum das hier steht.** Ein `port ... @ mmio(...)` (12.10) schreibt
//! Register, und ein UART0 mit gefuelltem Sende-FIFO ist noch lange nicht
//! mit der Aussenwelt verbunden: Auf dem C6 liegt zwischen Peripherie und
//! Pad die GPIO-Matrix, und ohne Eintrag dort bleibt jedes Byte im Chip.
//! Gemessen mit `loopback_probe.takt`: ueber den Draht kam nichts zurueck,
//! mit `CONF0.loopback` sofort — der Empfaenger arbeitete, es fehlte der
//! Weg nach draussen.
//!
//! **Ohne den HAL-Treiber.** `esp_hal::uart::Uart` wuerde CONF0, Teiler
//! und Baudrate selbst setzen und dem Takt-Treiber ins Register greifen;
//! sein `PinGuard` nimmt die Zuordnung beim Fallenlassen ausserdem wieder
//! zurueck. Hier werden nur die drei Felder der Matrix geschrieben. Die
//! Signalwerte selbst tragen keinen `Drop`, also bleibt die Zuordnung.

use esp_hal::gpio::interconnect::{InputSignal, OutputSignal};
use esp_hal::gpio::{InputConfig, OutputConfig, Pull};
use esp_hal::peripherals::{GPIO7, GPIO17};

/// Legt UART0 auf seine Pins: TX auf GPIO7, RX auf GPIO17.
///
/// GPIO7 und nicht GPIO16: Auf diesem Board fuehrt die mit `TX`
/// beschriftete Buchse nicht an GPIO16. Gemessen mit `wire_probe.takt`,
/// das einen Pin treibt und zaehlt, welcher folgt — GPIO7 gegen GPIO17
/// traf 81 von 81, jede andere Paarung die Haelfte.
///
/// Die Pads sind nach der Wandlung abgeschaltet (`init_gpio` loescht
/// `fun_ie` und die Ausgabefreigabe), darum die beiden Freigaben. Der
/// Pull-up haelt RX in der Ruhelage hoch, solange kein Draht steckt —
/// ein offener Eingang erzeugt sonst Rahmenfehler aus dem Nichts.
pub fn route_uart0(tx_pin: GPIO7<'static>, rx_pin: GPIO17<'static>) {
    let tx = OutputSignal::from(tx_pin);
    tx.set_output_high(true);
    tx.apply_output_config(&OutputConfig::default());
    tx.set_output_enable(true);
    esp_hal::gpio::OutputSignal::U0TXD.connect_to(&tx);

    let rx = InputSignal::from(rx_pin);
    rx.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
    rx.set_input_enable(true);
    esp_hal::gpio::InputSignal::U0RXD.connect_to(&rx);
}

