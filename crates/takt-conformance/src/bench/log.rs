//! Das Protokoll des Messprogramms (13.8): die Zeilen, die die Bibliothek
//! aus `takt bench --emit embed` ueber die Senke des Wirts schreibt.
//!
//! ```text
//! takt bench 3f2a9c0d11e4b7a8
//! bench core_hz 84000000
//! bench shift 8
//! bench overhead min 6 mean 6 max 7 n 200 digest 0
//! bench takt frame min 590 mean 596 max 633 n 200 digest 1
//! bench takt pid min 702 mean 711 max 750 n 200 digest 4711
//! bench c pid min 640 mean 644 max 668 n 200 digest 4711
//! bench entry f32 min 3100 mean 3104 max 3121 n 200 digest 1062406040
//! bench entry f64 min 9500 mean 9510 max 9544 n 200 digest 4604930618986332160
//! math 0 3ff0000000000000 cycles 4180
//! bench subnormal 0
//! takt end
//! ```
//!
//! Ein Lauf beginnt mit `takt bench <kennung>` und endet mit `takt end`;
//! was davor und danach steht — Startmeldungen, ein zweiter Lauf —, gehoert
//! nicht dazu. `bench shift` schreibt der Wirt, wenn er sein Abbild
//! verschoben hat (FB-367). Innerhalb eines Laufs ist der Leser streng: Eine
//! unbekannte `bench`-Zeile ist ein Fehler, keine ueberlesene.

use std::collections::BTreeMap;

use super::Series;

/// Ein Lauf des Messprogramms, so wie das Board ihn meldete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Die Kennung der Kerne ([`super::suite_id`]).
    pub suite: String,
    /// Kerntakt in Hertz.
    pub core_hz: u32,
    /// Um wie viele Byte der Wirt sein Programm verschoben hat.
    pub shift: u32,
    /// Was die Messung selbst kostet: zwei Lesungen des Zaehlers.
    pub overhead: Series,
    /// Die Takt-Kerne nach Namen.
    pub takt: BTreeMap<String, Series>,
    /// Die C-Referenzen nach dem Namen ihres Kerns.
    pub c: BTreeMap<String, Series>,
    /// Der Einstieg der Probe `math` an ihren Argumenten, je Breite (`f32`,
    /// `f64`).
    pub entries: BTreeMap<String, Series>,
    /// Die Mathematik an ihren Vektoren, ohne Stack.
    pub math: Vec<crate::math::Measured>,
    /// Faelle des Subnormal-Vektors mit falschem Ergebnis (4.2).
    pub subnormal: u64,
}

/// Die Laeufe in einem Protokoll, in ihrer Reihenfolge.
pub fn runs(text: &str) -> Result<Vec<Run>, String> {
    let mut out = Vec::new();
    let mut current: Option<Run> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(suite) = line.strip_prefix("takt bench ") {
            if current.is_some() {
                return Err(format!("`{line}` mitten in einem Lauf: Es fehlt `takt end`"));
            }
            current = Some(Run { suite: suite.trim().to_string(), ..Run::default() });
            continue;
        }
        if line == "takt end" {
            if let Some(run) = current.take() {
                out.push(complete(run)?);
            }
            continue;
        }
        let Some(run) = current.as_mut() else { continue };
        if line.starts_with("math ") {
            run.math.extend(crate::math::parse(line)?);
            continue;
        }
        let Some(rest) = line.strip_prefix("bench ") else { continue };
        read(run, rest)?;
    }
    if current.is_some() {
        return Err("das Protokoll endet mitten in einem Lauf: Es fehlt `takt end`".into());
    }
    Ok(out)
}

/// Eine `bench`-Zeile in den Lauf.
fn read(run: &mut Run, line: &str) -> Result<(), String> {
    let mut w = line.split_whitespace();
    match w.next() {
        Some("core_hz") => run.core_hz = u32::try_from(number(w.next(), line)?).map_err(|e| format!("{e}: {line}"))?,
        Some("shift") => run.shift = u32::try_from(number(w.next(), line)?).map_err(|e| format!("{e}: {line}"))?,
        Some("subnormal") => run.subnormal = number(w.next(), line)?,
        Some("overhead") => run.overhead = series(w, line)?,
        Some(kind @ ("takt" | "c" | "entry")) => {
            let name = w.next().ok_or_else(|| format!("Name fehlt in `bench {line}`"))?.to_string();
            let s = series(w, line)?;
            let map = match kind {
                "takt" => &mut run.takt,
                "c" => &mut run.c,
                _ => &mut run.entries,
            };
            if map.insert(name.clone(), s).is_some() {
                return Err(format!("`{name}` zweimal in einem Lauf"));
            }
        }
        _ => return Err(format!("unbekannte Zeile `bench {line}`")),
    }
    Ok(())
}

