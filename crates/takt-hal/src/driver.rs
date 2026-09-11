//! Was ein Treiber koennen muss.
//!
//! Prinzip 4 des Plans: „Treiber als Traits, deren
//! Simulationsimplementierung der Sim-Backend ist (die Sim/HW-Umschaltung
//! ist ein Treiberwechsel, kein zweiter Programmpfad)." Der
//! Simulationstreiber in `sim` ist darum die erste Implementierung dieser
//! Traits, kein Testdouble.
//!
//! **Ein Treiber liefert und stellt, er prueft nicht.** Alles, was 12.6
//! verlangt, macht `edge` mit dem, was hier herauskommt — ein Treiber, der
//! selbst pruefte, koennte die Pruefung auch weglassen.

use takt_mir::ChannelId;

use crate::quality::Quality;

/// Eine Skalar-Lieferung: Wert, Qualitaet des Treibers, Zeitstempel.
///
/// Die Qualitaet hier ist die *des Treibers* („mein Sensor meldet einen
/// Fehler"), nicht die des Randes. Was daraus wird, entscheidet `edge`:
/// Ein Treiber kann eine Lieferung abwerten, aber keine aufwerten.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading<V> {
    /// Der Channel.
    pub channel: ChannelId,
    /// Der Wert; `None`, wenn der Treiber keinen hat.
    pub value: Option<V>,
    /// Was der Treiber ueber ihn sagt.
    pub quality: Quality,
    /// Hardware-Zeitstempel in Nanosekunden.
    pub t: i64,
}

/// Ein Stromelement (8.6): Zeitstempel, Folgenummer, Nutzlast.
#[derive(Clone, Debug, PartialEq)]
pub struct Element<V> {
    /// Der Channel.
    pub channel: ChannelId,
    /// Hardware-Zeitstempel in Nanosekunden.
    pub t: i64,
    /// Folgenummer; streng steigend und lueckenlos (12.6, Zeile 2).
    pub seq: i64,
    /// Die Nutzlast.
    pub value: V,
}

/// Was der Treiber ueber einen Schreibvorgang zurueckmeldet (12.6, Zeile 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// Geschrieben und bestaetigt.
    Acked,
    /// Nicht bestaetigt — der Besitzer des Outputs bekommt `Runtime(Driver)`.
    Unconfirmed,
}

/// Ein Schreibvorgang an den Treiber.
#[derive(Clone, Debug, PartialEq)]
pub struct Writing<V> {
    /// Der Channel.
    pub channel: ChannelId,
    /// Der zu stellende Wert.
    pub value: V,
    /// Hardware-Zeitpunkt in Nanosekunden, zu dem gestellt werden soll
    /// (9.8: `at`/`pulse`); `None` heisst sofort.
    pub at: Option<i64>,
}

/// Ein Treiber.
///
/// `V` ist die Wertdarstellung des Aufrufers: `Value` im Interpreter,
/// getypte Strukturen im erzeugten Code (11.2). Der Rand kommt fuer beide
/// aus demselben Code, und genau darauf beruht Satz 9.4.4 fuer die
/// Randfaelle.
pub trait Driver<V> {
    /// Name des Treibers; steht in Meldungen und im Lauf-Header.
    fn name(&self) -> &str;

    /// Holt die Skalar-Lieferungen dieses Ticks aus den Treiberpuffern
    /// (12.1: `sample_inputs()`).
    fn read(&mut self, now: i64, out: &mut Vec<Reading<V>>);

    /// Holt die Stromelemente dieses Ticks (8.6). `|D| <= MAXPT` prueft
    /// `edge`, nicht der Treiber.
    fn poll(&mut self, now: i64, out: &mut Vec<Element<V>>) {
        let _ = (now, out);
    }

    /// Stellt einen Wert. Die Rueckmeldung geht in Pruefung 6.
    fn write(&mut self, w: &Writing<V>) -> Delivery;

    /// Freier Platz im Sendepuffer eines Ausgabestroms in Bytes
    /// (`free[o]`, 8.8).
    fn free(&self, channel: ChannelId) -> Option<u32> {
        let _ = channel;
        None
    }

    /// Ist der Heartbeat zum Geraet intakt (12.6, Zeile 6)?
    fn alive(&self) -> bool {
        true
    }
}
