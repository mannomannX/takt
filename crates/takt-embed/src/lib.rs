//! Takt als Baustein in Rust (12.11): die Typen und Geraete-Traits der
//! Treiber (12.6).
//!
//! **Zwei Ebenen.** Ein Geraet — ein Pin, ein ADC-Kanal, eine serielle
//! Schnittstelle — erfuellt die Traits dieses Crates: [`Input`],
//! [`StreamInput`], [`Output`], [`StreamOutput`], [`Device`]. Sie kennen
//! kein Programm; ein Treiber-Crate oder ein Adapter schreibt sie einmal.
//! Jedes Programm bekommt dazu einen erzeugten Trait `Drivers` mit einer
//! Methode je gebundener Adresse; der Wirt erfuellt ihn fuer sein
//! Treiberobjekt und verdrahtet je Kanal ein Geraet. Fehlt eine Methode,
//! uebersetzt die Firmware nicht, und die Meldung nennt den Kanal.
//!
//! **Die Zeit des Ticks.** Jede Methode bekommt `now`, die Tickgrenze in
//! Nanosekunden seit dem Start (3.3). Ein Eingang stempelt damit, was er
//! nicht genauer weiss (12.6 Zeile 1); ein Geraet, das nach Zeit urteilt,
//! braucht keine Uhr des Rahmens.

#![no_std]

/// Die Qualitaet einer Lieferung (3.5), in der Zahl des Prozessabbilds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Quality {
    /// Der Wert gilt.
    Good = 0,
    /// Der Wert gilt mit Vorbehalt.
    Suspect = 1,
    /// Der Wert ist alt.
    Stale = 2,
    /// Es gibt keinen Wert; der Grund ist der Treiber (12.6 Zeile 2).
    Bad = 3,
}

/// Eine Abtastung eines Eingangs (12.6): Wert, Qualitaet und Zeitstempel in
/// Nanosekunden seit dem Start. Mit [`Quality::Bad`] traegt sie keinen Wert;
/// `value` zaehlt dann nicht.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample<T> {
    /// Der Wert.
    pub value: T,
    /// Die Qualitaet.
    pub quality: Quality,
    /// Der Zeitpunkt der Messung.
    pub t: i64,
}

impl<T> Sample<T> {
    /// Ein guter Wert, gemessen zu `t`.
    pub fn good(value: T, t: i64) -> Sample<T> {
        Sample { value, quality: Quality::Good, t }
    }
}

/// Ein Element eines Eingabestroms, das der Treiber in den Puffer des
/// Rahmens geschrieben hat: wie viele Bytes, wann, mit welcher Folgenummer.
/// Ohne Folgenummer gilt die, die der Rahmen erwartet (12.6 Zeile 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    /// Die Bytes im Puffer; mehr als seine Laenge zaehlen nicht.
    pub len: usize,
    /// Der Zeitstempel.
    pub t: i64,
    /// Die Folgenummer, wenn der Treiber eine eigene fuehrt.
    pub seq: Option<i64>,
}

/// Ein skalarer Eingang.
pub trait Input<T> {
    /// Die Abtastung dieses Ticks; `None`, wenn keine vorliegt.
    fn sample(&mut self, now: i64) -> Option<Sample<T>>;
}

/// Ein Eingabestrom: je Aufruf hoechstens ein Element.
pub trait StreamInput {
    /// Schreibt das naechste Element nach `buf`; `None`, wenn keines wartet.
    /// Ein laengeres als `buf` kuerzt der Treiber; der Rand verwirft es
    /// (12.6 Zeile 5).
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece>;
}

/// Ein skalarer Ausgang.
pub trait Output<T> {
    /// Stellt `value`; `true`, wenn das Geraet den Schreibvorgang bestaetigt
    /// (12.6 Zeile 6).
    fn write(&mut self, value: T, now: i64) -> bool;
}

/// Ein Ausgabestrom, soweit der Rand ihn prueft (8.8, 12.6 Zeile 6).
pub trait StreamOutput {
    /// Der freie Platz im Sendepuffer des Geraets; `None`, wenn es ihn nicht kennt.
    fn free(&mut self, now: i64) -> Option<u32>;
}

/// Ein Geraet mit Heartbeat (12.4).
pub trait Device {
    /// Lebt das Geraet?
    fn alive(&mut self, now: i64) -> bool;
}