/// Ein Lauf ist vollstaendig, wenn er Kerntakt und Messaufwand nennt.
fn complete(run: Run) -> Result<Run, String> {
    if run.core_hz == 0 || run.overhead.n == 0 {
        return Err(format!("Lauf {}: Kerntakt oder Messaufwand fehlen", run.suite));
    }
    Ok(run)
}

fn number(word: Option<&str>, line: &str) -> Result<u64, String> {
    word.and_then(|w| w.parse().ok()).ok_or_else(|| format!("keine Zahl in `bench {line}`"))
}

fn series<'a>(words: impl Iterator<Item = &'a str>, line: &str) -> Result<Series, String> {
    let words: Vec<&str> = words.collect();
    let value = |key: &str| {
        words
            .iter()
            .position(|w| *w == key)
            .and_then(|i| words.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| format!("`{key}` fehlt in `bench {line}`"))
    };
    Ok(Series {
        min: value("min")?,
        mean: value("mean")?,
        max: value("max")?,
        n: value("n")?,
        digest: value("digest")?,
    })
}

impl Run {
    /// Der Lauf ohne den Messaufwand: Das Minimum zweier Lesungen des
    /// Zaehlers steckt in jeder Messung und gehoert nicht zum Gemessenen.
    /// Abgezogen wird das Minimum, nicht mehr — so bleibt jedes Maximum eine
    /// obere Schranke.
    pub fn without_overhead(&self) -> Run {
        let cost = self.overhead.min;
        let less = |s: &Series| Series {
            min: s.min.saturating_sub(cost),
            mean: s.mean.saturating_sub(cost),
            max: s.max.saturating_sub(cost),
            ..*s
        };
        let math = self
            .math
            .iter()
            .map(|m| crate::math::Measured {
                cycles: m.cycles.saturating_sub(u32::try_from(cost).unwrap_or(u32::MAX)),
                ..m.clone()
            })
            .collect();
        Run {
            takt: self.takt.iter().map(|(k, s)| (k.clone(), less(s))).collect(),
            c: self.c.iter().map(|(k, s)| (k.clone(), less(s))).collect(),
            entries: self.entries.iter().map(|(k, s)| (k.clone(), less(s))).collect(),
            math,
            ..self.clone()
        }
    }
}

/// Mehrere Laeufe derselben Kerne — je Lage einer (FB-367) — als eine
/// Messung: je Reihe das kleinste Minimum, das groesste Maximum, der Mittelwert
/// ueber alle Messungen; je Vektor der Mathematik die meisten Zyklen. Ein
/// Kern muss in jeder Lage dasselbe ergeben haben, sonst rechneten die Laeufe
/// Verschiedenes.
pub fn merge(runs: &[Run]) -> Result<Run, String> {
    let first = runs.first().ok_or("kein Lauf des Messprogramms")?;
    let mut out = Run { shift: 0, ..first.clone() };
    for run in &runs[1..] {
        if run.suite != first.suite || run.core_hz != first.core_hz {
            return Err(format!(
                "die Laeufe passen nicht zusammen: Kennung {} bei {} Hz gegen {} bei {} Hz",
                first.suite, first.core_hz, run.suite, run.core_hz
            ));
        }
        out.overhead = joined(&out.overhead, &run.overhead, "overhead")?;
        for (map, theirs) in [(&mut out.takt, &run.takt), (&mut out.c, &run.c), (&mut out.entries, &run.entries)] {
            if map.keys().ne(theirs.keys()) {
                return Err(format!("die Laeufe in Lage {} und {} messen andere Kerne", first.shift, run.shift));
            }
            for (name, s) in map.iter_mut() {
                *s = joined(s, &theirs[name], name)?;
            }
        }
        for m in &run.math {
            match out.math.iter_mut().find(|o| o.index == m.index) {
                Some(o) if o.result != m.result => {
                    return Err(format!("Vektor {} der Mathematik ergibt in zwei Lagen Verschiedenes", m.index));
                }
                Some(o) => o.cycles = o.cycles.max(m.cycles),
                None => out.math.push(m.clone()),
            }
        }
        out.subnormal = out.subnormal.max(run.subnormal);
    }
    Ok(out)
}

