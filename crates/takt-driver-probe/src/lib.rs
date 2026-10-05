//! Das Pruefgeraet des Treiberrands als Treiber-Crate (12.6, 13.8; M10
//! Schritte 29c, 29d und 11).
//!
//! Ein Treiber-Crate stellt Geraete: je Adresse einen Typ, der die
//! Geraete-Traits aus `takt-embed` erfuellt. Welcher Typ welche Adresse
//! bedient, steht in `takt-drivers.toml`; ein Pruefstand verdrahtet danach
//! (`takt_frame::drivers::rust_rig`). Dieses liefert die Folge aus
//! `takt_board_support::edge_probe`: jeden Verstoss gegen den
//! Treibervertrag einmal, dazu einen unbestaetigten Schreibvorgang und
//! einen stillen Heartbeat, und die Kanaele, die `recorded.takt` nicht
//! liest (8.2). Die Programme dazu stehen in
//! `takt-conformance/tests/programs/driver_edge.takt` und `recorded.takt`.
//!
//! **Der Tick aus der Zeit.** Jede Methode bekommt die Tickgrenze `now`;
//! das Pruefgeraet rechnet daraus die Nummer des Ticks, nach der seine
//! Folge laeuft ([`edge_probe::TICK_NS`]).
//!
//! Beide Bring-ups und das Wirts-Bring-up binden es; auf dem Wirt faehrt
//! `takt driver-test --crate` es wie jedes andere Treiber-Crate.

#![no_std]

use takt_board_support::edge_probe;
use takt_embed::{Device, Input, Output, Piece, Quality, Sample, StreamInput, StreamOutput};

/// Die Nummer des Ticks, dessen Grenze `now` ist.
fn tick_of(now: i64) -> u64 {
    u64::try_from(now / edge_probe::TICK_NS).unwrap_or(0)
}

/// Die Qualitaet einer Zahl des Pruefgeraets (3.5).
fn quality(q: u8) -> Quality {
    match q {
        0 => Quality::Good,
        1 => Quality::Suspect,
        2 => Quality::Stale,
        _ => Quality::Bad,
    }
}

/// Eine Abtastung eines Skalars des Treiberrands.
fn reading(channel: edge_probe::Scalar, now: i64) -> Option<Sample<i64>> {
    let (v, at) = edge_probe::reading(channel, tick_of(now))?;
    Some(Sample::good(v, at.unwrap_or(now)))
}

/// `edge_a/p`.
#[derive(Debug, Default)]
pub struct EdgeAP;

impl Input<i64> for EdgeAP {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        reading(edge_probe::Scalar::P, now)
    }
}

/// `edge_a/q`.
#[derive(Debug, Default)]
pub struct EdgeAQ;

impl Input<i64> for EdgeAQ {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        reading(edge_probe::Scalar::Q, now)
    }
}

/// `edge_b/k`.
#[derive(Debug, Default)]
pub struct EdgeBK;

impl Input<i64> for EdgeBK {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        reading(edge_probe::Scalar::K, now)
    }
}

/// Ein Element des Pruefgeraets: Bytes, Zeitstempel und Folgenummer, wenn eigene.
type ProbeElement = (&'static [u8], Option<i64>, Option<i64>);

/// Wie oft ein Strom im laufenden Tick schon gefragt wurde.
#[derive(Debug, Default)]
struct Polls {
    tick: Option<u64>,
    polled: usize,
}

impl Polls {
    /// Das naechste Element einer Folge: `element(tick, i)` liefert Bytes,
    /// Zeitstempel und Folgenummer, ohne Angabe die des Rahmens.
    fn next(
        &mut self,
        element: impl Fn(u64, usize) -> Option<ProbeElement>,
        buf: &mut [u8],
        now: i64,
    ) -> Option<Piece> {
        let tick = tick_of(now);
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.polled = 0;
        }
        let i = self.polled;
        self.polled += 1;
        let (bytes, at, seq) = element(tick, i)?;
        let len = bytes.len().min(buf.len());
        buf[..len].copy_from_slice(&bytes[..len]);
        Some(Piece { len, t: at.unwrap_or(now), seq })
    }
}

