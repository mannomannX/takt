//! Das Protokoll des Messprogramms (13.8): die Zeilen, die die Bibliothek
//! aus `takt bench --emit embed` ueber die Senke des Wirts schreibt.
//!
//! ```text
//! takt bench 3f2a9c0d11e4b7a8
//! bench part 1 1
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
//! verschoben hat (FB-367), `bench part` der Laeufer eines Teils (FB-740);
//! ohne sie ist es das ganze Messprogramm. Innerhalb eines Laufs ist der Leser
//! streng: Eine unbekannte `bench`-Zeile ist ein Fehler, keine ueberlesene.

use std::collections::BTreeMap;

use super::{Part, Series};

/// Ein Lauf des Messprogramms, so wie das Board ihn meldete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Die Kennung der Kerne ([`super::suite_id`]).
    pub suite: String,
    /// Welcher Teil des Messprogramms (FB-740).
    pub part: Part,
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
    /// Faelle des Subnormal-Vektors mit falschem Ergebnis (4.2); `None`, wo
    /// der Lauf ihn nicht rechnete — in einem Teil ohne die Mathematik.
    pub subnormal: Option<u64>,
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
        Some("part") => {
            let mut next = || u32::try_from(number(w.next(), line)?).map_err(|e| format!("{e}: {line}"));
            let (index, of) = (next()?, next()?);
            run.part = Part::new(index, of).map_err(|e| format!("{e}: `bench {line}`"))?;
        }
        Some("subnormal") => run.subnormal = Some(number(w.next(), line)?),
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

/// Die Teile des Messprogramms (FB-740) als ein Lauf je Lage, in der
/// Reihenfolge, in der die Lagen zuerst vorkommen. Zu jeder Lage gehoert
/// jeder Teil derselben Teilung genau einmal, und jeder Kern steht in genau
/// einem Teil.
///
/// Erwartet Laeufe ohne Messaufwand ([`Run::without_overhead`]): Jeder Teil
/// misst seinen eigenen, denn der haengt an der Lage seines Codes, und er
/// geht von den Reihen seines Teils ab, bevor sie zusammenkommen.
pub fn combine(runs: &[Run]) -> Result<Vec<Run>, String> {
    let mut shifts: Vec<u32> = Vec::new();
    for run in runs {
        if !shifts.contains(&run.shift) {
            shifts.push(run.shift);
        }
    }
    shifts
        .iter()
        .map(|&shift| {
            let parts: Vec<&Run> = runs.iter().filter(|r| r.shift == shift).collect();
            parts_joined(&parts, shift)
        })
        .collect()
}

