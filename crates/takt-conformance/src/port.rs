//! Die Portpruefung (12.11, 13.8; M11 Schritt 15): das Urteil ueber einen
//! Port, der das Pruefprogramm gefahren hat.
//!
//! **Ein Port ist belegt, nicht geglaubt** (plan/m11.md 2.9). `takt port-test
//! --emit embed` liefert das Pruefprogramm ([`PROGRAM`]) als Baustein, die
//! Nachricht eines Jobs fuer das Ziel bemessen ([`source`]). Der
//! Port faehrt es [`TICKS`] Ticks in Echtzeit in seiner Form, seine fremde
//! Aufgabe rechnet daneben die Last aus `takt_embed::port_test`, und alles
//! geht in seine Senke. [`judge`] urteilt ueber diese Ausgabe:
//!
//! | Pflicht | woran das Urteil sie sieht |
//! |---|---|
//! | Trace gleich dem Interpreter | Marke und Bilanz vollstaendig, nichts verworfen, der Trace reicht bis zum letzten Tick und gleicht dem Interpreter, der die Job- und Runtime-Zeilen des Ports als Stimulus bekommt (12.5) |
//! | FPU-Kontext gesichert | die fremde Last rechnete im Mittel eine Runde je Tick und nie falsch; `kernel` rechnet in jedem Schritt mit `f32` und gleicht dem Interpreter |
//! | Jobs unterbrechbar | ein Job war im Tick nach seinem Start noch nicht fertig, und kein Tick begann zu spaet |
//! | Stacks unter der Schranke | Schritt- und Job-Stack gemessen, unter `TICK_STACK_BYTES` und `JOB_STACK_BYTES` (12.3); rechnen die Jobs auf dem Schritt-Stack, sagt die Bilanz `jobstack geteilt`, und seine Schranke deckt sie (FB-459) |
//! | Drift | kein Tick ueber seiner Periode und keiner eine Periode zu spaet (`ueberlaeufe`, `verspaetet`, 7.3) |
//! | Keine Runtime-Faults | keine Zeile `runtime` (5.3, 7.3, 12.3, 12.6): Das Pruefprogramm gibt einem richtigen Port keinen Anlass |
//! | Journal | geschrieben, nie fehlgeschlagen (5.9, 12.3) |
//! | Speicherschutz genannt | vor der Marke die Kopfzeile `Schutz: Fenster`, `Partition` oder `keiner` (12.11), und kein Nachweis darunter widerspricht ihr |
//!
//! Drift und Jitter stehen gemessen im Urteil und im Konformitaetsbericht
//! (`[port <name>]`), bewertet werden sie nicht. `verloren` folgt aus
//! `verspaetet`: Eine verlorene Periode hat einen Tick, der eine Periode zu
//! spaet begann (FB-519).
//!
//! **Was der Port schreibt.** Was ein Bring-up mit
//! `takt_rt_baremetal::run::report` schreibt: die Marke `takt trace`, den
//! Trace mit seinen Zeitzeilen (`t=K time … drift=N …`), vor der Bilanz die
//! Zeile der fremden Last ([`FOREIGN`]) und die Bilanz `takt schlief …` mit
//! Ueberlaeufen, Journal und Stacks, zuletzt `takt end`.
//!
//! **Nicht entscheidbar ist nicht bestanden.** Fehlt eine Zahl, die eine
//! Pflicht belegt, faellt der Port durch und das Urteil nennt sie: Eine
//! Pruefung, die bei fehlender Eingabe schweigt, waere von einer bestandenen
//! nicht zu unterscheiden.

use crate::board::{self, Drift, Options};
use crate::report::PortEntry;
use takt_rt_core::protection::{self, Case};

/// Das Pruefprogramm, ein Programm des Korpus.
pub const PROGRAM: &str = include_str!("../../../corpus-try/124_port_test.takt");

/// Sein Name im Korpus.
pub const CORPUS_NAME: &str = "124_port_test.takt";

/// Das Praefix seiner Symbole in der Lieferform (12.11).
pub const PREFIX: &str = "port_test";

/// So viele Ticks faehrt der Port: eine Sekunde Last, dann das
/// Schlaffenster, in dem das Journal schreibt.
pub const TICKS: u64 = 2000;

/// Die Nachricht eines Jobs im Korpusprogramm in Byte: Auf beiden Boards
/// rechnet `sha256` darueber laenger als einen Tick.
pub const JOB_BYTES: u64 = 4096;

/// Hoechstens so lang wird die Nachricht; sie liegt in der Arena.
pub const JOB_BYTES_MAX: u64 = 1 << 26;

