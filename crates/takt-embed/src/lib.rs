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
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "Stufe 2 des Regelwerks, mit `unsafe` (plan/cert.md Abschnitt 6)"
    )
)]

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
    /// Ob das Programm Jobs hat (4.5, FB-385): [`NoJobs`] oder [`WithJobs`].
    /// Einen Job-Kontext bekommt es ueber [`Work::context`].
    type Work: Work;
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

/// Ob ein Programm Jobs hat (4.5, 12.11, FB-385); die Huelle nennt es
/// ([`Program::Work`]). Ein Programm mit Jobs bekommt seinen Job-Kontext,
/// eines ohne keinen: Wer fuer jedes Programm baut, waehlt hier, statt eine
/// Konstante zu pruefen, und einen Job-Kontext, der still fehlt, gibt es
/// nicht.
pub trait Work {
    /// Der Job-Kontext eines solchen Programms ueber dem Kontext `C` des Wirts.
    type Context<C: Context>: Context;

    /// Der Job-Kontext: `start` baut ihn, und nur fuer ein Programm mit Jobs
    /// wird es gerufen.
    fn context<C: Context>(start: impl FnOnce() -> C) -> Self::Context<C>;
}

/// Ein Programm ohne Jobs.
#[derive(Debug)]
pub enum NoJobs {}

/// Ein Programm mit Jobs (4.5).
#[derive(Debug)]
pub enum WithJobs {}

impl Work for NoJobs {
    type Context<C: Context> = Idle;

    fn context<C: Context>(_: impl FnOnce() -> C) -> Idle {
        Idle
    }
}

impl Work for WithJobs {
    type Context<C: Context> = C;

    fn context<C: Context>(start: impl FnOnce() -> C) -> C {
        start()
    }
}

/// Ein Job-Kontext, wie ihn der Wirt zwischen den Ticks treibt (4.5).
pub trait Context {
    /// Rechnet, was ansteht, bis der Job abgibt oder der naechste Tick ihn
    /// unterbricht.
    fn run(&mut self);

    /// Rechnet jeden Job zu Ende: in logischer Zeit, wo der Wirt warten
    /// darf (13.8).
    fn finish(&mut self);
}

/// Kein Job-Kontext: der eines Programms ohne Jobs.
#[derive(Debug)]
pub struct Idle;

impl Context for Idle {
    fn run(&mut self) {}

    fn finish(&mut self) {}
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

/// Ein Eingang zusammengesetzten Typs (Record, Feld, `samples`, 8.9): Der
/// Wert geht als Bytes in seiner Speicherform nach `value`, wie der
/// erzeugte Code ihn im Prozessabbild liest; die Abtastung nennt Qualitaet
/// und Zeitstempel.
pub trait BlockInput {
    /// Schreibt die Abtastung dieses Ticks nach `value`; `None`, wenn keine
    /// vorliegt.
    fn sample(&mut self, value: &mut [u8], now: i64) -> Option<Sample<()>>;
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

#[cfg(test)]
mod tests {
    use super::{Context, NoJobs, WithJobs, Work};

    /// Ein Job-Kontext, der zaehlt, wie oft der Wirt ihn treibt.
    struct Counted(u32);

    impl Context for Counted {
        fn run(&mut self) {
            self.0 += 1;
        }

        fn finish(&mut self) {
            self.0 += 10;
        }
    }

    /// **Nur ein Programm mit Jobs bekommt einen Job-Kontext** (4.5,
    /// FB-385): Fuer eines ohne wird er nicht gebaut, und was der Wirt
    /// zwischen den Ticks treibt, tut nichts; fuer eines mit ist er der des
    /// Wirts.
    #[test]
    fn only_a_program_with_jobs_gets_a_job_context() {
        let mut built = false;
        let mut idle = <NoJobs as Work>::context(|| {
            built = true;
            Counted(0)
        });
        idle.run();
        idle.finish();
        assert!(!built, "ohne Jobs kein Job-Kontext");
        let mut context = <WithJobs as Work>::context(|| Counted(0));
        context.run();
        context.finish();
        assert_eq!(context.0, 11);
    }
}
