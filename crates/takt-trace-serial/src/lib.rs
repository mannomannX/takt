//! Den Trace eines Boards lesen und gegen den Interpreter halten (13.8).
//!
//! **Warum es dieses Crate gibt.** `tools/takt-on-board.sh` nannte den
//! Vergleich „den Kern des M5-Exits" und endete mit dem Satz, der Trace
//! laufe nun auf PA9 — angeschaut hat ihn ein Mensch. Ein Exit-Kriterium,
//! das niemand automatisch prueft, ist eine Absichtserklaerung: Es faellt
//! erst auf, wenn jemand hinsieht, und es faellt nie auf, wenn niemand
//! hinsieht. Dieselbe Klasse wie das erfundene Flash-Protokoll (FB-137) —
//! ein Werkzeug, das zufrieden meldet und nichts prueft.
//!
//! **Was hier steht und was nicht.** Hier steht das Lesen: eine serielle
//! Schnittstelle oeffnen, Zeilen sammeln, ein Ende erkennen. Der
//! *Vergleich* steht nicht hier, sondern in
//! [`takt_conformance::run::compare`] — dieselbe Funktion, die schon
//! Interpreter gegen erzeugten Code haelt. Eine zweite Vergleichsfunktion
//! waere eine zweite Meinung darueber, was „gleich" heisst, und genau das
//! soll es nicht geben (9.4.4).
//!
//! **Die Zerlegung ist von der Hardware getrennt**, damit sie geprueft
//! werden kann, ohne ein Board anzuschliessen: [`Reader`] nimmt Bytes aus
//! beliebiger Quelle, [`open`] liefert die serielle.

use std::io::Read;
use std::time::{Duration, Instant};

/// Sammelt Zeilen aus einem Bytestrom.
///
/// **Ein Stueck Puffer, kein Zeilenleser.** Die serielle Schnittstelle
/// liefert, was gerade da ist; eine Zeile kann auf drei Lesevorgaenge
/// verteilt sein, und drei Zeilen koennen in einem stecken. Wer
/// `read_line` erwartet, verliert die letzte halbe Zeile oder wartet
/// ewig auf sie.
#[derive(Debug, Default)]
pub struct Reader {
    rest: String,
    lines: Vec<String>,
}

impl Reader {
    /// Nimmt gelesene Bytes auf und zerlegt, was vollstaendig ist.
    ///
    /// Bytes, die kein gueltiges UTF-8 sind, werden ersetzt statt
    /// verworfen: Ein gestoertes Zeichen soll die Zeile sichtbar falsch
    /// machen, nicht verschwinden lassen — sonst sieht ein Uebertragungs-
    /// fehler wie ein Wertunterschied aus.
    pub fn push(&mut self, bytes: &[u8]) {
        self.rest.push_str(&String::from_utf8_lossy(bytes));
        while let Some(at) = self.rest.find('\n') {
            let line = self.rest[..at].trim_end_matches('\r').trim().to_string();
            self.rest.drain(..=at);
            if !line.is_empty() {
                self.lines.push(line);
            }
        }
    }

    /// Die gesammelten Zeilen als Trace-Text.
    pub fn text(&self) -> String {
        let mut s = self.lines.join("\n");
        if !s.is_empty() {
            s.push('\n');
        }
        s
    }

    /// Die groesste Tickzahl, die eine `t=`-Zeile nennt.
    ///
    /// Damit erkennt der Aufrufer, wann genug gelesen ist — eine feste
    /// Wartezeit waere entweder zu kurz (unvollstaendiger Trace) oder zu
    /// lang (jeder Lauf kostet sie).
    pub fn last_tick(&self) -> Option<u64> {
        self.lines.iter().filter_map(|l| tick_of(l)).max()
    }

    /// Zeilen, die keine Trace-Zeilen sind.
    ///
    /// Das Board schreibt beim Start einen Banner; er gehoert nicht in den
    /// Vergleich, aber in den Bericht — dort steht der Kerntakt, und ein
    /// falscher erklaert die Haelfte aller Abweichungen.
    pub fn preamble(&self) -> Vec<&str> {
        self.lines.iter().filter(|l| tick_of(l).is_none()).map(String::as_str).collect()
    }

    /// Wie viele Bytes das Board laut seiner Bilanzzeile verworfen hat
    /// (`takt schlief … verworfen N …`); `None` ohne Bilanz.
    pub fn dropped(&self) -> Option<u64> {
        let balance = self.lines.iter().rev().find(|l| l.starts_with("takt schlief "))?;
        let mut words = balance.split_whitespace();
        words.by_ref().find(|w| *w == "verworfen")?;
        words.next()?.parse().ok()
    }
}

