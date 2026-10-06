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
//!
//! **Die Lieferform** (12.11). `takt build --emit embed` schreibt je
//! Programm eine Bibliothek und das Modul `P.rs`; der Bauhelfer
//! ([`build::Program`], Merkmal `build`) ruft es aus `build.rs`. Die Huelle
//! im Modul erfuellt [`rt::Program`] und laeuft unter dem Kern ohne Warten
//! ([`rt::Runtime::service`]). Die Testhilfe ([`testing`], Merkmal
//! `testing`) faehrt das Programm in logischer Zeit und vergleicht seinen
//! Trace mit dem Interpreter (13.1).

#![no_std]

#[cfg(any(feature = "build", feature = "testing"))]
extern crate std;

#[cfg(feature = "build")]
pub mod build;
#[cfg(feature = "testing")]
pub mod testing;

/// Der Kern ohne Warten (12.11): `Runtime::service`, `Program`, die Senke.
pub use takt_rt_core as rt;

// Was der erzeugte Code ruft — Natives, korrekt gerundete Mathematik, der
// Rand (4.5, 12.6) —, bindet jeder Wirt mit diesem Crate.
use takt_native_abi as _;

/// Ein Programm der Lieferform: die erzeugte Huelle `Program` in `P.rs`.
/// Ein Port treibt es mit [`rt::Runtime::service`] und rechnet im
/// Job-Kontext, was `service` als `jobs` meldet.
pub trait Program: rt::Program {
    /// Der Griff des Job-Kontexts.
    type Jobs: Jobs;
    /// Der Griff, der zwischen den Ticks verteilt.
    type Dispatch: Dispatch;

    /// Der Griff fuer den Job-Kontext (4.5), einmal je Programm: `None`, wenn
    /// ihn schon jemand hat.
    fn jobs(&mut self) -> Option<Self::Jobs>;

    /// Der Griff, der dem Job-Kontext zwischen den Ticks den naechsten
    /// Auftrag gibt (4.5), fuer einen Port, der nicht ueber `service`
    /// verteilt; einmal je Programm.
    fn dispatch(&mut self) -> Option<Self::Dispatch>;
}

/// Der Job-Kontext eines Programms (4.5). Es gibt ihn einmal; er lebt nicht
/// laenger als die Arena des Programms.
pub trait Jobs: Send {
    /// Rechnet den Auftrag, den `service` gegeben hat; `service` darf ihn
    /// unterbrechen.
    fn work(&mut self);
}

/// Das Verteilen der Jobs fuer einen Port, der zwischen den Ticks selbst
/// verteilt (4.5): im Kontext des Schritts, solange der Job-Kontext ruht.
pub trait Dispatch {
    /// Gibt dem Job-Kontext den aeltesten wartenden Job; wahr, wenn er zu
    /// rechnen hat.
    fn next(&mut self) -> bool;
}

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
    /// Ein laengeres als `buf` kuerzt der Treiber. Text und Bytes kuerzt der
    /// Rand auf ihr `N` (3.9); ein Record-Element, das nicht seine Laenge
    /// hat, verwirft er als `malformed` (12.6 Zeile 5).
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

    /// Puffer leer und Sender fertig, das letzte Bit draussen (`tx.idle`,
    /// 8.8); `None`, wenn das Geraet es nicht beantworten kann.
    fn idle(&mut self, now: i64) -> Option<bool> {
        let _ = now;
        None
    }
}

/// Ein Geraet mit Heartbeat (12.4).
pub trait Device {
    /// Lebt das Geraet?
    fn alive(&mut self, now: i64) -> bool;
}
