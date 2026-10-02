//! Das Pruefgeraet des Treiberrands (12.6, M10 Schritt 29c): ein Treiber
//! auf dem Board, der jeden Verstoss gegen den Treibervertrag einmal
//! liefert, dazu einen unbestaetigten Schreibvorgang und einen stillen
//! Heartbeat.
//!
//! Die Folge steht hier, damit Board und Wirt dieselbe haben: Das Board
//! liefert sie ueber die Geraete von `takt-driver-probe`; der Wirt macht
//! daraus den Stimulus, mit dem der Interpreter denselben Lauf rechnet. Das
//! Programm dazu steht in `takt-conformance/tests/programs/driver_edge.takt`.

/// Der Tick der Pruefprogramme `driver_edge.takt` und `recorded.takt`: Die
/// Folge laeuft nach der Nummer des Ticks.
pub const TICK_NS: i64 = 10_000_000;

/// Ein Skalar des Pruefgeraets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    /// `edge_a/p`
    P,
    /// `edge_a/q`
    Q,
    /// `edge_b/k`
    K,
}

impl Scalar {
    /// Alle, in der Reihenfolge des Programms.
    pub const ALL: [Scalar; 3] = [Scalar::P, Scalar::Q, Scalar::K];

    /// Der Name des Kanals im Programm.
    pub fn name(self) -> &'static str {
        match self {
            Scalar::P => "p",
            Scalar::Q => "q",
            Scalar::K => "k",
        }
    }
}

/// Die Lieferung eines Skalars in Tick `tick`: der Wert und der
/// Zeitstempel in Nanosekunden, ohne Angabe die Tickgrenze.
pub fn reading(channel: Scalar, tick: u64) -> Option<(i64, Option<i64>)> {
    use Scalar::{K, P, Q};
    Some(match (channel, tick) {
        (P, 0) => (5, None),
        (Q, 0) => (6, None),
        (K, 0) => (7, None),
        // Zeile 1: 5 ms hinter dem Fenster, in der Toleranz eines Ticks.
        (P, 1) => (6, Some(15_000_000)),
        // Zeile 1, zweiter Fall: jenseits der Toleranz, ganz `edge_a`.
        (P, 2) => (7, Some(-50_000_000)),
        (Q, 3) => (8, None),
        (P, 4) => (9, None),
        // Zeile 2: fallender Zeitstempel.
        (K, 10) => (1, Some(95_000_000)),
        (K, 11) => (2, Some(94_000_000)),
        (K, 12) => (3, None),
        _ => return None,
    })
}

/// Ein Strom des Pruefgeraets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    /// `edge_u/rx`, Zeilen.
    Lines,
    /// `edge_c/rx`, Records `Pair` in kanonischer Form.
    Pairs,
}

impl Stream {
    /// Beide, in der Reihenfolge des Programms.
    pub const ALL: [Stream; 2] = [Stream::Lines, Stream::Pairs];

    /// Der Name des Kanals im Programm.
    pub fn name(self) -> &'static str {
        match self {
            Stream::Lines => "rx",
            Stream::Pairs => "pairs",
        }
    }
}

/// Das `i`-te Element eines Stroms in Tick `tick`: Bytes und Folgenummer;
/// der Zeitstempel ist die Tickgrenze.
pub fn element(stream: Stream, tick: u64, i: usize) -> Option<(&'static [u8], i64)> {
    use Stream::{Lines, Pairs};
    let all: &[(&[u8], i64)] = match (stream, tick) {
        (Lines, 0) => &[(b"a", 0)],
        // Zeile 2: Luecke in `seq`, dann lueckenlos weiter.
        (Lines, 5) => &[(b"x", 10)],
        (Lines, 6) => &[(b"y", 11)],
        // Zeile 2: drei Elemente bei `MAXPT = 200 Hz * 10 ms = 2`.
        (Lines, 7) => &[(b"1", 12), (b"2", 13), (b"3", 14)],
        (Lines, 8) => &[(b"z", 15)],
        (Pairs, 0) => &[(&[1, 2], 0)],
        // Zeile 5: ein Byte zu viel fuer `Pair`, dann ein gutes.
        (Pairs, 9) => &[(&[1, 2, 3], 1), (&[3, 4], 2)],
        _ => &[],
    };
    all.get(i).copied()
}

