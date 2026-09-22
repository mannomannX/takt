//! Ein Taster als Eingang des Prozessabbilds (12.1 Schritt 2, 12.6).
//!
//! Der BOOT-Taster des DevKitM-1 liegt an IO9 gegen Masse: gedrueckt ist
//! `Low`. Nach aussen meldet der Treiber `true` fuer gedrueckt, damit das
//! Programm die Polaritaet der Platine nicht kennen muss — sie steht in
//! der Hardware-Konfiguration (8.10, `port = "GPIO9 active_low"`).
//!
//! **Warum entprellt wird.** Ein mechanischer Kontakt schliesst nicht
//! einmal, sondern zehn- bis hundertmal in wenigen Millisekunden. Ohne
//! Entprellung saehe ein Programm mit 1 ms Tick daraus mehrere
//! Betaetigungen. Der Treiber meldet darum erst, was ueber `STABLE_TICKS`
//! Abfragen gleich geblieben ist; solange der Pegel wackelt, bleibt der
//! letzte stabile Wert stehen und die Qualitaet ist `Suspect` (12.6) —
//! ein Wert, den das Programm benutzen darf, aber nicht muss.

use esp_hal::gpio::{Input, InputConfig, Level, Pull};

/// So viele gleiche Abfragen machen einen Pegel stabil.
const STABLE_TICKS: u8 = 3;

/// Ein entprellter Eingang an einem GPIO.
pub struct Button {
    pin: Input<'static>,
    stable: bool,
    candidate: bool,
    seen: u8,
}

impl Button {
    /// Bindet den Pin mit Pull-up; ein offener Eingang liest `High`.
    pub fn new(pin: impl esp_hal::gpio::InputPin + 'static) -> Button {
        let pin = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
        let level = pin.level() == Level::Low;
        Button { pin, stable: level, candidate: level, seen: STABLE_TICKS }
    }

    /// Fragt den Pin ab und fuehrt die Entprellung nach.
    ///
    /// Liefert den stabilen Pegel und ob er es ist: Ein wackelnder
    /// Kontakt gibt `false` zurueck, und der Aufrufer macht daraus
    /// `Suspect`.
    pub fn poll(&mut self) -> (bool, bool) {
        let now = self.pin.level() == Level::Low;
        if now == self.candidate {
            self.seen = self.seen.saturating_add(1);
        } else {
            self.candidate = now;
            self.seen = 1;
        }
        if self.seen >= STABLE_TICKS {
            self.stable = self.candidate;
        }
        (self.stable, self.seen >= STABLE_TICKS)
    }
}