/// So viele Ticks soll ein Job auf einem Wirt rechnen: Die Pflicht verlangt
/// zwei, die Messung beim Erzeugen schwankt.
pub const JOB_TICKS: u64 = 4;

/// Die Periode des Pruefprogramms in Nanosekunden (`tick = 1 ms`).
pub const TICK_NS: u64 = 1_000_000;

/// Die Zeile, die die Laenge der Nachricht traegt.
const JOB_LINE: &str = "const JOB_BYTES = ";

/// Die Zeile der fremden Last vor der Bilanz: `takt fremd fpu runden N
/// fehler M` (`takt_rt_core::port_test`).
pub const FOREIGN: &str = takt_rt_core::port_test::LINE;

/// Der Nachweis einer Probe im Kopf, die aus einer Aufgabe ohne Privileg in
/// die Arena schrieb: `verweigert`, `offen` oder `ungeprueft` dahinter
/// (`examples/zephyr`, der C-Wirt der Black Pill unter FreeRTOS-MPU).
pub const PARTITION_PROBE: &str = "takt schutz partition ";

/// Das Urteil ueber eine Ausgabe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Wie spaet die Ticks begannen.
    pub drift: Drift,
    /// Runden der fremden Last und wie viele davon falsch rechneten.
    pub foreign: Option<(u64, u64)>,
    /// Der laengste Job in Ticks von seinem Start bis zur Fertigstellung.
    pub longest_job: Option<u64>,
    /// Tiefe und Schranke des Schritt-Stacks in Byte.
    pub stack: Option<(u64, u64)>,
    /// Tiefe und Schranke des Job-Stacks in Byte.
    pub job_stack: Option<(u64, u64)>,
    /// Die Jobs rechnen auf dem Schritt-Stack (`jobstack geteilt`).
    pub job_stack_shared: bool,
    /// Schreibvorgaenge des Journals.
    pub journal_writes: Option<u64>,
    /// Der Speicherschutz, den der Kopf nennt (12.11).
    pub protection: Option<Case>,
    /// Was der Port nicht belegt hat, je Pflicht eine Zeile.
    pub failures: Vec<String>,
}

impl Verdict {
    /// Hat der Port bestanden?
    pub fn passed(&self) -> bool {
        self.failures.is_empty()
    }

    /// Das Urteil als Abschnitt `[port <name>]` des Konformitaetsberichts.
    pub fn entry(&self, name: &str, date: &str) -> PortEntry {
        PortEntry {
            name: name.to_string(),
            date: date.to_string(),
            passed: self.passed(),
            ticks: self.drift.ticks as u64,
            drift_median_ns: self.drift.median,
            drift_late_ns: self.drift.late,
            jitter_ns: self.drift.jitter,
            foreign_rounds: self.foreign.map(|f| f.0),
            foreign_errors: self.foreign.map(|f| f.1),
            longest_job_ticks: self.longest_job,
            stack: self.stack,
            job_stack: self.job_stack,
            job_stack_shared: self.job_stack_shared,
            journal_writes: self.journal_writes,
            protection: self.protection.map(|c| c.name().to_string()),
        }
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pair = |p: Option<(u64, u64)>| p.map_or_else(|| "?".to_string(), |(a, b)| format!("{a} von {b}"));
        writeln!(f, "Portpruefung: {}", if self.passed() { "bestanden" } else { "durchgefallen" })?;
        writeln!(
            f,
            "  Drift: {} Zeitzeilen, Median {} ns, p99 {} ns, spaetester {} ns darueber, Jitter {} ns",
            self.drift.ticks,
            self.drift.median,
            self.drift.p99,
            self.drift.late,
            self.drift.jitter.map_or_else(|| "?".to_string(), |j| j.to_string())
        )?;
        writeln!(
            f,
            "  fremde Last: {}; laengster Job: {} Ticks",
            self.foreign.map_or_else(|| "?".to_string(), |(r, e)| format!("{r} Runden, {e} falsch")),
            self.longest_job.map_or_else(|| "?".to_string(), |t| t.to_string())
        )?;
        writeln!(
            f,
            "  Stack {} Byte, Job-Stack {}; Journal {} Schreibvorgaenge",
            pair(self.stack),
            if self.job_stack_shared { "auf dem Schritt-Stack".to_string() } else { pair(self.job_stack) + " Byte" },
            self.journal_writes.map_or_else(|| "?".to_string(), |w| w.to_string())
        )?;
        writeln!(f, "  {}{}", protection::LABEL, self.protection.map_or("?", Case::name))?;
        for failure in &self.failures {
            writeln!(f, "  - {failure}")?;
        }
        Ok(())
    }
}