/// Bestaetigt der Ausgang `edge_o/o` den Schreibvorgang des Commits in
/// Tick `tick` (Zeile 6)?
pub fn confirms(tick: u64) -> bool {
    tick != 3
}

/// Lebt das Geraet `edge_o` beim Commit in Tick `tick` (Heartbeat, Zeile 6)?
pub fn alive(tick: u64) -> bool {
    tick != 8
}

/// Die Ticks, in denen der Rahmen fuer `o` `Runtime(Driver)` erhebt: je
/// einer nach dem gescheiterten Commit.
pub const DRIVER_FAULTS: [u64; 2] = [4, 9];

/// `edge_r/level` (`i32`) in Tick `tick`, ein Kanal, den `recorded.takt`
/// nicht liest (8.2): Wert und Qualitaet (3 ist `Bad`); in Tick 3 liefert
/// der Treiber nichts.
pub fn level(tick: u64) -> Option<(i32, u8)> {
    Some(match tick {
        0 | 1 => (5, 0),
        3 => return None,
        4 => (0, 3),
        _ => (7, 0),
    })
}

/// `edge_r/temp` (`float[degC]`, `f64`): Wert und Zeitstempel, ohne
/// Angabe die Tickgrenze.
pub fn temp(tick: u64) -> (f64, Option<i64>) {
    match tick {
        0 | 1 => (21.5, None),
        2 => (22.0, Some(15_000_000)),
        _ => (22.0, None),
    }
}

/// `edge_r/on` (`bool`).
pub fn on(tick: u64) -> bool {
    tick < 3
}

/// `edge_r/mode`: die Diskriminante von `Mode` (`IDLE`, `RUN`, `ERROR`);
/// 7 kennt das Enum nicht.
pub fn mode(tick: u64) -> u32 {
    match tick {
        0 | 1 => 0,
        2..=4 => 1,
        6 => 7,
        _ => 2,
    }
}

/// Ein Strom des Pruefgeraets, den `recorded.takt` nicht liest (8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unread {
    /// `edge_r/frames`, Records `Pair` in kanonischer Form.
    Frames,
    /// `edge_r/text`, `line<8>`.
    Text,
    /// `edge_r/raw`, `u8`.
    Raw,
}

/// Das `i`-te Element eines solchen Stroms in Tick `tick`: Bytes, eigener
/// Zeitstempel und eigene Folgenummer, ohne Angabe die des Rahmens.
pub fn unread(stream: Unread, tick: u64, i: usize) -> Option<(&'static [u8], Option<i64>, Option<i64>)> {
    use Unread::{Frames, Raw, Text};
    let all: &[(&[u8], Option<i64>, Option<i64>)] = match (stream, tick) {
        (Frames, 0) => &[(&[1, 2], None, None)],
        (Frames, 2) => &[(&[3, 4], None, None), (&[5, 6], Some(25_000_000), None)],
        // Ein Byte zu viel fuer `Pair`: aufgezeichnet, wie geliefert.
        (Frames, 3) => &[(&[7, 8, 9], None, None)],
        (Text, 1) => &[(b"hi", None, None)],
        (Text, 4) => &[(b"a\tb", None, None)],
        // Zu lang fuer `line<8>`, dann kein UTF-8: Die Drahtform traegt beides.
        (Text, 5) => &[(b"123456789", None, None)],
        (Text, 6) => &[(&[0xff], None, None)],
        (Raw, 0) => &[(&[65], None, None)],
        // Eine Luecke in `seq`.
        (Raw, 3) => &[(&[66], None, Some(7))],
        // Zwei Bytes sind kein `u8`.
        (Raw, 4) => &[(&[1, 2], None, None)],
        _ => &[],
    };
    all.get(i).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_violation_comes_once() {
        assert_eq!(element(Stream::Lines, 7, 2), Some((&b"3"[..], 14)));
        assert_eq!(element(Stream::Lines, 7, 3), None);
        assert_eq!(reading(Scalar::P, 2), Some((7, Some(-50_000_000))));
        assert!(!confirms(3) && !alive(8));
        assert_eq!(DRIVER_FAULTS, [4, 9]);
    }
}
