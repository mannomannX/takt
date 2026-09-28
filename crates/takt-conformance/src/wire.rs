//! Der Boardmodus von `takt driver-test` (13.8): die Messschleife.
//!
//! Ein Output ist auf einen Input gebrueckt (`gpio/loop_out` an
//! `gpio/loop_in`); das Messprogramm liest je Tick den Eingang und
//! schreibt den anderen Pegel, das Board misst mit dem Zyklenzaehler
//! (`takt_board_support::wire`) und meldet am Ende eine Zeile. Daraus
//! werden `guard_ns` und `jitter_ns` des Outputs und `latency_ns` des
//! Inputs in der Hardware-Konfiguration (8.10).
//!
//! **Was nicht gemessen wird.** Die Rahmen wenden geplante Ausgaben zu
//! Tickbeginn an (9.8, `harness::scheduled`); ein `at T` faellt auf den
//! Tick, der `T` enthaelt. Das ist eine Eigenschaft des Rahmens, keine
//! Messgroesse, und steht als `tick_granular = true` am Output (7.5).

/// Die Adresse des Ausgangs der Schleife.
pub const OUT: &str = "gpio/loop_out";

/// Die Adresse des Eingangs der Schleife.
pub const IN: &str = "gpio/loop_in";

/// So viele Ticks laeuft die Messung.
pub const TICKS: u64 = 2_000;

/// Das Messprogramm: je Tick lesen, dann den anderen Pegel schreiben.
pub const PROBE: &str = "system:
    language = 1
    tick     = 1 ms

# Die Messschleife von `takt driver-test --board` (13.8).
output probe : bool @ hw(\"gpio/loop_out\") with safe = false
input  echo  : bool @ hw(\"gpio/loop_in\") with max_age = 10 ms

machine wire:
    pub var seen : bool = false
    var level    : bool = false

    initial RUN

    state RUN:
        loop:
            seen = echo
            level = not level
            probe = level
";

/// Was die Schleife gemessen hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loop {
    /// Schreibvorgaenge, deren Pegel ankam.
    pub arrived: u64,
    /// Vom Schreibaufruf, bis der Pegel am Eingang ansteht (7.5).
    pub guard_ns: i64,
    /// Wie weit der Schreibaufruf um den Tickbeginn streut.
    pub jitter_ns: i64,
    /// Wie lange ein Lesevorgang dauert.
    pub latency_ns: i64,
}

impl Loop {
    /// Die Werte fuer die Hardware-Konfiguration, je Adresse.
    pub fn values(&self) -> [(&'static str, Vec<(&'static str, String)>); 2] {
        [
            (
                OUT,
                vec![
                    ("guard_ns", self.guard_ns.to_string()),
                    ("jitter_ns", self.jitter_ns.to_string()),
                    ("tick_granular", "true".to_string()),
                ],
            ),
            (IN, vec![("latency_ns", self.latency_ns.to_string())]),
        ]
    }
}

/// Liest die Zeile `takt schleife …` aus dem Trace eines Laufs.
///
/// Eine offene Bruecke ist ein Fehler: Ein Pegel, der nicht ankam, oder ein
/// Lesen, das nicht den geschriebenen Pegel sah, macht jede Zahl wertlos.
pub fn parse(text: &str) -> Result<Loop, String> {
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("takt schleife "))
        .ok_or("keine Zeile `takt schleife`: bindet das Board die Schleife?")?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let value = |key: &str| -> Result<i64, String> {
        let at = words.iter().position(|w| *w == key).ok_or_else(|| format!("`{key}` fehlt in `{line}`"))?;
        words.get(at + 1).and_then(|v| v.parse().ok()).ok_or_else(|| format!("`{key}` ohne Zahl in `{line}`"))
    };
    let (lost, wrong) = (value("verloren")?, value("falsch")?);
    let arrived = value("angekommen")?;
    if lost > 0 || wrong > 0 || arrived == 0 {
        return Err(format!(
            "die Bruecke {OUT} -> {IN} ist offen: {arrived} Pegel angekommen, {lost} verloren, {wrong} falsch gelesen"
        ));
    }
    Ok(Loop {
        arrived: arrived as u64,
        guard_ns: value("guard_ns")?,
        jitter_ns: value("jitter_ns")?,
        latency_ns: value("latency_ns")?,
    })
}

/// Baut das Messprogramm fuer das Board, laesst es in Echtzeit laufen und
/// liest die Schleife.
#[cfg(feature = "board")]
pub fn measure(board: &mut dyn crate::board::Board) -> Result<Loop, String> {
    let dir = std::env::temp_dir().join("takt-wire");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("probe.takt");
    std::fs::write(&path, PROBE).map_err(|e| format!("{}: {e}", path.display()))?;
    let options = crate::board::Options::timed(TICKS);
    let elf = board.build(&path, &options)?;
    parse(&board.run(&elf, &options)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = "takt schleife angekommen 2001 verloren 0 falsch 0 guard_ns 44 jitter_ns 1210 latency_ns 32";

    #[test]
    fn a_closed_loop_reads_back() {
        let l = parse(&format!("t=1999 out probe true\n{LINE}\ntakt schlief 0\ntakt end\n")).expect("geschlossen");
        assert_eq!(l, Loop { arrived: 2001, guard_ns: 44, jitter_ns: 1210, latency_ns: 32 });
        let [(out, o), (inp, i)] = l.values();
        assert_eq!((out, inp), (OUT, IN));
        assert!(o.contains(&("tick_granular", "true".to_string())) && i == [("latency_ns", "32".to_string())]);
    }

    #[test]
    fn an_open_bridge_is_an_error() {
        let e = parse(&LINE.replace("verloren 0", "verloren 3")).expect_err("offen");
        assert!(e.contains("offen") && e.contains("3 verloren"), "{e}");
        assert!(parse("takt end\n").is_err());
    }
}
