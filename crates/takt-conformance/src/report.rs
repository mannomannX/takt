//! Der Konformitaetsbericht (13.8, `grammar/conformance-report.md`).
//!
//! **Bericht und Hardware-Konfiguration tragen dieselben Zahlen mit
//! verschiedener Aufgabe** (13.8): Der Bericht ist das Protokoll der
//! Messung mit ihren Umstaenden — Datum, Board, Profil, Wiederholungen,
//! Streuung —, die Konfiguration traegt das Ergebnis, das der Compiler
//! liest. Wer eine Zahl der Konfiguration anzweifelt, findet hier, wie sie
//! entstand. `takt bench` schreibt beide in einem Lauf.
//!
//! Das Format ist zeilenweiser Text wie die Hardware-Konfiguration: Kopf
//! `# takt-conformance <version>`, Abschnitte in eckigen Klammern,
//! `schluessel = wert`. Der Leser ist streng — ein unbekannter Schluessel
//! ist ein Fehler mit Zeilennummer —, weil ein Bericht ein Beleg ist und
//! ein stillschweigend ueberlesenes Feld ein Beleg weniger waere.

use std::fmt::Write as _;

/// Formatversion dieses Schreibers (11.3); Leser nehmen jede bis zu ihrer.
///
/// 2: das Messprogramm als Baustein (M11 Schritt 12) — `suite` und
/// `placements` im Lauf, `overhead` in der Kalibrierung, `function` und
/// `placements` je Probe; die Kerne heissen wie im Protokoll. 3: die
/// Stack-Reserve je Form (`stack_reserve_<form>`, FB-513). 4: das Urteil der
/// Portpruefung je Port (`[port <name>]`, M11 Schritt 15). 5: der
/// Speicherschutz je Port (`protection`, M11 Schritt 23). 6: ohne
/// Zeitgarantie ihr Grund je Port (`untimed`, FB-860).
pub const FORMAT_VERSION: u32 = 6;

const MAGIC: &str = "takt-conformance";

/// Ein Konformitaetsbericht.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Die Formatversion der gelesenen Datei.
    pub format_version: u32,
    /// Die Umstaende des Laufs.
    pub run: Run,
    /// Was die Kalibrierung ergab, ausser den Gewichten.
    pub calibration: Option<Calibration>,
    /// Je Gewicht seine Herkunft.
    pub probes: Vec<Probe>,
    /// Je gemessenem Kern, ob die Tabelle ihn deckt.
    pub checks: Vec<Check>,
    /// Die Referenzkerne aus 13.8, Takt gegen C.
    pub kernels: Vec<Kernel>,
    /// Die kuratierten Natives: Ergebnisse gegen den Wirt, Stack gegen die
    /// Zusage (4.5, 13.8).
    pub natives: Vec<NativeEntry>,
    /// Der Korpus auf dem Board gegen den Interpreter.
    pub corpus: Option<Corpus>,
    /// Die Portpruefung je Port (12.11): das Urteil von `takt port-test
    /// --judge` mit dem, was es gemessen hat.
    pub ports: Vec<PortEntry>,
}

/// Die Umstaende eines Laufs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    /// Datum, `JJJJ-MM-TT`.
    pub date: String,
    /// Das Board (`stm32f401`, `esp32c6`).
    pub board: String,
    /// Die Zielklasse, wie `takt build --target` sie nennt (12.8).
    pub target: String,
    /// Das Laufzeitprofil (12.8).
    pub profile: String,
    /// Das Werkzeug mit Version.
    pub tool: String,
    /// Kerntakt in Hertz.
    pub core_hz: u32,
    /// Messungen je Kern und Lage.
    pub runs: u64,
    /// Die Kennung des Messprogramms (`takt bench --emit embed`).
    pub suite: String,
    /// Die Lagen, in denen gemessen wurde, als Verschiebung in Byte (FB-367).
    pub placements: Vec<u32>,
}