/// Das Pruefprogramm mit `job_bytes` Byte je Nachricht: ein Vielfaches
/// von 64 (die Fuellschleife schreibt je Tick ein Vierundsechzigstel)
/// zwischen [`JOB_BYTES`] und [`JOB_BYTES_MAX`].
pub fn source(job_bytes: u64) -> Result<String, String> {
    if job_bytes % 64 != 0 || !(JOB_BYTES..=JOB_BYTES_MAX).contains(&job_bytes) {
        return Err(format!(
            "die Nachricht eines Jobs hat {job_bytes} Byte; erlaubt ist ein Vielfaches von 64 \
             zwischen {JOB_BYTES} und {JOB_BYTES_MAX}"
        ));
    }
    let line = format!("{JOB_LINE}{JOB_BYTES}\n");
    if PROGRAM.matches(&line).count() != 1 {
        return Err(format!("das Korpusprogramm traegt `{}` nicht genau einmal", line.trim_end()));
    }
    Ok(PROGRAM.replacen(&line, &format!("{JOB_LINE}{job_bytes}\n"), 1))
}

/// Die Laenge der Nachricht in `text`, wenn `text` das Pruefprogramm ist:
/// Jede andere Abweichung von [`PROGRAM`] macht es zu einem anderen
/// Programm, ueber das die Portpruefung nicht urteilt.
pub fn job_bytes(text: &str) -> Result<u64, String> {
    let text = text.replace("\r\n", "\n");
    let value = text.lines().find_map(|l| l.strip_prefix(JOB_LINE)).ok_or("keine Zeile `const JOB_BYTES = …`")?;
    let n = value.trim().parse::<u64>().map_err(|e| format!("`{JOB_LINE}{value}`: {e}"))?;
    if source(n)? != text {
        return Err("das ist nicht das Pruefprogramm: Ausser `JOB_BYTES` weicht es von 124_port_test ab".into());
    }
    Ok(n)
}

/// Die Laenge der Nachricht, mit der ein Job auf einem Wirt mit
/// `bytes_per_ms` Durchsatz von `sha256` [`JOB_TICKS`] Ticks rechnet: die
/// naechste Zweierpotenz, mindestens [`JOB_BYTES`], hoechstens
/// [`JOB_BYTES_MAX`].
pub fn job_bytes_for(bytes_per_ms: u64) -> u64 {
    let ticks_ms = JOB_TICKS.saturating_mul(TICK_NS) / 1_000_000;
    let want = bytes_per_ms.saturating_mul(ticks_ms).max(JOB_BYTES);
    want.checked_next_power_of_two().unwrap_or(JOB_BYTES_MAX).min(JOB_BYTES_MAX)
}