/// Ein Strom des Treiberrands: Bytes und Folgenummer, der Zeitstempel ist
/// die Tickgrenze.
fn edge_element(stream: edge_probe::Stream) -> impl Fn(u64, usize) -> Option<ProbeElement> {
    move |tick, i| edge_probe::element(stream, tick, i).map(|(bytes, s)| (bytes, None, Some(s)))
}

/// `edge_u/rx`.
#[derive(Debug, Default)]
pub struct EdgeURx(Polls);

impl StreamInput for EdgeURx {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(edge_element(edge_probe::Stream::Lines), buf, now)
    }
}

/// `edge_c/rx`.
#[derive(Debug, Default)]
pub struct EdgeCRx(Polls);

impl StreamInput for EdgeCRx {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(edge_element(edge_probe::Stream::Pairs), buf, now)
    }
}

/// Ein Strom, den `recorded.takt` nicht liest (8.2).
fn unread(stream: edge_probe::Unread) -> impl Fn(u64, usize) -> Option<ProbeElement> {
    move |tick, i| edge_probe::unread(stream, tick, i)
}

/// `edge_r/frames`.
#[derive(Debug, Default)]
pub struct EdgeRFrames(Polls);

impl StreamInput for EdgeRFrames {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(unread(edge_probe::Unread::Frames), buf, now)
    }
}

/// `edge_r/text`.
#[derive(Debug, Default)]
pub struct EdgeRText(Polls);

impl StreamInput for EdgeRText {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(unread(edge_probe::Unread::Text), buf, now)
    }
}

/// `edge_r/raw`.
#[derive(Debug, Default)]
pub struct EdgeRRaw(Polls);

impl StreamInput for EdgeRRaw {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(unread(edge_probe::Unread::Raw), buf, now)
    }
}

/// Der Ausgang `edge_o/o`: bestaetigt, ausser wenn das Pruefgeraet es nicht tut.
#[derive(Debug, Default)]
pub struct EdgeOO;

impl Output<bool> for EdgeOO {
    fn write(&mut self, _value: bool, now: i64) -> bool {
        edge_probe::confirms(tick_of(now))
    }
}

/// Der Ausgabestrom `edge_t/tx`: meldet in einem Tick mehr freien Platz, als
/// der Strom fasst ([`edge_probe::TX_OVER_AT`], 12.6 Zeile 6).
#[derive(Debug, Default)]
pub struct EdgeTTx;

impl StreamOutput for EdgeTTx {
    fn free(&mut self, now: i64) -> Option<u32> {
        Some(edge_probe::tx_free(tick_of(now)))
    }

    /// `tx.idle` (8.8): fertig genau dann, wenn der Puffer leer ist
    /// (`free == capacity`), wie das Simulationsgeraet.
    fn idle(&mut self, now: i64) -> Option<bool> {
        Some(edge_probe::tx_free(tick_of(now)) == edge_probe::TX_CAPACITY)
    }
}

/// Der Ausgang `edge_f/o`: bestaetigt nie (Dauerversagen, 12.6 Zeile 6).
#[derive(Debug, Default)]
pub struct EdgeFO;

impl<T> Output<T> for EdgeFO {
    fn write(&mut self, _value: T, now: i64) -> bool {
        edge_probe::failing_confirms(tick_of(now))
    }
}

/// Das Geraet `edge_o` mit seinem Heartbeat.
#[derive(Debug, Default)]
pub struct EdgeO;

impl Device for EdgeO {
    fn alive(&mut self, now: i64) -> bool {
        edge_probe::alive(tick_of(now))
    }
}

/// `edge_r/level`, ein Kanal, den `recorded.takt` nicht liest (8.2).
#[derive(Debug, Default)]
pub struct EdgeRLevel;

impl Input<i32> for EdgeRLevel {
    fn sample(&mut self, now: i64) -> Option<Sample<i32>> {
        let (value, q) = edge_probe::level(tick_of(now))?;
        Some(Sample { value, quality: quality(q), t: now })
    }
}