/// Was die Kalibrierung neben den Gewichten ergab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Calibration {
    /// `T_IO` in Pikosekunden (7.2).
    pub t_io_ps: u64,
    /// Die Streckung der Tabelle als Bruch; `(1, 1)` ohne.
    pub stretch: (u64, u64),
    /// Stack-Reserve von Runtime, Treibern, ISRs und Port in Byte, je Form
    /// (12.3, FB-513).
    pub stack_reserve: takt_mir::hardware::Reserves,
    /// Faelle des Subnormal-Vektors mit falschem Ergebnis (4.2).
    pub subnormal_failures: u64,
    /// Was die Messung selbst kostet, in Zyklen; von jeder Reihe abgezogen.
    pub overhead: u64,
}

/// Kuerzeste, mittlere und laengste Messung in Zyklen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spread {
    /// Kuerzeste Messung.
    pub min: u64,
    /// Mittelwert.
    pub mean: u64,
    /// Laengste Messung.
    pub max: u64,
}

/// Die Herkunft eines Gewichts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// Der Schluessel wie in der Hardware-Konfiguration (`i32`, `f64_div`).
    pub name: String,
    /// Bei der Mathematik die Funktion, an der das Gewicht haengt.
    pub function: Option<String>,
    /// Das Gewicht in Pikosekunden, vor einer Streckung.
    pub ps: u64,
    /// Um wie viele Operationen der Klasse sich die beiden Kerne unterscheiden.
    pub ops: u64,
    /// Der kleine Kern.
    pub small: Spread,
    /// Der grosse Kern.
    pub large: Spread,
    /// Das Gewicht je Lage in Pikosekunden, wenn in mehreren gemessen wurde.
    pub placements: Vec<u64>,
}

/// Ein Kern gegen die gestreckte Tabelle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    /// Name des Kerns.
    pub name: String,
    /// Gemessen, Pikosekunden (Maximum).
    pub measured_ps: u64,
    /// Die Schranke `Σ N_c · c_target[c] + T_IO`.
    pub bound_ps: u64,
}

/// Ein Referenzkern aus 13.8.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kernel {
    /// Name.
    pub name: String,
    /// Der Takt-Kern.
    pub takt: Spread,
    /// Die C-Referenz.
    pub c: Spread,
    /// Ergaben beide denselben Wert? Ohne gleiche Werte ist das
    /// Verhaeltnis keine Aussage ueber die Sprache.
    pub same_digest: bool,
    /// Implizite Pruefungen, die im Takt-Kern blieben (3.4).
    pub implicit_checks: u32,
}

/// Eine kuratierte Native auf dem Board.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeEntry {
    /// Name, wie das Programm sie ruft.
    pub name: String,
    /// Gerechnete Vektoren.
    pub vectors: u32,
    /// Jedes Ergebnis bitgleich mit dem Wirt?
    pub same_result: bool,
    /// Der groesste gemessene Stack-Bedarf in Byte.
    pub stack: u32,
    /// Die Zusage `stack` (4.5).
    pub contract: u32,
}

/// Der Korpus auf dem Board.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Corpus {
    /// Programme im Lauf.
    pub programs: u32,
    /// Programme mit Abweichung vom Interpreter.
    pub deviations: u32,
}

/// Das Urteil der Portpruefung ueber einen Port (12.11, 13.8). Was die
/// Ausgabe nicht nannte, fehlt auch hier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortEntry {
    /// Der Port: seine Form, wo es mehrere gibt mit dem Rahmen (`interrupt`,
    /// `rtic`, `embassy`).
    pub name: String,
    /// Datum des Urteils, `JJJJ-MM-TT`.
    pub date: String,
    /// Hat `takt port-test --judge` die Ausgabe angenommen?
    pub passed: bool,
    /// Zeitzeilen des Laufs.
    pub ticks: u64,
    /// Median der `drift` in ns (7.3).
    pub drift_median_ns: i64,
    /// Der spaeteste Tick in ns ueber dem Median.
    pub drift_late_ns: i64,
    /// Um wie viel der Abstand zweier Tickbeginne die Periode hoechstens
    /// ueberschritt, in ns.
    pub jitter_ns: Option<i64>,
    /// Runden der fremden FPU-Last neben dem Lauf.
    pub foreign_rounds: Option<u64>,
    /// Davon falsch gerechnet.
    pub foreign_errors: Option<u64>,
    /// Der laengste Job in Ticks vom Start bis zur Fertigstellung.
    pub longest_job_ticks: Option<u64>,
    /// Tiefe und Schranke des Schritt-Stacks in Byte.
    pub stack: Option<(u64, u64)>,
    /// Tiefe und Schranke des Job-Stacks in Byte.
    pub job_stack: Option<(u64, u64)>,
    /// Die Jobs rechnen auf dem Schritt-Stack, dessen Schranke sie deckt
    /// (FB-459); dann fehlt `job_stack`.
    pub job_stack_shared: bool,
    /// Schreibvorgaenge des Journals.
    pub journal_writes: Option<u64>,
    /// Der Speicherschutz, den der Kopf des Laufs nannte (12.11): `Fenster`,
    /// `Partition` oder `keiner`.
    pub protection: Option<String>,
    /// Ohne Zeitgarantie ihr Grund: Die Zeit ist gemessen, nicht bewertet
    /// (13.8, FB-860).
    pub untimed: Option<String>,
}