/// Das Pruefprogramm `text`, uebersetzt wie fuer den Port: im Sim-Build,
/// damit seine Ausgaenge ohne Treiber in den Trace gehen.
pub fn program(text: &str) -> Result<takt_mir::Program, String> {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(text, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    out.program.ok_or_else(|| "kein Programm".to_string())
}

/// Urteilt ueber die Ausgabe eines Ports, der das Pruefprogramm `text`
/// gefahren hat (Modulkopf).
pub fn judge(log: &str, text: &str) -> Verdict {
    let mut v = Verdict { drift: Drift::of(log), ..Verdict::default() };
    match job_bytes(text) {
        Ok(_) => trace_agrees(log, text, &mut v.failures),
        Err(e) => v.failures.push(e),
    }
    foreign_load(log, &mut v);
    jobs_interruptible(log, &mut v);
    stacks(log, &mut v);
    on_time(log, &mut v.failures);
    no_faults(log, &mut v.failures);
    journal(log, &mut v);
    protected(log, &mut v);
    v
}

/// Trace vollstaendig und gleich dem Interpreter.
fn trace_agrees(log: &str, text: &str, failures: &mut Vec<String>) {
    if let Err(e) = board::balanced(log, &Options::timed(TICKS)) {
        failures.push(e);
        return;
    }
    match board::reached(log) {
        Some(last) if last >= TICKS => {}
        last => {
            let last = last.map_or_else(|| "keinem".to_string(), |t| t.to_string());
            failures.push(format!("Trace unvollstaendig: er reicht bis Tick {last}, der Lauf hat {TICKS} Ticks"));
            return;
        }
    }
    let expected = match interpreted(log, text) {
        Ok(t) => t,
        Err(e) => {
            failures.push(format!("der Interpreter rechnete den Lauf nicht nach: {e}"));
            return;
        }
    };
    let diffs = crate::compare(&expected, log);
    if let Some(first) = diffs.first() {
        failures.push(format!(
            "der Trace weicht {} Mal vom Interpreter ab, zuerst in Tick {} bei `{}`: Interpreter {}, Port {}",
            diffs.len(),
            first.tick,
            first.output,
            first.interpreter,
            first.native
        ));
    }
}

/// Der Soll-Trace: der Interpreter mit den Job-Zeilen des Ports (12.5).
fn interpreted(log: &str, text: &str) -> Result<String, String> {
    let p = program(text)?;
    let stimulus = takt_interp::Trace::parse(&stimulus(log)).map_err(|e| e.to_string())?;
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    let trace = takt_interp::run(&p, &stimulus, &options).map(|r| r.trace.render()).map_err(|e| format!("{e:?}"))?;
    // Der Rahmen schreibt ein `f32` als `f64` (`crate::run::widen_f32`).
    Ok(crate::run::widen_f32(&trace, &crate::run::f32_outputs(&p)))
}

/// Was der Interpreter nachspielt: die Eingaben, die der Port aufzeichnete
/// (`crate::run::recorded_inputs`), und die Runtime-Faults, die er erhob
/// (grammar/trace.md: `runtime` ist Stimulus). Jeder Fault ist ein Befund
/// ([`no_faults`]); nachgespielt bleibt der Trace danach vergleichbar.
fn stimulus(log: &str) -> String {
    let recorded = crate::run::recorded_inputs(log);
    let tick = |l: &str| l.strip_prefix("t=").and_then(|r| r.split_once(' ')).and_then(|(k, _)| k.parse::<u64>().ok());
    let mut lines: Vec<&str> = recorded.lines().chain(runtime_lines(log)).collect();
    lines.sort_by_key(|l| tick(l));
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Die Zeilen `runtime` einer Ausgabe.
fn runtime_lines(log: &str) -> impl Iterator<Item = &str> {
    log.lines().map(str::trim_end).filter(|l| l.split_whitespace().nth(1) == Some("runtime"))
}

/// Die fremde Last lief neben dem Lauf und rechnete nie falsch.
fn foreign_load(log: &str, v: &mut Verdict) {
    let line = log.lines().rev().find_map(|l| l.trim_end().strip_prefix(FOREIGN));
    let parsed = line.and_then(|rest| {
        let mut words = rest.split_whitespace();
        let rounds = words.next()?.parse().ok()?;
        let errors = words.skip_while(|w| *w != takt_rt_core::port_test::ERRORS.trim()).nth(1)?.parse().ok()?;
        Some((rounds, errors))
    });
    v.foreign = parsed;
    match parsed {
        None => v.failures.push(format!("FPU-Kontext nicht entscheidbar: die Zeile `{}` fehlt", FOREIGN.trim_end())),
        Some((_, errors)) if errors > 0 => {
            v.failures.push(format!("FPU-Kontext nicht gesichert: die fremde Last rechnete {errors} Mal falsch"));
        }
        Some((rounds, _)) if rounds < TICKS => v.failures.push(format!(
            "FPU-Kontext nicht entscheidbar: die fremde Last rechnete {rounds} Runden in {TICKS} Ticks, \
             weniger als eine je Tick"
        )),
        Some(_) => {}
    }
}

/// Ein Job lief ueber eine Tickgrenze: Seine Fertigstellung erscheint
/// mindestens zwei Ticks nach dem Start, obwohl `duration` einen nennt
/// (4.5). Dass der Tick dabei puenktlich blieb, belegt [`on_time`].
fn jobs_interruptible(log: &str, v: &mut Verdict) {
    v.longest_job = log
        .lines()
        .filter_map(|l| {
            let (tick, rest) = l.strip_prefix("t=")?.split_once(' ')?;
            let words: Vec<&str> = rest.split_whitespace().collect();
            let ["job", _, _, "done", start] = words.as_slice() else { return None };
            let start: u64 = start.strip_prefix("start=")?.parse().ok()?;
            tick.parse::<u64>().ok()?.checked_sub(start)
        })
        .max();
    match v.longest_job {
        Some(ticks) if ticks >= 2 => {}
        Some(_) => v.failures.push(
            "Jobs unterbrechbar nicht entscheidbar: jeder Job war im Tick nach seinem Start fertig, keiner lief \
             ueber eine Tickgrenze"
                .to_string(),
        ),
        None => v.failures.push("Jobs unterbrechbar nicht entscheidbar: kein Job wurde fertig".to_string()),
    }
}

/// Beide Stacks gemessen und unter ihrer Schranke (12.3); rechnen die Jobs
/// auf dem Schritt-Stack, sagt die Bilanz es.
fn stacks(log: &str, v: &mut Verdict) {
    let pair = |depth: &str, bound: &str| Some((board::counter(log, depth)?, board::counter(log, bound)?));
    v.stack = pair("stack", "schranke");
    v.job_stack = pair("jobstack", "jobschranke");
    v.job_stack_shared = log
        .lines()
        .rev()
        .find(|l| l.starts_with("takt schlief "))
        .is_some_and(|l| l.split_whitespace().collect::<Vec<_>>().windows(2).any(|w| w == ["jobstack", "geteilt"]));
    if v.stack.is_none() {
        v.failures.push("die Bilanz nennt Tiefe und Schranke des Schritt-Stacks nicht (12.3)".to_string());
    }
    if v.job_stack.is_none() && !v.job_stack_shared {
        v.failures.push(
            "die Bilanz nennt Tiefe und Schranke des Job-Stacks nicht und sagt nicht `jobstack geteilt` (12.3)"
                .to_string(),
        );
    }
    if let Err(e) = board::within_stacks(log) {
        v.failures.push(e);
    }
}

/// Kein Tick kam zu spaet (7.3).
fn on_time(log: &str, failures: &mut Vec<String>) {
    for (label, what) in
        [("ueberlaeufe", "Ticks ueberschritten ihre Periode"), ("verspaetet", "Ticks begannen eine Periode zu spaet")]
    {
        match board::counter(log, label) {
            Some(0) => {}
            Some(n) => failures.push(format!("{n} {what} (`{label}`, 7.3)")),
            None => failures.push(format!("die Bilanz nennt `{label}` nicht")),
        }
    }
}

/// Der Port erhob keinen Runtime-Fault; jeder nennt seine Art und ihren
/// Abschnitt.
fn no_faults(log: &str, failures: &mut Vec<String>) {
    let faults: Vec<&str> = runtime_lines(log).collect();
    let Some(first) = faults.first() else { return };
    let section = |kind: &str| match kind {
        "Overrun" => "7.3",
        "Hardware" => "12.3",
        "Driver" => "12.6",
        _ => "5.3",
    };
    let mut kinds: Vec<&str> = faults.iter().filter_map(|l| l.split_whitespace().nth(2)).collect();
    kinds.sort_unstable();
    kinds.dedup();
    let named: Vec<String> = kinds.iter().map(|k| format!("`{k}` ({})", section(k))).collect();
    failures.push(format!("{} Runtime-Faults, zuerst `{first}`: {}", faults.len(), named.join(", ")));
}

/// Der Kopf nennt den Speicherschutz (12.11): eine Kopfzeile `Schutz: …`
/// vor der Marke mit einem der drei Faelle, die letzte gilt. Ein Nachweis
/// darunter darf ihr nicht widersprechen: Eine Probe, die die Partition
/// abwies, verlangt `Partition`; eine, die in die Arena schrieb, schliesst
/// es aus. Ein Schutz, der nur behauptet ist, waere schlechter als keiner.
fn protected(log: &str, v: &mut Verdict) {
    let header = log.rfind(board::MARK).map_or(log, |at| &log[..at]);
    let Some(name) = header.lines().rev().find_map(|l| l.trim().strip_prefix(protection::LABEL)) else {
        v.failures.push(format!("der Kopf nennt den Speicherschutz nicht (`{}…`, 12.11)", protection::LABEL));
        return;
    };
    let Some(case) = Case::from_name(name) else {
        let known: Vec<&str> = Case::ALL.iter().map(|c| c.name()).collect();
        v.failures
            .push(format!("`{}{name}` ist kein Fall des Speicherschutzes, nur {known:?} (12.11)", protection::LABEL));
        return;
    };
    v.protection = Some(case);
    let probe = header.lines().rev().find_map(|l| l.trim_end().strip_prefix(PARTITION_PROBE));
    match (case, probe) {
        (Case::Partition, Some("offen")) => v.failures.push(format!(
            "der Kopf nennt `{}{}`, doch die Probe schrieb in die Arena (`{PARTITION_PROBE}offen`)",
            protection::LABEL,
            case.name()
        )),
        (Case::Window | Case::Unprotected, Some("verweigert")) => v.failures.push(format!(
            "die Partition wies die Probe ab (`{PARTITION_PROBE}verweigert`), doch der Kopf nennt `{}{}`",
            protection::LABEL,
            case.name()
        )),
        _ => {}
    }
}

/// Das Journal schrieb und scheiterte nie (5.9, 12.3).
fn journal(log: &str, v: &mut Verdict) {
    v.journal_writes = board::counter(log, "journal geschrieben");
    match v.journal_writes {
        Some(n) if n >= 1 => {}
        Some(_) => v.failures.push("das Journal schrieb nie, auch nicht im Schlaffenster (12.3)".to_string()),
        None => v.failures.push("die Bilanz nennt `journal geschrieben` nicht".to_string()),
    }
    if let Some(n) = board::counter(log, "fehlgeschlagen").filter(|n| *n > 0) {
        v.failures.push(format!("{n} Schreibvorgaenge des Journals schlugen fehl"));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::*;

    /// Die Bilanz eines Ports, der jede Pflicht erfuellt, mit `gesendet`
    /// fuer `sent` Byte.
    fn summary(sent: usize) -> String {
        format!(
            "takt schlief 0 ueberlaeufe 0 verspaetet 0 verloren 0 rueckstand 5000 ns verworfen 0 gesendet {sent} \
             journal geschrieben 1 fehlgeschlagen 0 flush 0 nvm loeschen 0 ns programmieren 0 ns stack 3900 \
             schranke 4500 programm 300 jobstack 1100 jobschranke 2700\n"
        )
    }

    /// Ein Lauf, wie ihn ein Port schreibt, der jede Pflicht erfuellt: der
    /// Interpreter, dessen 20 Jobs je drei Ticks nach ihrem Start fertig
    /// werden, mit einer Zeitzeile je Tick, der Zeile der fremden Last und der
    /// Bilanz. `FILL` braucht 64 Ticks, und jeder Job startet elf Ticks nach
    /// der Fertigstellung seines Vorgaengers (`wait 10 ms`).
    fn port_log() -> &'static str {
        static LOG: OnceLock<String> = OnceLock::new();
        LOG.get_or_init(|| {
            let mut stimulus = String::new();
            let mut start = 63;
            for _ in 0..20 {
                stimulus.push_str(&format!("t={} job hasher v done start={start}\n", start + 3));
                start += 14;
            }
            let stimulus = takt_interp::Trace::parse(&stimulus).expect("Stimulus");
            let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
            let p = program(PROGRAM).expect("Pruefprogramm");
            let trace = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
            // Wie der Rahmen: ein `f32` als `f64`.
            let trace = crate::run::widen_f32(&trace, &crate::run::f32_outputs(&p));
            let tick_of = |l: &str| l.strip_prefix("t=")?.split_once(' ')?.0.parse::<u64>().ok();
            let mut lines = trace.lines().peekable();
            let mut body = String::new();
            // Die Zeitzeile steht am Ende ihres Ticks, ab Tick 1 (FB-687).
            for k in 0..=TICKS {
                while let Some(l) = lines.next_if(|l| tick_of(l) == Some(k)) {
                    body.push_str(l);
                    body.push('\n');
                }
                if k > 0 {
                    body.push_str(&format!("t={k} time took=40000 drift=3000 slept=0\n"));
                }
            }
            body.push_str(&format!("{FOREIGN}40000 fehler 0\n"));
            format!("takt auf wirt\r\n  Schutz: keiner\r\n{}{body}{}takt end\n", board::MARK, summary(body.len()))
        })
    }

    /// **Ein Port, der jede Pflicht erfuellt, besteht**, und das Urteil
    /// nennt, was es gemessen hat.
    #[test]
    fn a_port_that_keeps_every_duty_passes() {
        let log = port_log();
        assert!(log.contains("t=66 job hasher v done start=63\n"), "die Jobs enden drei Ticks nach dem Start");
        let v = judge(log, PROGRAM);
        assert!(v.passed(), "{v}");
        assert_eq!(v.longest_job, Some(3));
        assert_eq!(v.foreign, Some((40_000, 0)));
        assert_eq!((v.stack, v.job_stack, v.journal_writes), (Some((3900, 4500)), Some((1100, 2700)), Some(1)));
        assert_eq!((v.drift.median, v.drift.jitter), (3000, Some(0)));
        assert_eq!(v.protection, Some(Case::Unprotected));
        let entry = v.entry("interrupt", "2026-10-11");
        assert!(entry.passed && entry.ticks == TICKS - 2, "{entry:?}");
        assert_eq!(entry.protection.as_deref(), Some("keiner"));
    }

    /// **Der Kopf nennt den Speicherschutz, und kein Nachweis widerspricht**
    /// (12.11): ohne Kopfzeile, mit einem unbekannten Fall oder gegen die
    /// Probe der Partition faellt der Port durch; die letzte Kopfzeile vor
    /// der Marke gilt.
    #[test]
    fn the_header_names_the_protection_and_no_probe_contradicts_it() {
        let judged = |header: &str| {
            let mut v = Verdict::default();
            protected(&format!("takt auf wirt\n{header}{}t=0 time\n", board::MARK), &mut v);
            v
        };
        for case in Case::ALL {
            let v = judged(&format!("{}\r\n", case.line()));
            assert!(v.passed() && v.protection == Some(case), "{case:?}: {v}");
        }
        let missing = judged("");
        assert!(missing.failures.iter().any(|f| f.contains("nicht")) && missing.protection.is_none(), "{missing}");
        let unknown = judged("  Schutz: teilweise\n");
        assert!(unknown.failures.iter().any(|f| f.contains("teilweise")), "{unknown}");
        let refused = judged("  Schutz: Partition\ntakt schutz partition verweigert\n");
        assert!(refused.passed(), "{refused}");
        let open = judged("  Schutz: Partition\ntakt schutz partition offen\n");
        assert!(open.failures.iter().any(|f| f.contains("offen")), "{open}");
        let understated = judged("  Schutz: keiner\ntakt schutz partition verweigert\n");
        assert!(understated.failures.iter().any(|f| f.contains("verweigert")), "{understated}");
        let unchecked = judged("  Schutz: Partition\ntakt schutz partition ungeprueft\n");
        assert!(unchecked.passed(), "{unchecked}");
        let last = judged("  Schutz: keiner\n\n  Schutz: Fenster\n");
        assert_eq!(last.protection, Some(Case::Window), "{last}");
    }

    /// **Ein Trace, der vom Interpreter abweicht, faellt durch**, auch wenn
    /// alles andere stimmt.
    #[test]
    fn a_trace_unlike_the_interpreter_fails() {
        let log = port_log();
        let at = log.find(" out hashes 2\n").expect("zweiter Job");
        let wrong = format!("{} out hashes 7\n{}", &log[..at], &log[at + " out hashes 2\n".len()..]);
        let v = judge(&wrong, PROGRAM);
        assert!(v.failures.iter().any(|f| f.contains("weicht") && f.contains("hashes")), "{v}");
    }

    /// **Rechnet die fremde Last falsch, sichert der Port die FPU-Register
    /// nicht** (12.11).
    #[test]
    fn a_wrong_foreign_round_means_the_fpu_was_not_saved() {
        let mut v = Verdict::default();
        foreign_load(&format!("{FOREIGN}40000 fehler 17\n"), &mut v);
        assert_eq!(v.foreign, Some((40_000, 17)));
        assert!(v.failures.iter().any(|f| f.contains("nicht gesichert") && f.contains("17")), "{v}");
        let mut idle = Verdict::default();
        foreign_load(&format!("{FOREIGN}12 fehler 0\n"), &mut idle);
        assert!(idle.failures.iter().any(|f| f.contains("weniger als eine je Tick")), "{idle}");
        let mut silent = Verdict::default();
        foreign_load("takt end\n", &mut silent);
        assert!(silent.failures.iter().any(|f| f.contains("fehlt")), "{silent}");
    }

    /// **Ein Port, der den Schritt hinter den Jobs warten laesst**, verspaetet
    /// den Tick, und kein Job ist im Tick nach seinem Start noch offen.
    #[test]
    fn a_step_that_waits_for_the_job_fails() {
        let mut v = Verdict::default();
        jobs_interruptible("t=16 job hasher v done start=15\nt=18 job hasher v done start=17\n", &mut v);
        assert!(v.failures.iter().any(|f| f.contains("Tickgrenze")), "{v}");
        let mut late = Vec::new();
        on_time(&summary(0).replace("verspaetet 0 verloren 0", "verspaetet 12 verloren 31"), &mut late);
        assert!(late.iter().any(|f| f.contains("12 Ticks begannen")), "{late:?}");
        let mut none = Verdict::default();
        jobs_interruptible("t=40 out hashes 0\n", &mut none);
        assert!(none.failures.iter().any(|f| f.contains("kein Job")), "{none}");
    }

    /// **`verloren` ist kein eigener Befund**: Er folgt aus `verspaetet`
    /// (FB-519), und das Urteil nennt den verspaeteten Tick (7.3).
    #[test]
    fn jitter_alone_loses_no_tick() {
        let mut quiet = Vec::new();
        on_time(&summary(0).replace("verloren 0", "verloren 3"), &mut quiet);
        assert!(quiet.is_empty(), "{quiet:?}");
    }

    /// **Ein Runtime-Fault ist ein Befund**, und der Interpreter spielt ihn
    /// nach (12.5): Das Urteil nennt seine Art mit ihrem Abschnitt.
    #[test]
    fn a_runtime_fault_is_named_and_replayed() {
        let log = "t=0 out hashes 0\nt=17 runtime Overrun\nt=18 job hasher v done start=15\nt=19 runtime Overrun\n";
        let mut failures = Vec::new();
        no_faults(log, &mut failures);
        assert_eq!(failures, ["2 Runtime-Faults, zuerst `t=17 runtime Overrun`: `Overrun` (7.3)"]);
        assert_eq!(
            stimulus(log),
            "t=17 runtime Overrun\nt=18 job hasher v done start=15\nt=19 runtime Overrun\n",
            "in Tickordnung"
        );
    }

    /// **Eine Zahl, die fehlt, belegt nichts**: ohne Job-Stack, ohne
    /// Ueberlaeufe, ohne Schreibvorgang faellt der Port durch.
    #[test]
    fn a_missing_number_is_not_a_pass() {
        let mut v = Verdict::default();
        stacks(&summary(0).replace(" jobstack 1100 jobschranke 2700", ""), &mut v);
        assert!(v.failures.iter().any(|f| f.contains("Job-Stacks")), "{v}");
        let mut shared = Verdict::default();
        stacks(&summary(0).replace("jobstack 1100 jobschranke 2700", "jobstack geteilt"), &mut shared);
        assert!(shared.passed() && shared.job_stack_shared, "{shared}");
        let mut deep = Verdict::default();
        stacks(&summary(0).replace("stack 3900", "stack 4600"), &mut deep);
        assert!(deep.failures.iter().any(|f| f.contains("4600")), "{deep}");
        let mut quiet = Vec::new();
        on_time("takt schlief 0 gesendet 0\n", &mut quiet);
        assert_eq!(quiet.len(), 2, "{quiet:?}");
        let mut unwritten = Verdict::default();
        journal(&summary(0).replace("geschrieben 1", "geschrieben 0"), &mut unwritten);
        assert!(unwritten.failures.iter().any(|f| f.contains("schrieb nie")), "{unwritten}");
    }

    /// Das Pruefprogramm ist das des Korpus und uebersetzt.
    #[test]
    fn the_program_is_the_corpus_program() {
        let path = crate::board::corpus_path(CORPUS_NAME);
        assert_eq!(std::fs::read_to_string(path).expect("Korpus"), PROGRAM);
        program(PROGRAM).expect("uebersetzt");
        assert_eq!(job_bytes(PROGRAM), Ok(JOB_BYTES));
    }

    #[test]
    fn the_length_of_a_message_goes_in_and_comes_back() {
        let text = source(1 << 20).expect("erlaubt");
        assert_eq!(job_bytes(&text), Ok(1 << 20));
        program(&text).expect("uebersetzt");
        assert!(source(4100).is_err(), "kein Vielfaches von 64");
        assert!(source(2048).is_err(), "unter der Laenge der Boards");
        assert!(source(JOB_BYTES_MAX * 2).is_err(), "ueber der Grenze");
    }

    #[test]
    fn another_program_is_not_the_test_program() {
        let changed = PROGRAM.replacen("wait 10 ms", "wait 20 ms", 1);
        assert_ne!(changed, PROGRAM);
        assert!(job_bytes(&changed).is_err());
        let v = judge("", &changed);
        assert!(v.failures.iter().any(|f| f.contains("nicht das Pruefprogramm")), "{v}");
    }

    #[test]
    fn a_fast_host_gets_a_longer_message() {
        assert_eq!(job_bytes_for(0), JOB_BYTES, "ein Board ohne Messung");
        assert_eq!(job_bytes_for(500), JOB_BYTES, "2 KiB in vier Ticks reichen nicht an die Boards heran");
        // 500 MB/s: 500 KiB je Millisekunde, vier Ticks sind 2 MB, die
        // naechste Zweierpotenz 2 MiB.
        assert_eq!(job_bytes_for(500_000), 1 << 21);
        assert_eq!(job_bytes_for(u64::MAX), JOB_BYTES_MAX);
    }

    /// **Eine lange Nachricht fuellt sich ohne Fault** (M11 Schritt 18c): Auf
    /// einem Wirt zaehlt `i` im Fuellschritt ueber 255, und `i as u8` war
    /// dort ein `RangeFault` in Tick 0 — der Interpreter fiel mit, also
    /// stimmte der Trace, und kein Job begann.
    #[test]
    fn a_long_message_fills_without_a_fault() {
        let p = program(&source(1 << 17).expect("Quelle")).expect("uebersetzt");
        let options = takt_interp::RunOptions { ticks: 70, ..Default::default() };
        let trace = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
        assert!(!trace.contains(" fault "), "{trace}");
    }
}
