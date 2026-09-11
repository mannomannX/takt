//! Der Simulationstreiber.
//!
//! Prinzip 4 des Plans: Die Sim/HW-Umschaltung ist ein Treiberwechsel, kein
//! zweiter Programmpfad. Dieser Treiber ist darum die erste Implementierung
//! von [`Driver`] und **kein Testdouble** — `takt
//! sim` laeuft durch denselben Rand wie `takt run`, nur mit einer anderen
//! Quelle. Waere er ein Sonderweg, pruefte die Simulation die Randfaelle
//! aus 12.6 nie, und Satz 9.4.4 gaelte fuer sie nicht.
//!
//! Die Quelle ist der Stimulus: gesetzte Werte je Tick. Was der Stimulus
//! nicht setzt, liefert der Treiber nicht — das Fortschreiben des letzten
//! Werts ist Sache des Prozessabbilds (8.3), nicht des Treibers.

use takt_mir::ChannelId;

use crate::driver::{Delivery, Driver, Element, Reading, Writing};
use crate::edge::{Capacity, Heartbeat};
use crate::quality::Quality;

/// Ein Treiber, der liefert, was ihm vorgelegt wurde.
#[derive(Debug, Default)]
pub struct Sim<V> {
    /// Was im naechsten `read` herauskommt.
    pending: Vec<Reading<V>>,
    /// Was im naechsten `poll` herauskommt.
    elements: Vec<Element<V>>,
    /// Was geschrieben wurde — die Simulation bestaetigt jeden Vorgang.
    pub written: Vec<Writing<V>>,
    /// Freier Platz je Ausgabestrom in Bytes (8.8).
    free: Vec<(ChannelId, u32)>,
    /// Heartbeat; in der Simulation intakt, bis ein Test ihn nimmt.
    alive: bool,
}

impl<V> Sim<V> {
    /// Ein Treiber ohne Lieferungen.
    pub fn new() -> Sim<V> {
        Sim { pending: Vec::new(), elements: Vec::new(), written: Vec::new(), free: Vec::new(), alive: true }
    }

    /// Legt eine Abtastung fuer den naechsten Tick vor (Stimulus).
    pub fn feed(&mut self, channel: ChannelId, value: V, t: i64) {
        self.pending.push(Reading { channel, value: Some(value), quality: Quality::Good, t });
    }

    /// Legt eine Abtastung mit eigener Qualitaet vor — so meldet ein
    /// Treiber einen Sensorfehler (12.6: abwerten darf er, aufwerten nicht).
    pub fn feed_as(&mut self, channel: ChannelId, value: Option<V>, quality: Quality, t: i64) {
        self.pending.push(Reading { channel, value, quality, t });
    }

    /// Legt ein Stromelement vor (8.6).
    pub fn feed_element(&mut self, channel: ChannelId, t: i64, seq: i64, value: V) {
        self.elements.push(Element { channel, t, seq, value });
    }

    /// Setzt den freien Platz eines Ausgabestroms (8.8).
    pub fn set_free(&mut self, channel: ChannelId, free: u32) {
        match self.free.iter_mut().find(|(c, _)| *c == channel) {
            Some(slot) => slot.1 = free,
            None => self.free.push((channel, free)),
        }
    }

    /// Laesst den Heartbeat aussetzen (12.6, Zeile 6).
    pub fn set_alive(&mut self, alive: bool) {
        self.alive = alive;
    }
}

impl<V: Clone> Driver<V> for Sim<V> {
    fn name(&self) -> &str {
        "sim"
    }

    fn read(&mut self, now: i64, out: &mut Vec<Reading<V>>) {
        let _ = now;
        out.append(&mut self.pending);
    }

    fn poll(&mut self, now: i64, out: &mut Vec<Element<V>>) {
        let _ = now;
        out.append(&mut self.elements);
    }

    fn write(&mut self, w: &Writing<V>) -> Delivery {
        self.written.push(w.clone());
        Delivery::Acked
    }

    fn free(&self, channel: ChannelId) -> Option<u32> {
        self.free.iter().find(|(c, _)| *c == channel).map(|(_, f)| *f)
    }

    fn alive(&self) -> bool {
        self.alive
    }
}

impl<V> Heartbeat for Sim<V> {
    fn alive(&self) -> bool {
        self.alive
    }
}

impl<V> Capacity for Sim<V> {
    fn free(&self, channel: ChannelId) -> Option<u32> {
        self.free.iter().find(|(c, _)| *c == channel).map(|(_, f)| *f)
    }
}