/// Was beim Lesen schiefgehen kann; jeder Fall nennt die Zeile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Zeilennummer, ab 1.
    pub line: u32,
    /// Was nicht stimmt.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Zeile {}: {}", self.line, self.message)
    }
}

/// Schreibt einen Bericht.
pub fn render(r: &Report) -> String {
    let mut s = format!("# {MAGIC} {FORMAT_VERSION}\n");
    s.push_str("# Konformitaetsbericht (13.8): das Protokoll der Messung. Zeiten in Pikosekunden,\n");
    s.push_str("# Streuungen als `min mean max` in Zyklen.\n");
    let run = &r.run;
    let _ = write!(
        s,
        "\n[run]\ndate = \"{}\"\nboard = \"{}\"\ntarget = \"{}\"\nprofile = \"{}\"\ntool = \"{}\"\ncore_hz = {}\nruns = {}\n\
         suite = \"{}\"\nplacements = {}\n",
        run.date,
        run.board,
        run.target,
        run.profile,
        run.tool,
        run.core_hz,
        run.runs,
        run.suite,
        numbers(&run.placements)
    );
    if let Some(c) = &r.calibration {
        let _ = write!(s, "\n[calibration]\nt_io_ps = {}\nstretch = {}/{}\n", c.t_io_ps, c.stretch.0, c.stretch.1);
        for (form, bytes) in c.stack_reserve.iter() {
            let _ = writeln!(s, "{} = {bytes}", form.reserve_key());
        }
        let _ = writeln!(s, "subnormal_failures = {}\noverhead = {}", c.subnormal_failures, c.overhead);
    }
    for p in &r.probes {
        let _ = write!(s, "\n[probe {}]\n", p.name);
        if let Some(f) = &p.function {
            let _ = writeln!(s, "function = \"{f}\"");
        }
        let _ =
            write!(s, "ps = {}\nops = {}\nsmall = {}\nlarge = {}\n", p.ps, p.ops, spread(&p.small), spread(&p.large));
        if !p.placements.is_empty() {
            let _ = writeln!(s, "placements = {}", numbers(&p.placements));
        }
    }
    for c in &r.checks {
        let _ = write!(s, "\n[check {}]\nmeasured_ps = {}\nbound_ps = {}\n", c.name, c.measured_ps, c.bound_ps);
    }
    for k in &r.kernels {
        let _ = write!(
            s,
            "\n[kernel {}]\ntakt = {}\nc = {}\nsame_digest = {}\nimplicit_checks = {}\n",
            k.name,
            spread(&k.takt),
            spread(&k.c),
            k.same_digest,
            k.implicit_checks
        );
    }
    for n in &r.natives {
        let _ = write!(
            s,
            "\n[native {}]\nvectors = {}\nsame_result = {}\nstack = {}\ncontract = {}\n",
            n.name, n.vectors, n.same_result, n.stack, n.contract
        );
    }
    if let Some(c) = &r.corpus {
        let _ = write!(s, "\n[corpus]\nprograms = {}\ndeviations = {}\n", c.programs, c.deviations);
    }
    for p in &r.ports {
        let _ = write!(
            s,
            "\n[port {}]\ndate = \"{}\"\npassed = {}\nticks = {}\ndrift_median_ns = {}\ndrift_late_ns = {}\n",
            p.name, p.date, p.passed, p.ticks, p.drift_median_ns, p.drift_late_ns
        );
        let numbers = [
            ("jitter_ns", p.jitter_ns.map(|j| j.to_string())),
            ("foreign_rounds", p.foreign_rounds.map(|n| n.to_string())),
            ("foreign_errors", p.foreign_errors.map(|n| n.to_string())),
            ("longest_job_ticks", p.longest_job_ticks.map(|n| n.to_string())),
            ("stack", p.stack.map(|(d, b)| format!("{d} {b}"))),
            ("job_stack", p.job_stack.map(|(d, b)| format!("{d} {b}"))),
            ("job_stack_shared", p.job_stack_shared.then(|| "true".to_string())),
            ("journal_writes", p.journal_writes.map(|n| n.to_string())),
            ("protection", p.protection.as_ref().map(|c| format!("\"{c}\""))),
            ("untimed", p.untimed.as_ref().map(|c| format!("\"{c}\""))),
        ];
        for (key, value) in numbers {
            if let Some(value) = value {
                let _ = writeln!(s, "{key} = {value}");
            }
        }
    }
    s
}