/// Die Tickzahl einer Zeile `t=<n> …`.
fn tick_of(line: &str) -> Option<u64> {
    line.split_whitespace().next()?.strip_prefix("t=")?.parse().ok()
}

/// Was beim Lesen schiefgehen kann.
#[derive(Debug)]
pub enum Error {
    /// Die Schnittstelle liess sich nicht oeffnen.
    Open(String),
    /// Es kam nichts oder zu wenig.
    Silent {
        /// Wie lange gewartet wurde.
        waited: Duration,
        /// Was bis dahin ankam.
        got: usize,
    },
    /// Der Trace endet vor dem verlangten Tick (13.8).
    Incomplete {
        /// Der letzte Tick, der ankam.
        reached: u64,
        /// Der verlangte.
        wanted: u64,
    },
    /// Das Board hat laut Bilanz Bytes verworfen; der Trace hat Luecken
    /// (FB-292).
    Dropped(u64),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Open(e) => write!(f, "Schnittstelle nicht zu oeffnen: {e}"),
            Error::Silent { waited, got } => write!(
                f,
                "nach {:.1} s nur {got} Zeilen — sendet das Board? (PA9, 115200 8N1; RESET druecken)",
                waited.as_secs_f32()
            ),
            Error::Incomplete { reached, wanted } => {
                write!(f, "Trace unvollstaendig: er reicht bis Tick {reached} von {wanted}")
            }
            Error::Dropped(n) => write!(f, "Trace unvollstaendig: das Board verwarf {n} Byte (FB-292)"),
        }
    }
}

/// Liest, bis `ticks` Ticks da sind (der letzte ist `ticks - 1`, wie im
/// Interpreter) oder die Zeit abgelaufen ist.
///
/// **Die Zeitgrenze ist kein Notausgang, sondern die Abbruchbedingung.**
/// Ein Board, das haengt, sendet nichts mehr; ohne Grenze wartete das
/// Werkzeug ewig, und in einer Testsuite hiesse das: sie steht. 4.1
/// verlangt beschraenkte Schleifen fuer Takt-Programme, und dasselbe ist
/// hier richtig.
///
/// **Weniger als verlangt ist ein Fehler, kein kuerzerer Vergleich**: Ein
/// Board, das bei Tick 5 von 200 verstummt, waere sonst „gleich bis Tick
/// 5“. Ebenso eine Bilanz mit verworfenen Bytes, wo sie ankam.
pub fn read_until(port: &mut dyn Read, ticks: u64, limit: Duration) -> Result<Reader, Error> {
    let mut reader = Reader::default();
    let start = Instant::now();
    let mut buf = [0u8; 512];
    while start.elapsed() < limit {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => reader.push(&buf[..n]),
            // Eine Zeitueberschreitung beim Lesen ist normal: Zwischen zwei
            // Ticks kommt nichts. Erst die aeussere Grenze entscheidet.
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(Error::Open(e.to_string())),
        }
        if let Some(n) = reader.dropped().filter(|n| *n > 0) {
            return Err(Error::Dropped(n));
        }
        if reader.last_tick().is_some_and(|t| t.saturating_add(1) >= ticks) {
            return Ok(reader);
        }
    }
    match reader.last_tick() {
        None => Err(Error::Silent { waited: start.elapsed(), got: reader.lines.len() }),
        Some(reached) if reached.saturating_add(1) < ticks => Err(Error::Incomplete { reached, wanted: ticks }),
        Some(_) => Ok(reader),
    }
}

/// Oeffnet eine serielle Schnittstelle.
///
/// 115200 8N1 ist, was `takt-board-stm32f401` einrichtet; andere Boards
/// bringen ihre eigene Rate mit, darum ist sie ein Parameter.
pub fn open(port: &str, baud: u32) -> Result<Box<dyn serialport::SerialPort>, Error> {
    serialport::new(port, baud)
        // Kurz, weil zwischen zwei Ticks nichts kommt: Der Aufruf soll
        // zurueckkehren und die aeussere Schleife entscheiden lassen.
        .timeout(Duration::from_millis(100))
        .open()
        .map_err(|e| Error::Open(format!("{port}: {e}")))
}