/// Zwei Reihen desselben Kerns als eine.
fn joined(a: &Series, b: &Series, name: &str) -> Result<Series, String> {
    if a.digest != b.digest {
        return Err(format!("`{name}` ergibt in zwei Lagen Verschiedenes: {} gegen {}", a.digest, b.digest));
    }
    let n = a.n + b.n;
    let total = u128::from(a.mean) * u128::from(a.n) + u128::from(b.mean) * u128::from(b.n);
    let mean = if n == 0 { 0 } else { u64::try_from(total / u128::from(n)).unwrap_or(u64::MAX) };
    Ok(Series { min: a.min.min(b.min), mean, max: a.max.max(b.max), n, digest: a.digest })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "boot\r\n\ntakt bench 0123456789abcdef\r\nbench core_hz 84000000\r\nbench shift 8\r\n\
                       bench overhead min 6 mean 6 max 9 n 200 digest 0\r\n\
                       bench takt pid min 702 mean 711 max 750 n 200 digest 4711\r\n\
                       bench c pid min 640 mean 644 max 668 n 200 digest 4711\r\n\
                       bench entry f64 min 9500 mean 9510 max 9544 n 200 digest 5\r\n\
                       math 0 3ff0000000000000 cycles 4180\r\nbench subnormal 0\r\ntakt end\r\n";

    #[test]
    fn a_run_reads_back() {
        let runs = runs(LOG).expect("lesbar");
        let [run] = runs.as_slice() else { panic!("ein Lauf: {runs:?}") };
        assert_eq!((run.suite.as_str(), run.core_hz, run.shift), ("0123456789abcdef", 84_000_000, 8));
        assert_eq!(run.takt["pid"], Series { min: 702, mean: 711, max: 750, n: 200, digest: 4711 });
        assert_eq!(run.c["pid"].max, 668);
        assert_eq!(run.entries["f64"].min, 9500);
        assert_eq!((run.math.len(), run.math[0].cycles, run.math[0].stack), (1, 4180, None));
    }

    /// Der Messaufwand geht ab, sein Minimum und nicht mehr.
    #[test]
    fn the_overhead_comes_off() {
        let run = runs(LOG).expect("lesbar").remove(0).without_overhead();
        assert_eq!((run.takt["pid"].min, run.takt["pid"].max), (696, 744));
        assert_eq!(run.math[0].cycles, 4174);
    }

    /// Zwei Lagen: das groessere Maximum, das kleinere Minimum, die meisten
    /// Zyklen je Vektor; ein Kern, der in einer Lage anders rechnet, faellt auf.
    #[test]
    fn two_placements_merge_to_the_worse() {
        let a = runs(LOG).expect("lesbar").remove(0);
        let other =
            LOG.replace("max 750", "max 790").replace("min 702", "min 690").replace("cycles 4180", "cycles 4100");
        let b = runs(&other).expect("lesbar").remove(0);
        let m = merge(&[a.clone(), b]).expect("passt");
        assert_eq!((m.takt["pid"].min, m.takt["pid"].max, m.takt["pid"].n), (690, 790, 400));
        assert_eq!(m.math[0].cycles, 4180);
        let wrong = runs(&LOG.replace("digest 4711\r\nbench c", "digest 4712\r\nbench c")).expect("lesbar").remove(0);
        assert!(merge(&[a, wrong]).is_err());
    }

    #[test]
    fn a_broken_run_is_refused() {
        assert!(runs(&LOG.replace("bench subnormal 0", "bench unbekannt 0")).is_err());
        assert!(runs(&LOG.replace("takt end", "")).is_err(), "ohne Ende");
        assert!(runs(&LOG.replace("bench overhead min 6 mean 6 max 9 n 200 digest 0\r\n", "")).is_err());
        assert_eq!(runs("takt end\nbench takt x\n").expect("ausserhalb eines Laufs"), Vec::new());
    }
}