fn spread(s: &Spread) -> String {
    format!("{} {} {}", s.min, s.mean, s.max)
}

/// Eine Liste von Zahlen, durch Leerzeichen getrennt.
fn numbers<T: std::fmt::Display>(values: &[T]) -> String {
    values.iter().map(ToString::to_string).collect::<Vec<_>>().join(" ")
}

/// Eine Liste von Zahlen aus [`numbers`].
fn parse_numbers<T: std::str::FromStr>(value: &str) -> Option<Vec<T>> {
    value.split_whitespace().map(|w| w.parse().ok()).collect()
}

/// Welcher Abschnitt gerade gelesen wird.
enum Section {
    Run,
    Calibration,
    Probe,
    Check,
    Kernel,
    Native,
    Corpus,
    Port,
}

/// Liest einen Bericht.
pub fn parse(text: &str) -> Result<Report, ParseError> {
    let mut r = Report::default();
    let mut section: Option<Section> = None;
    let mut seen_magic = false;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i as u32 + 1;
        let err = |message: String| ParseError { line: line_no, message };
        if !seen_magic {
            if let Some(v) = raw.trim().strip_prefix('#').and_then(|l| l.trim().strip_prefix(MAGIC)) {
                let v: u32 = v.trim().parse().map_err(|_| err("Formatversion fehlt".into()))?;
                if v > FORMAT_VERSION {
                    return Err(err(format!("Formatversion {v} ist neuer als {FORMAT_VERSION} (11.3)")));
                }
                r.format_version = v;
                seen_magic = true;
                continue;
            }
            if !raw.trim().is_empty() {
                return Err(err(format!("der Bericht muss mit `# {MAGIC} <version>` beginnen")));
            }
            continue;
        }
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(head) = line.strip_prefix('[') {
            let head = head.strip_suffix(']').ok_or_else(|| err("`]` fehlt".into()))?;
            let (kind, name) = head.split_once(' ').map_or((head, ""), |(k, n)| (k, n.trim()));
            section = Some(match (kind, name.is_empty()) {
                ("run", true) => Section::Run,
                ("calibration", true) => {
                    r.calibration = Some(Calibration { stretch: (1, 1), ..Calibration::default() });
                    Section::Calibration
                }
                ("probe", false) => {
                    r.probes.push(Probe { name: name.to_string(), ..Probe::default() });
                    Section::Probe
                }
                ("check", false) => {
                    r.checks.push(Check { name: name.to_string(), ..Check::default() });
                    Section::Check
                }
                ("kernel", false) => {
                    r.kernels.push(Kernel { name: name.to_string(), ..Kernel::default() });
                    Section::Kernel
                }
                ("native", false) => {
                    r.natives.push(NativeEntry { name: name.to_string(), ..NativeEntry::default() });
                    Section::Native
                }
                ("corpus", true) => {
                    r.corpus = Some(Corpus::default());
                    Section::Corpus
                }
                ("port", false) => {
                    r.ports.push(PortEntry { name: name.to_string(), ..PortEntry::default() });
                    Section::Port
                }
                _ => return Err(err(format!("unbekannter Abschnitt `[{head}]`"))),
            });
            continue;
        }
        let (key, value) =
            line.split_once('=').ok_or_else(|| err(format!("weder Abschnitt noch Zuweisung: `{line}`")))?;
        let (key, value) = (key.trim(), value.trim());
        let text = || value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value).to_string();
        let number = || value.parse::<u64>().map_err(|_| err(format!("`{value}` ist keine ganze Zahl")));
        let signed = || value.parse::<i64>().map_err(|_| err(format!("`{value}` ist keine ganze Zahl")));
        let depth_and_bound = || match parse_numbers::<u64>(value).as_deref() {
            Some([depth, bound]) => Ok((*depth, *bound)),
            _ => Err(err(format!("`{value}` sind nicht Tiefe und Schranke"))),
        };
        let flag = || match value {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(err(format!("`{value}` ist weder `true` noch `false`"))),
        };
        let unknown = || err(format!("unbekannter Schluessel `{key}`"));
        match section {
            Some(Section::Run) => match key {
                "date" => r.run.date = text(),
                "board" => r.run.board = text(),
                "target" => r.run.target = text(),
                "profile" => r.run.profile = text(),
                "tool" => r.run.tool = text(),
                "core_hz" => r.run.core_hz = number()? as u32,
                "runs" => r.run.runs = number()?,
                "suite" => r.run.suite = text(),
                "placements" => {
                    r.run.placements =
                        parse_numbers(value).ok_or_else(|| err(format!("`{value}` sind keine Verschiebungen")))?
                }
                _ => return Err(unknown()),
            },
            Some(Section::Calibration) => {
                let c = r.calibration.as_mut().expect("Abschnitt angelegt");
                match key {
                    "t_io_ps" => c.t_io_ps = number()?,
                    "stretch" => {
                        let (n, d) = value.split_once('/').ok_or_else(|| err(format!("`{value}` ist kein Bruch")))?;
                        let parse =
                            |t: &str| t.trim().parse::<u64>().map_err(|_| err(format!("`{value}` ist kein Bruch")));
                        c.stretch = (parse(n)?, parse(d)?);
                    }
                    key if takt_mir::hardware::Form::ALL.iter().any(|f| f.reserve_key() == key) => {
                        let form = takt_mir::hardware::Form::ALL
                            .into_iter()
                            .find(|f| f.reserve_key() == key)
                            .unwrap_or(takt_mir::hardware::Form::Own);
                        c.stack_reserve.set(form, Some(number()?));
                    }
                    "subnormal_failures" => c.subnormal_failures = number()?,
                    "overhead" => c.overhead = number()?,
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Probe) => {
                let p = r.probes.last_mut().expect("Abschnitt angelegt");
                match key {
                    "function" => p.function = Some(text()),
                    "ps" => p.ps = number()?,
                    "ops" => p.ops = number()?,
                    "placements" => {
                        p.placements =
                            parse_numbers(value).ok_or_else(|| err(format!("`{value}` sind keine Gewichte")))?
                    }
                    "small" => {
                        p.small = parse_spread(value).ok_or_else(|| err(format!("`{value}` ist keine Streuung")))?
                    }
                    "large" => {
                        p.large = parse_spread(value).ok_or_else(|| err(format!("`{value}` ist keine Streuung")))?
                    }
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Check) => {
                let c = r.checks.last_mut().expect("Abschnitt angelegt");
                match key {
                    "measured_ps" => c.measured_ps = number()?,
                    "bound_ps" => c.bound_ps = number()?,
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Kernel) => {
                let k = r.kernels.last_mut().expect("Abschnitt angelegt");
                match key {
                    "takt" => {
                        k.takt = parse_spread(value).ok_or_else(|| err(format!("`{value}` ist keine Streuung")))?
                    }
                    "c" => k.c = parse_spread(value).ok_or_else(|| err(format!("`{value}` ist keine Streuung")))?,
                    "same_digest" => k.same_digest = flag()?,
                    "implicit_checks" => k.implicit_checks = number()? as u32,
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Native) => {
                let n = r.natives.last_mut().expect("Abschnitt angelegt");
                match key {
                    "vectors" => n.vectors = number()? as u32,
                    "same_result" => n.same_result = flag()?,
                    "stack" => n.stack = number()? as u32,
                    "contract" => n.contract = number()? as u32,
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Corpus) => {
                let c = r.corpus.as_mut().expect("Abschnitt angelegt");
                match key {
                    "programs" => c.programs = number()? as u32,
                    "deviations" => c.deviations = number()? as u32,
                    _ => return Err(unknown()),
                }
            }
            Some(Section::Port) => {
                let p = r.ports.last_mut().expect("Abschnitt angelegt");
                match key {
                    "date" => p.date = text(),
                    "passed" => p.passed = flag()?,
                    "ticks" => p.ticks = number()?,
                    "drift_median_ns" => p.drift_median_ns = signed()?,
                    "drift_late_ns" => p.drift_late_ns = signed()?,
                    "jitter_ns" => p.jitter_ns = Some(signed()?),
                    "foreign_rounds" => p.foreign_rounds = Some(number()?),
                    "foreign_errors" => p.foreign_errors = Some(number()?),
                    "longest_job_ticks" => p.longest_job_ticks = Some(number()?),
                    "stack" => p.stack = Some(depth_and_bound()?),
                    "job_stack" => p.job_stack = Some(depth_and_bound()?),
                    "job_stack_shared" => p.job_stack_shared = flag()?,
                    "journal_writes" => p.journal_writes = Some(number()?),
                    "protection" => p.protection = Some(text()),
                    "untimed" => p.untimed = Some(text()),
                    _ => return Err(unknown()),
                }
            }
            None => return Err(err(format!("`{key}` steht vor jedem Abschnitt"))),
        }
    }
    if !seen_magic {
        return Err(ParseError { line: 1, message: format!("der Bericht muss mit `# {MAGIC} <version>` beginnen") });
    }
    Ok(r)
}

fn parse_spread(value: &str) -> Option<Spread> {
    let v: Vec<u64> = value.split_whitespace().map(|w| w.parse().ok()).collect::<Option<_>>()?;
    match v.as_slice() {
        [min, mean, max] => Some(Spread { min: *min, mean: *mean, max: *max }),
        _ => None,
    }
}

/// Das heutige Datum als `JJJJ-MM-TT` (UTC), fuer `Run::date`.
///
/// Aus den Tagen seit 1970 nach Hinnants `civil_from_days`: Ein Bericht
/// braucht ein Datum, und eine Abhaengigkeit fuer eines waere zu viel.
pub fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    civil_date(i64::try_from(secs / 86_400).unwrap_or(0))
}

fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        let s = |a, b, c| Spread { min: a, mean: b, max: c };
        Report {
            format_version: FORMAT_VERSION,
            run: Run {
                date: "2026-09-25".into(),
                board: "stm32f401".into(),
                target: "thumbv7em".into(),
                profile: "baremetal".into(),
                tool: "takt 0.1.0".into(),
                core_hz: 84_000_000,
                runs: 200,
                suite: "3f2a9c0d11e4b7a8".into(),
                placements: vec![0, 8, 24],
            },
            calibration: Some(Calibration {
                t_io_ps: 5_000,
                stretch: (21, 20),
                stack_reserve: {
                    let mut r = takt_mir::hardware::Reserves::default();
                    r.set(takt_mir::hardware::Form::Own, Some(1_432));
                    r.set(takt_mir::hardware::Form::Rtos, Some(2_048));
                    r
                },
                subnormal_failures: 0,
                overhead: 6,
            }),
            probes: vec![
                Probe {
                    name: "i32".into(),
                    function: None,
                    ps: 11_905,
                    ops: 192,
                    small: s(100, 101, 110),
                    large: s(292, 293, 300),
                    placements: vec![11_905, 11_905, 11_905],
                },
                Probe {
                    name: "f64_math".into(),
                    function: Some("acos".into()),
                    ps: 1_008_345_000,
                    ops: 8,
                    small: s(170_000, 170_100, 170_400),
                    large: s(848_000, 848_300, 848_900),
                    placements: Vec::new(),
                },
            ],
            checks: vec![Check { name: "i32_large".into(), measured_ps: 3_571_500, bound_ps: 3_600_000 }],
            kernels: vec![Kernel {
                name: "crc32".into(),
                takt: s(9_000, 9_010, 9_050),
                c: s(8_800, 8_800, 8_820),
                same_digest: true,
                implicit_checks: 2,
            }],
            natives: vec![NativeEntry {
                name: "sha256".into(),
                vectors: 9,
                same_result: true,
                stack: 312,
                contract: 512,
            }],
            corpus: Some(Corpus { programs: 44, deviations: 0 }),
            ports: vec![
                PortEntry {
                    name: "interrupt".into(),
                    date: "2026-10-11".into(),
                    passed: true,
                    ticks: 2998,
                    drift_median_ns: 3_100,
                    drift_late_ns: 2_400,
                    jitter_ns: Some(2_200),
                    foreign_rounds: Some(41_000),
                    foreign_errors: Some(0),
                    longest_job_ticks: Some(4),
                    stack: Some((3_900, 4_500)),
                    job_stack: Some((1_100, 2_700)),
                    job_stack_shared: false,
                    journal_writes: None,
                    protection: Some("Fenster".into()),
                    untimed: None,
                },
                PortEntry {
                    name: "posix".into(),
                    date: "2026-10-21".into(),
                    passed: true,
                    untimed: Some("kein PREEMPT_RT".into()),
                    ..PortEntry::default()
                },
            ],
        }
    }

    #[test]
    fn a_report_round_trips() {
        let r = sample();
        let text = render(&r);
        assert!(text.starts_with("# takt-conformance 6\n"), "{text}");
        assert!(text.contains("[port posix]\n") && text.contains("untimed = \"kein PREEMPT_RT\"\n"), "{text}");
        assert!(text.contains("job_stack = 1100 2700\nprotection = \"Fenster\"\n"), "{text}");
        assert!(text.contains("[port interrupt]\ndate = \"2026-10-11\"\npassed = true\n"), "{text}");
        assert!(text.contains("stack = 3900 4500\njob_stack = 1100 2700\n"), "{text}");
        assert!(!text.contains("journal_writes"), "was die Ausgabe nicht nannte, fehlt: {text}");
        assert!(text.contains("stack_reserve = 1432\nstack_reserve_rtos = 2048\n"), "Reserve je Form: {text}");
        assert!(text.contains("[probe f64_math]\nfunction = \"acos\"\nps = "), "{text}");
        assert!(text.contains("placements = 0 8 24\n") && text.contains("placements = 11905 11905 11905\n"), "{text}");
        assert_eq!(parse(&text), Ok(r));
        // Version 1 kannte Kennung, Lagen und Messaufwand nicht; sie liest sich weiter.
        let old = parse("# takt-conformance 1\n[run]\nruns = 200\n[calibration]\nt_io_ps = 5\n").expect("Version 1");
        assert_eq!((old.run.suite.as_str(), old.run.placements.len()), ("", 0));
    }

    /// Ein Bericht ist ein Beleg: Ein unbekannter Schluessel faellt auf.
    #[test]
    fn an_unknown_key_is_an_error_with_its_line() {
        let text = render(&sample()).replace("ops = 192", "operations = 192");
        let e = parse(&text).expect_err("abgelehnt");
        assert!(e.message.contains("operations"), "{e}");
        assert!(e.line > 1);
    }

    /// Der Vektor der Spezifikation (R4) ist ein Bericht dieser Version.
    #[test]
    fn the_vector_of_the_format_reads() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/conformance-report.md");
        let spec = std::fs::read_to_string(path).expect("Spezifikation");
        let vector = spec
            .split("## R4")
            .nth(1)
            .and_then(|r| r.split("```text\n").nth(1))
            .and_then(|r| r.split("```").next())
            .expect("Vektor in R4");
        let r = parse(vector).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r.format_version, FORMAT_VERSION);
        assert!(r.ports.iter().any(|p| p.passed && p.stack.is_some()), "{r:?}");
    }

    #[test]
    fn a_newer_version_is_refused() {
        assert!(parse("# takt-conformance 99\n").is_err());
        assert!(parse("[run]\n").is_err(), "ohne Kopf");
    }

    #[test]
    fn the_calendar_is_right() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_721), "2026-09-25");
        assert_eq!(civil_date(11_016), "2000-02-29");
    }
}