/// Die Teile einer Lage als ein Lauf.
fn parts_joined(parts: &[&Run], shift: u32) -> Result<Run, String> {
    let Some(first) = parts.first() else { return Err(format!("Lage {shift}: kein Teil")) };
    let of = first.part.of;
    if parts.iter().any(|r| r.part.of != of) {
        return Err(format!("Lage {shift}: Teile verschiedener Teilungen des Messprogramms"));
    }
    for index in 1..=of {
        match parts.iter().filter(|r| r.part.index == index).count() {
            1 => {}
            0 => return Err(format!("Lage {shift}: Teil {index} von {of} fehlt im Protokoll")),
            _ => return Err(format!("Lage {shift}: Teil {index} von {of} steht mehrfach im Protokoll")),
        }
    }
    let mut out = Run { part: Part::WHOLE, ..(*first).clone() };
    for run in parts.iter().skip(1) {
        if run.suite != out.suite || run.core_hz != out.core_hz {
            return Err(format!(
                "Lage {shift}: die Teile passen nicht zusammen: Kennung {} bei {} Hz gegen {} bei {} Hz",
                out.suite, out.core_hz, run.suite, run.core_hz
            ));
        }
        out.overhead = joined(&out.overhead, &run.overhead, "overhead")?;
        for (map, theirs) in [(&mut out.takt, &run.takt), (&mut out.c, &run.c), (&mut out.entries, &run.entries)] {
            for (name, series) in theirs {
                if map.insert(name.clone(), *series).is_some() {
                    return Err(format!("Lage {shift}: `{name}` steht in zwei Teilen"));
                }
            }
        }
        for m in &run.math {
            if out.math.iter().any(|o| o.index == m.index) {
                return Err(format!("Lage {shift}: Vektor {} der Mathematik steht in zwei Teilen", m.index));
            }
            out.math.push(m.clone());
        }
        out.subnormal = match (out.subnormal, run.subnormal) {
            (Some(_), Some(_)) => return Err(format!("Lage {shift}: der Subnormal-Vektor steht in zwei Teilen")),
            (ours, theirs) => ours.or(theirs),
        };
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
        assert!(runs(&LOG.replace("bench shift 8", "bench part 3 2")).is_err(), "Teil 3 von 2");
    }

    /// Zwei Teile einer Lage (FB-740): `pid` im ersten, die Mathematik im
    /// zweiten. Ohne `bench part` ist ein Lauf das ganze Messprogramm.
    fn two_parts() -> [Run; 2] {
        let first = LOG.replace("bench shift 8", "bench part 1 2\r\nbench shift 8");
        let first = first
            .lines()
            .filter(|l| !l.starts_with("bench entry") && !l.starts_with("math ") && !l.starts_with("bench subnormal"))
            .collect::<Vec<_>>()
            .join("\n");
        let second = LOG
            .replace("bench shift 8", "bench part 2 2\r\nbench shift 8")
            .replace("bench overhead min 6 mean 6 max 9", "bench overhead min 7 mean 7 max 7");
        let second = second.lines().filter(|l| !l.contains(" pid ")).collect::<Vec<_>>().join("\n");
        [runs(&first).expect("Teil 1").remove(0), runs(&second).expect("Teil 2").remove(0)]
    }

    /// Die Teile einer Lage werden ein Lauf: jeder Kern, die Mathematik und
    /// der Subnormal-Vektor aus seinem Teil; jeder Teil mit seinem eigenen
    /// Messaufwand.
    #[test]
    fn the_parts_of_a_placement_combine() {
        assert_eq!(runs(LOG).expect("lesbar")[0].part, Part::WHOLE);
        let [a, b] = two_parts();
        assert_eq!((a.part, b.part), (Part { index: 1, of: 2 }, Part { index: 2, of: 2 }));
        assert_eq!((a.subnormal, b.subnormal), (None, Some(0)));
        let combined = combine(&[a.without_overhead(), b.without_overhead()]).expect("passt");
        let [run] = combined.as_slice() else { panic!("eine Lage: {combined:?}") };
        assert_eq!((run.part, run.shift, run.subnormal), (Part::WHOLE, 8, Some(0)));
        assert_eq!(run.takt["pid"].max, 744, "der Messaufwand des ersten Teils");
        assert_eq!((run.entries["f64"].min, run.math[0].cycles), (9493, 4173), "der des zweiten");
        assert_eq!(run.overhead.n, 400);
    }

    /// Fehlt ein Teil, steht er doppelt oder ein Kern in zwei Teilen, gilt
    /// das Protokoll nicht.
    #[test]
    fn a_missing_or_doubled_part_is_refused() {
        let [a, b] = two_parts();
        let e = combine(std::slice::from_ref(&a)).expect_err("Teil 2 fehlt");
        assert!(e.contains("Teil 2 von 2 fehlt"), "{e}");
        assert!(combine(&[a.clone(), a.clone(), b.clone()]).is_err(), "Teil 1 doppelt");
        let mut both = b.clone();
        both.takt.insert("pid".into(), a.takt["pid"]);
        let e = combine(&[a.clone(), both]).expect_err("pid in zwei Teilen");
        assert!(e.contains("`pid` steht in zwei Teilen"), "{e}");
        let whole = Run { part: Part::WHOLE, ..b };
        assert!(combine(&[a, whole]).is_err(), "Teile verschiedener Teilungen");
    }
}