/// `edge_r/temp`.
#[derive(Debug, Default)]
pub struct EdgeRTemp;

impl Input<f64> for EdgeRTemp {
    fn sample(&mut self, now: i64) -> Option<Sample<f64>> {
        let (value, at) = edge_probe::temp(tick_of(now));
        Some(Sample::good(value, at.unwrap_or(now)))
    }
}

/// `edge_r/on`.
#[derive(Debug, Default)]
pub struct EdgeROn;

impl Input<bool> for EdgeROn {
    fn sample(&mut self, now: i64) -> Option<Sample<bool>> {
        Some(Sample::good(edge_probe::on(tick_of(now)), now))
    }
}

/// `edge_r/mode`.
#[derive(Debug, Default)]
pub struct EdgeRMode;

impl Input<u32> for EdgeRMode {
    fn sample(&mut self, now: i64) -> Option<Sample<u32>> {
        Some(Sample::good(edge_probe::mode(tick_of(now)), now))
    }
}

// --- Ausdrueckliche Stummel (13.8, GEN-021) -----------------------------
//
// Ein Pruefstand ohne Geraet fuer eine Adresse baut nicht; wo das
// Pruefgeraet nichts zu pruefen hat, nennt die Verdrahtung einen dieser
// Typen, und der Stummel steht so in ihr statt still im Rahmen.

/// Ein Ausgang, der jeden Schreibvorgang bestaetigt: `o/p_ok` und die
/// anderen Anzeigen von `driver_edge.takt`, `o/beat` von `recorded.takt`.
#[derive(Debug, Default)]
pub struct Confirmed;

impl<T> Output<T> for Confirmed {
    fn write(&mut self, _value: T, _now: i64) -> bool {
        true
    }
}

/// Ein Geraet, dessen Heartbeat immer schlaegt: `o`.
#[derive(Debug, Default)]
pub struct Alive;

impl Device for Alive {
    fn alive(&mut self, _now: i64) -> bool {
        true
    }
}

/// Ein Eingang ohne Lieferung: `edge_r/quiet`, den `recorded.hw` ohne
/// Treiber nennt; er bleibt `Bad` (3.5).
#[derive(Debug, Default)]
pub struct Quiet;

impl<T> Input<T> for Quiet {
    fn sample(&mut self, _now: i64) -> Option<Sample<T>> {
        None
    }
}

// --- Das vertragstreue Pruefgeraet (GEN-046) ----------------------------
//
// Dieselben Traits, aber ohne Verstoss: Jede Abtastung kommt gut zur
// Tickgrenze, jedes Element mit steigender Nummer innerhalb von `MAXPT`,
// jeder Schreibvorgang wird bestaetigt, der Heartbeat schlaegt. `fair.takt`
// neben der Verdrahtung liest sie; `takt driver-test --crate` besteht
// damit. `fair_a/spike` liefert einmal einen Wert ausserhalb der Range und
// einmal einen Sprung ueber `max_slew`: Die Zeilen 3 und 4 urteilen ueber
// den Wert, nicht ueber den Treiber (13.8).

/// Der Tick, in dem `fair_a/spike` die Range verlaesst.
pub const OUT_OF_RANGE_AT: u64 = 3;

/// Der Tick, in dem `fair_a/spike` weiter springt als `max_slew` erlaubt.
pub const SLEW_AT: u64 = 6;

/// `fair_a/level`: 40 bis 43, je Tick um hoechstens drei.
#[derive(Debug, Default)]
pub struct FairLevel;

impl Input<i64> for FairLevel {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        let k = tick_of(now);
        Some(Sample::good(40 + i64::try_from(k % 4).unwrap_or(0), now))
    }
}

/// `fair_a/spike`: 20, ausser 150 in [`OUT_OF_RANGE_AT`] und 90 in [`SLEW_AT`].
#[derive(Debug, Default)]
pub struct FairSpike;