/// Die sichtbaren seriellen Schnittstellen, fuer die Fehlermeldung.
///
/// Ein „Schnittstelle nicht gefunden" ohne Liste laesst den Nutzer raten;
/// auf Windows heissen sie `COM3`, auf Linux `/dev/ttyUSB0`, und welche
/// gerade da ist, weiss nur das System.
pub fn available() -> Vec<String> {
    serialport::available_ports().unwrap_or_default().into_iter().map(|p| p.port_name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eine Zeile, die auf mehrere Lesevorgaenge faellt, bleibt eine.
    #[test]
    fn a_line_split_across_reads_stays_one_line() {
        let mut r = Reader::default();
        r.push(b"t=0 out ");
        r.push(b"led 1\nt=1 ");
        r.push(b"out led 0\n");
        assert_eq!(r.text(), "t=0 out led 1\nt=1 out led 0\n");
    }

    /// Eine halbe Zeile am Ende zaehlt noch nicht.
    ///
    /// Sonst stuende im Trace eine abgeschnittene Zeile, und der Vergleich
    /// meldete einen Wertunterschied, wo eine Uebertragung noch lief.
    #[test]
    fn half_a_line_is_not_yet_a_line() {
        let mut r = Reader::default();
        r.push(b"t=0 out led 1\nt=1 out le");
        assert_eq!(r.text(), "t=0 out led 1\n");
        assert_eq!(r.last_tick(), Some(0));
    }

    /// Zeilenenden mit `\r\n` werden wie `\n` behandelt.
    #[test]
    fn carriage_returns_are_stripped() {
        let mut r = Reader::default();
        r.push(b"t=0 out led 1\r\n");
        assert_eq!(r.text(), "t=0 out led 1\n");
    }

    /// Der Banner landet in der Vorrede, nicht im Trace.
    #[test]
    fn the_banner_stays_out_of_the_comparison() {
        let mut r = Reader::default();
        r.push(b"takt auf stm32f401\n  Kerntakt 84000000 Hz\nt=0 out led 1\n");
        assert_eq!(r.preamble(), vec!["takt auf stm32f401", "Kerntakt 84000000 Hz"]);
        assert_eq!(r.last_tick(), Some(0));
    }

    /// Gestoerte Bytes machen die Zeile sichtbar falsch, nicht unsichtbar.
    #[test]
    fn broken_bytes_stay_visible() {
        let mut r = Reader::default();
        r.push(b"t=0 out led \xff\xfe1\n");
        assert!(r.text().contains("t=0 out led"), "die Zeile bleibt: {:?}", r.text());
    }

    /// Das Lesen endet, wenn der gesuchte Tick da ist.
    #[test]
    fn reading_stops_at_the_requested_tick() {
        let data = b"t=0 out led 1\nt=1 out led 0\nt=2 out led 1\n";
        let mut src = &data[..];
        let r = read_until(&mut src, 2, Duration::from_secs(1)).expect("gelesen");
        assert!(r.last_tick().unwrap() >= 1);
    }

    /// Ein stummes Board wird als stumm gemeldet, nicht als leerer Trace.
    ///
    /// Ein leerer Trace verglichen mit einem vollen ergibt „keine
    /// gemeinsamen Outputs" — und das meldete `compare` als *bestanden*.
    #[test]
    fn a_silent_board_is_an_error_not_an_empty_trace() {
        let mut src = &b""[..];
        let e = read_until(&mut src, 10, Duration::from_millis(200)).expect_err("stumm");
        assert!(matches!(e, Error::Silent { .. }), "{e}");
    }

    /// **Ein Board, das vor dem verlangten Tick verstummt, ist ein Fehler**
    /// (13.8): Bis Tick 5 von 10 gleich heisst nicht gleich.
    #[test]
    fn a_trace_that_stops_early_is_incomplete() {
        let data = b"t=0 out led 1\nt=3 out led 0\nt=5 out led 1\n";
        let mut src = &data[..];
        let e = read_until(&mut src, 10, Duration::from_millis(200)).expect_err("unvollstaendig");
        assert!(matches!(e, Error::Incomplete { reached: 5, wanted: 10 }), "{e}");
        assert!(e.to_string().contains("Tick 5 von 10"), "{e}");
    }

    /// **Verworfene Bytes in der Bilanz sind ein Fehler** (FB-292): Der
    /// Trace hat dann Luecken, die wie Gleichheit aussehen koennen.
    #[test]
    fn dropped_bytes_in_the_balance_are_an_error() {
        let data = b"t=0 out led 1\ntakt schlief 0 ueberlaeufe 0 verworfen 3 gesendet 40\ntakt end\n";
        let mut src = &data[..];
        let e = read_until(&mut src, 5, Duration::from_millis(200)).expect_err("verworfen");
        assert!(matches!(e, Error::Dropped(3)), "{e}");
        let clean = b"t=0 out led 1\nt=1 out led 0\ntakt schlief 0 verworfen 0 gesendet 28\ntakt end\n";
        let mut src = &clean[..];
        assert!(read_until(&mut src, 1, Duration::from_millis(200)).is_ok());
    }
}
