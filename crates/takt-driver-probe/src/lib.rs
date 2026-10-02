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
use takt_embed::{Device, Input, Output, Piece, Quality, Sample, StreamInput};

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