impl Input<i64> for FairSpike {
    fn sample(&mut self, now: i64) -> Option<Sample<i64>> {
        let value = match tick_of(now) {
            OUT_OF_RANGE_AT => 150,
            SLEW_AT => 90,
            _ => 20,
        };
        Some(Sample::good(value, now))
    }
}

/// `fair_u/rx`: in jedem geraden Tick eine Zeile, die Nummer vergibt der Rahmen.
#[derive(Debug, Default)]
pub struct FairRx(Polls);

impl StreamInput for FairRx {
    fn poll(&mut self, buf: &mut [u8], now: i64) -> Option<Piece> {
        self.0.next(|tick, i| (tick % 2 == 0 && i == 0).then_some((&b"fair"[..], None, None)), buf, now)
    }
}

/// `fair_o/o`: bestaetigt jeden Schreibvorgang.
#[derive(Debug, Default)]
pub struct FairO;

impl Output<bool> for FairO {
    fn write(&mut self, _value: bool, _now: i64) -> bool {
        true
    }
}

/// Das Geraet `fair_o`: sein Heartbeat schlaegt immer.
#[derive(Debug, Default)]
pub struct FairDevice;

impl Device for FairDevice {
    fn alive(&mut self, _now: i64) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = edge_probe::TICK_NS;

    /// Das vertragstreue Geraet liefert jeden Tick gut zur Tickgrenze; nur
    /// `spike` verlaesst die Range (Tick 3) und springt (Tick 6).
    #[test]
    fn the_fair_devices_keep_the_contract_and_spike_only_in_value() {
        for k in 0..10i64 {
            let level = FairLevel.sample(k * T).expect("jeder Tick");
            assert_eq!((level.quality, level.t), (Quality::Good, k * T));
            assert!((40..=43).contains(&level.value));
            let spike = FairSpike.sample(k * T).expect("jeder Tick");
            let want = match k {
                3 => 150,
                6 => 90,
                _ => 20,
            };
            assert_eq!((spike.value, spike.quality), (want, Quality::Good), "Tick {k}");
        }
        let mut rx = FairRx::default();
        let mut buf = [0u8; 17];
        let lines: usize = (0..10).map(|k| core::iter::from_fn(|| rx.poll(&mut buf, k * T)).count()).sum();
        assert_eq!(lines, 5, "je geradem Tick eine Zeile, nie zwei");
        assert!(FairO.write(true, 0) && FairDevice.alive(0));
    }

    /// `edge_t/tx` ueberfaehrt seinen Puffer genau einmal, `edge_f/o`
    /// bestaetigt nie.
    #[test]
    fn the_output_devices_break_the_contract_as_planned() {
        let t = edge_probe::TICK_NS;
        let over = Some(edge_probe::TX_CAPACITY + 1);
        let at: Option<i64> = (0..16).find(|k| EdgeTTx.free(k * t) == over);
        assert_eq!(at, Some(edge_probe::TX_OVER_AT as i64));
        assert_eq!((0..16).filter(|k| EdgeTTx.free(k * t) == over).count(), 1);
        assert!((0..16).all(|k| !Output::<bool>::write(&mut EdgeFO, true, k * t)));
    }

    /// FB-124: `edge_t/tx` beantwortet `tx.idle`, fertig genau bei vollem
    /// freiem Platz; in dem Tick, in dem es den Puffer ueberfaehrt, nicht.
    #[test]
    fn the_stream_device_answers_idle() {
        let t = edge_probe::TICK_NS;
        let at = edge_probe::TX_OVER_AT as i64;
        assert!((0..16).filter(|k| *k != at).all(|k| EdgeTTx.idle(k * t) == Some(true)));
        assert_eq!(EdgeTTx.idle(at * t), Some(false));
    }

    /// Die ausdruecklichen Stummel: bestaetigt, lebendig, ohne Lieferung.
    #[test]
    fn the_explicit_stubs_answer_as_their_names_say() {
        assert!(Output::<bool>::write(&mut Confirmed, true, 0) && Output::<i64>::write(&mut Confirmed, 7, 0));
        assert!(Alive.alive(0));
        assert!(Input::<u8>::sample(&mut Quiet, 0).is_none());
    }
}
