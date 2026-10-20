//! Ein Board vom Host aus: bauen, schreiben, starten, den Trace lesen
//! (13.8; plan/m10.md 2.1).
//!
//! Die Board-Suite, `takt bench` und `takt driver-test` tun dasselbe: ein
//! Takt-Programm in das Bring-up eines Boards bauen, das Abbild schreiben,
//! starten und den Trace bis `takt end` lesen. Hier steht das einmal, mit
//! dem, was die Board-Woche gelehrt hat: eine harte Frist fuer jeden
//! fremden Prozess und jedes Lesen (FB-266), ein Zwischenspeicher fuer
//! Abbilder (FB-270) und je Board ein Weg zurueck, der keine Hand braucht.
//!
//! | Board | Schreiben und Starten | Zurueck, wenn es nicht antwortet |
//! |---|---|---|
//! | ESP32-C6 ([`esp32c6`]) | `probe-rs` ueber USB-Serial-JTAG | Chip-Reset auf RTC-Ebene, ueber JTAG oder Konsole (FB-264, FB-266) |
//! | STM32F401 ([`stm32f401`]) | DfuSe ueber den Bootloader im ROM ([`dfuse`]), zurueckgelesen | `TAKT` auf der Trace-Leitung: Die Anwendung merkt sich den Wunsch, das Sprungbrett springt in den Bootloader (FB-275) |
//! | Wirt ([`host`]) | ein Prozess, der MCU-Rahmen in logischer Zeit | entfaellt: Der Prozess endet |
//!
//! Ein Board traegt Bring-ups in Rust und C-Wirte ([`c_host`], M11 Schritt
//! 18b): Geschrieben, gestartet und gelesen wird jedes Abbild gleich.

pub mod c_host;
pub mod dfuse;
pub mod esp32c6;
pub mod host;
pub mod stm32f401;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Der Korpus der Boards: was das Manifest der Suite `board` gibt
/// ([`crate::suites`]), dazu die Beispiele ([`crate::suites::EXAMPLES`]).
pub fn corpus() -> Vec<&'static str> {
    let mut out = crate::suites::programs("board");
    out.extend(crate::suites::EXAMPLES);
    out
}

/// Die Zeile, mit der jedes Bring-up seinen Lauf beendet.
pub const END: &str = "takt end";

/// Die Zeile, ab der das Board zaehlt, was es sendet (FB-304).
pub const MARK: &str = "takt trace\r\n";

/// Ein Trace mit Luecken ist keiner: Jede Zeile hinter der Luecke waere
/// eine Abweichung. Bytes fehlen, wenn die Telemetrie des Boards sie
/// verwirft (`verworfen N`, 12.2, FB-292) oder wenn sie auf dem Weg zum
/// Wirt verloren gehen (FB-304). Fuer den zweiten Fall nennt die
/// Abschlusszeile, wie viele Bytes das Board ab [`MARK`] sandte
/// (`gesendet N`), und ebenso viele muessen bis zu ihr angekommen sein.
///
/// Ohne Marke und Bilanz (Messkern, Natives) gibt es nichts zu zaehlen; ein
/// Lauf des Programms ([`Bin::Takt`]) braucht beide, und sein Trace muss bis
/// zum letzten Tick reichen, wenn das Programm nicht selbst endet (12.7):
/// Ein Bring-up, das zu frueh `takt end` schreibt, liefert sonst einen
/// kurzen Trace, und [`crate::compare`] saehe nur, was der Interpreter
/// danach noch aendert.
///
/// Verwirft es einen Lauf, legt es dessen Text ab und nennt die Datei
/// ([`kept`]): Der Befund steht im Text, nicht in der Meldung.
pub(crate) fn complete(text: String, options: &Options) -> Result<String, String> {
    if options.judged {
        return Ok(text);
    }
    let stacks = if options.calibrating { Ok(()) } else { within_stacks(&text) };
    match stacks.and_then(|()| balanced(&text, options)) {
        Ok(()) => Ok(text),
        Err(e) => Err(format!(
            "{e}
{}",
            kept(&text)
        )),
    }
}

/// Legt den Text eines verworfenen Laufs unter `takt-board-failed` im
/// Zielverzeichnis ab und nennt die Datei.
fn kept(text: &str) -> String {
    let dir = crate::target_dir().join("takt-board-failed");
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let path = dir.join(format!("{stamp}-{}.log", std::process::id()));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)) {
        Ok(()) => format!("der Lauf: {}", path.display()),
        Err(e) => format!("der Lauf liess sich nicht ablegen ({}): {e}", path.display()),
    }
}

/// Was [`complete`] ohne die Stacks verlangt: keine verworfenen Bytes — ausser
/// in einem Messlauf, der nur seine Bilanz belegt ([`Options::lossy`]) —, die
/// Bilanz ab [`MARK`] und ein Trace bis zum letzten Tick.
pub(crate) fn balanced(text: &str, options: &Options) -> Result<(), String> {
    let summary = text.rfind("takt schlief ");
    if let Some(n) = counter(text, "verworfen").filter(|n| *n > 0 && !options.lossy) {
        // Hielt der Adapter die Leitung an, las der Wirt nicht mit (FB-530);
        // sonst brachte das Board mehr, als Ring und Leitung fassen (FB-741).
        let why = match counter(text, "angehalten").filter(|h| *h > 0) {
            Some(h) => format!(", die Leitung stand {h}-mal (XOFF des Wirts, FB-530)"),
            None => String::new(),
        };
        return Err(format!("Trace unvollstaendig: das Board verwarf {n} Byte (FB-292){why}"));
    }
    let mark = text[..summary.unwrap_or(text.len())].rfind(MARK);
    match (mark, summary.zip(counter(text, "gesendet"))) {
        (None, None) if options.bin != Bin::Takt => return Ok(()),
        (None, None) => return Err("Trace unvollstaendig: ohne `takt trace` und Bilanz (KON1-010)".into()),
        (Some(m), Some((s, sent))) => {
            let arrived = s - (m + MARK.len());
            if u64::try_from(arrived).is_ok_and(|a| a != sent) {
                return Err(format!(
                    "Trace unvollstaendig: das Board sandte {sent} Byte, angekommen sind {arrived} (FB-304)"
                ));
            }
        }
        (Some(_), None) => return Err("Trace unvollstaendig: nach `takt trace` fehlt die Bilanz (FB-304)".into()),
        (None, Some(_)) => return Err("Trace unvollstaendig: vor der Bilanz fehlt `takt trace` (FB-304)".into()),
    }
    let ended = text
        .lines()
        .any(|l| l.strip_prefix("t=").and_then(|r| r.split_once(' ')).is_some_and(|(_, r)| r.starts_with("end ")));
    let checked = options.bin == Bin::Takt && !options.timed && options.ticks > 0 && !ended;
    match reached(text) {
        _ if !checked => Ok(()),
        Some(last) if last >= options.ticks => Ok(()),
        last => Err(format!(
            "Trace unvollstaendig: er reicht bis Tick {}, der Lauf hat {} Ticks (KON1-010)",
            last.map_or_else(|| "keinem".to_string(), |t| t.to_string()),
            options.ticks
        )),
    }
}

/// **Jeder Stack bleibt unter seiner Schranke** (12.3): Die Bilanz nennt
/// die Tiefe des Schritt- und des Job-Stacks unter Last (Painting) und
/// `TICK_STACK_BYTES` und `JOB_STACK_BYTES` des Programms. Reicht ein Stack
/// darueber, war die Schranke falsch — die Reserve des Ports, die Marge oder
/// der Programmanteil, den die Bilanz mitnennt —, und der Waechter darunter
/// ist die letzte Linie.
pub(crate) fn within_stacks(text: &str) -> Result<(), String> {
    for (depth, bound, what, part) in [
        ("stack", "schranke", "TICK_STACK_BYTES", Some("programm")),
        ("jobstack", "jobschranke", "JOB_STACK_BYTES", None),
    ] {
        if let (Some(depth), Some(bound)) = (counter(text, depth), counter(text, bound))
            && depth > bound
        {
            let part =
                part.and_then(|p| counter(text, p)).map_or_else(String::new, |p| format!(", davon Programm {p}"));
            return Err(format!("der Stack reichte {depth} Byte tief, `{what}` ist {bound}{part} (12.3)"));
        }
    }
    Ok(())
}

/// Die Zahl hinter `label` in der letzten Bilanzzeile eines Laufs (`takt
/// schlief 0 ueberlaeufe 0 … journal geschrieben 3 …`, geschrieben von
/// `takt_rt_baremetal::report`); `label` kann aus mehreren Woertern bestehen.
pub fn counter(text: &str, label: &str) -> Option<u64> {
    let line = text.lines().rev().find(|l| l.starts_with("takt schlief "))?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let label: Vec<&str> = label.split_whitespace().collect();
    let at = words.windows(label.len()).position(|w| w == label.as_slice())?;
    words.get(at + label.len())?.parse().ok()
}

/// Der letzte Tick des Traces, bis zu dem ein Konformitaetslauf reicht. Die
/// Zeitzeile `t=<k> time ... slept=<n>` deckt die Ticks `k` bis `k + n`; das Board
/// schreibt sie hoechstens je Millisekunde, unter einer Millisekunde Tick
/// also nur jeden `m`-ten Tick (FB-271). Dieses `m` teilt jedes `k`, der
/// groesste gemeinsame Teiler der Zeilen ist darum eine obere Schranke fuer
/// die Luecke nach der letzten.
pub(crate) fn reached(text: &str) -> Option<u64> {
    let times: Vec<(u64, u64)> = text
        .lines()
        .filter_map(|l| {
            let (k, rest) = l.trim_end().strip_prefix("t=")?.split_once(" time ")?;
            let slept = rest.split_whitespace().find_map(|w| w.strip_prefix("slept="))?.parse().ok()?;
            Some((k.parse().ok()?, slept))
        })
        .collect();
    let gcd = |mut a: u64, mut b: u64| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let step = times.iter().map(|(k, _)| *k).fold(0, gcd).max(1);
    times.iter().map(|(k, slept)| k + slept + step - 1).max()
}

/// Wie spaet die Ticks eines Laufs in Echtzeit nach ihrer Grenze begannen
/// (`drift` der Zeitzeilen, 7.3); die ersten beiden laufen noch an.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Zeitzeilen.
    pub ticks: usize,
    /// Median in ns.
    pub median: i64,
    /// Der spaeteste Tick, in ns ueber dem Median.
    pub late: i64,
    /// Die fuenf spaetesten Ticks mit ihrer Nummer und Drift: Ihr Abstand
    /// verraet, was sie aufhielt.
    pub latest: Vec<(u64, i64)>,
    /// Um wie viel der Abstand zweier Tickbeginne die Periode hoechstens
    /// ueberschritt, gerechnet wie `takt bench` (13.8).
    pub jitter: Option<i64>,
}

impl Drift {
    /// Aus dem Trace eines Laufs in Echtzeit.
    pub fn of(text: &str) -> Drift {
        let mut drifts: Vec<(u64, i64)> = text
            .lines()
            .filter_map(|l| {
                let tick = l.strip_prefix("t=")?.split_whitespace().next()?.parse().ok()?;
                Some((tick, l.split_whitespace().find_map(|w| w.strip_prefix("drift="))?.parse().ok()?))
            })
            .skip(2)
            .collect();
        drifts.sort_unstable_by_key(|&(_, d)| d);
        let median = drifts.get(drifts.len() / 2).map_or(0, |&(_, d)| d);
        let late = drifts.last().map_or(0, |&(_, d)| d - median);
        let latest = drifts.iter().rev().take(5).copied().collect();
        Drift { ticks: drifts.len(), median, late, latest, jitter: crate::bench::tick_jitter(text) }
    }
}

/// Ein Verstoss gegen den Treibervertrag im Trace eines Laufs (12.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    /// Die Zeile der Tabelle in 12.6.
    pub row: u8,
    /// Die Zeile des Trace, die ihn zeigt.
    pub line: String,
}

/// Die Verstoesse gegen den Treibervertrag, die ein Trace zeigt (12.6,
/// `grammar/trace.md`): `warped` und `degraded window` (Zeile 1), jede
/// andere Verletzung, die `degraded` nennt (Zeile 2), ein Strom, dessen
/// `malformed` steigt (Zeile 5), `runtime Driver` (Zeile 6) und `runtime
/// Hardware` (Zeile 7). `recovered` ist keiner. Die Zeilen 3 und 4 urteilen
/// ueber den Wert, nicht ueber den Treiber: Sie zeigen sich in der
/// Qualitaet, die das Programm liest.
pub fn violations(trace: &str) -> Vec<Violation> {
    let mut malformed: Vec<(&str, u64)> = Vec::new();
    let mut found = Vec::new();
    for line in trace.lines().map(str::trim_end) {
        let Some((_, event)) = line.strip_prefix("t=").and_then(|r| r.split_once(' ')) else { continue };
        let words: Vec<&str> = event.split_whitespace().collect();
        let row = match words.as_slice() {
            ["driver", _, "warped", ..] | ["driver", _, "degraded", "window"] => Some(1),
            ["driver", _, "degraded", ..] => Some(2),
            ["runtime", "Driver", ..] => Some(6),
            ["runtime", "Hardware"] => Some(7),
            ["stream", name, .., count] => {
                let n = count.strip_prefix("malformed=").and_then(|n| n.parse::<u64>().ok()).unwrap_or(0);
                let before = match malformed.iter_mut().find(|(s, _)| s == name) {
                    Some((_, m)) => std::mem::replace(m, n),
                    None => {
                        malformed.push((name, n));
                        0
                    }
                };
                (n > before).then_some(5)
            }
            _ => None,
        };
        if let Some(row) = row {
            found.push(Violation { row, line: line.to_string() });
        }
    }
    found
}

/// Welches Programm des Bring-ups das Takt-Programm bindet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Bin {
    /// `takt`: das Programm unter der Tickschleife, mit Trace.
    #[default]
    Takt,
    /// `bench`: das Messprogramm von `takt bench` (13.8), ein Teil davon,
    /// gebaut mit dem Merkmal `bench`; das Takt-Programm bleibt ungenutzt.
    Bench {
        /// Um wie viele Byte das Programm im Flash verschoben liegt (FB-367).
        shift: u32,
        /// Welcher Teil des Messprogramms (FB-740).
        part: crate::bench::Part,
    },
    /// `natives`: die Vektoren der kuratierten Natives mit dem
    /// Stack-Bedarf je Aufruf (13.8); das Takt-Programm bleibt ungenutzt.
    Natives,
    /// `leftover`: ein Vorgaenger, nach dem das Sprungbrett dem naechsten
    /// Abbild eine Region der MPU hinterlaesst (FB-752); nur auf dem F401,
    /// das Takt-Programm bleibt ungenutzt.
    Leftover,
}

/// Die Form, in der das Bring-up den Kern ruft (12.11); jede ausser dem
/// eigenen Kern rechnet im Profil `shared` (12.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Form {
    /// Die Runtime wartet selbst auf die Frist (12.3).
    #[default]
    Own,
    /// Die ISR eines Alarms auf die Frist rechnet den Schritt, darunter
    /// laeuft eine fremde Hauptschleife.
    Interrupt,
    /// Die Hauptschleife des Wirts ruft `service`, sobald die Frist
    /// erreicht ist; ihre laengste Runde nennt die Hardware-Konfiguration.
    Poll,
    /// Takt als hoechstpriore Aufgabe unter dem RTOS des Boards, die der
    /// Alarm zur Frist weckt, mit Treiber-Aufgabe und Funk-ISR als Last; auf
    /// dem F401 RTIC 2 (Merkmal `rtic`).
    Rtos,
    /// Die Form `rtos` unter Embassy (M11 Schritt 14d): Takt als Aufgabe
    /// eines Executors hoher Prioritaet, die mit `embassy-time` auf die Frist
    /// wartet; darunter eine fremde Hauptschleife (Merkmal `embassy`).
    Embassy,
}

impl Form {
    /// Das Merkmal des Bring-ups, das die Form waehlt; der eigene Kern
    /// braucht keines.
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Form::Own => None,
            Form::Interrupt => Some("interrupt"),
            Form::Poll => Some("poll"),
            Form::Rtos => Some("rtic"),
            Form::Embassy => Some("embassy"),
        }
    }
}

/// Wie ein Programm auf das Board kommt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Nach so vielen Ticks endet der Lauf mit [`END`]; im Messprogramm
    /// die Zahl der Messungen.
    pub ticks: u64,
    /// Das Journal vor dem Lauf loeschen (5.9), wie der Interpreter ohne
    /// Speicher beginnt. Ein Board ohne Journal uebergeht es.
    pub fresh: bool,
    /// Welches Programm.
    pub bin: Bin,
    /// In Echtzeit: Der Tick kommt vom Timer, und die Telemetrie verwirft,
    /// was die Leitung nicht nimmt (12.2). Sonst laeuft `takt` in logischer
    /// Zeit mit verlustfreiem Trace — ein Konformitaetslauf vergleicht nur
    /// die Semantik (FB-292).
    pub timed: bool,
    /// Die Hardware-Konfiguration des Baus (8.10): Sie gibt geplanten
    /// Outputs ihr `guard` (7.5). Ohne sie rechnet das Board wie die
    /// Simulation.
    pub hardware: Option<PathBuf>,
    /// Die Form des Ports (12.11). Ein Board, dessen Bring-up sie nicht
    /// kennt ([`Board::forms`]), baut dann nicht.
    pub form: Form,
    /// Vor dem Lauf die Fliesskomma-Umgebung verstellen, wie ein Wirt es
    /// fuer seinen eigenen Code darf: Flush-to-Zero, Default-NaN, Rundung
    /// gegen null (4.2, 12.11). Takt rechnet trotzdem wie der Interpreter,
    /// und die Abschlusszeile nennt das Steuerregister danach. Ein Board
    /// ohne FPU uebergeht es.
    pub hostile_fpu: bool,
    /// Der Build des Abbilds (8.3): `Sim` fuer einen Vergleich mit dem
    /// Interpreter, in dem Plant-Modelle auf dem Board mitlaufen; `Hw`, wenn
    /// die Treiber die Eingaenge speisen.
    pub build: takt_sema::Build,
    /// Zeilen an die Konsole des Boards, je mit ihrem Abstand zur ersten
    /// Zeile des Traces (`t=0`): Tunes vom Host (8.4).
    pub console: Vec<ConsoleLine>,
    /// Ein Lauf, der die Stack-Reserve erst misst (13.8): Die Schranke des
    /// Programms rechnet noch mit der alten Reserve, und [`complete`] haelt
    /// die Tiefe nicht gegen sie.
    pub calibrating: bool,
    /// Ein absichtlich falscher Port, den die Portpruefung verwerfen muss
    /// ([`Flaw`]).
    pub flaw: Option<Flaw>,
    /// Die Ausgabe urteilt die Portpruefung ([`crate::port::judge`]): Sie
    /// prueft die Vollstaendigkeit selbst und nennt sie neben den anderen
    /// Pflichten, also gibt [`complete`] sie ungeprueft weiter.
    pub judged: bool,
    /// Ein Messlauf in Echtzeit, der nur seine Bilanz belegt, nicht seinen
    /// Trace: der Lastkern der Stack-Reserve (13.8, FB-741). Verwirft die
    /// Telemetrie, wie 12.2 es im Betrieb verlangt, wenn die Leitung ein
    /// Aufholen nicht traegt, zaehlt der Lauf trotzdem; dass ankam, was das
    /// Board sandte, prueft [`complete`] weiter (FB-304).
    pub lossy: bool,
}

/// Ein absichtlich falscher Port (12.11, M11 Schritt 15): Die Portpruefung
/// ([`crate::port`]) muss ihn verwerfen. Das Bring-up des F401 baut ihn in
/// der Interruptform (`TAKT_PORT_FLAW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flaw {
    /// Eine ISR legt die FPU-Register der Hauptschleife nicht ab, und der
    /// Schritt rechnet in ihnen weiter.
    UnsavedFpu,
    /// Die Jobs haben die Prioritaet des Schritts: Er kann sie nicht
    /// unterbrechen und wartet, bis sie fertig sind.
    UninterruptibleJobs,
}

impl Flaw {
    /// Der Wert von `TAKT_PORT_FLAW`.
    pub fn name(self) -> &'static str {
        match self {
            Flaw::UnsavedFpu => "fpu",
            Flaw::UninterruptibleJobs => "jobs",
        }
    }
}

/// Eine Zeile an die Konsole des Boards: `line`, `after` nach der ersten
/// Zeile des Traces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    /// Der Abstand zur ersten Zeile des Traces.
    pub after: Duration,
    /// Die Zeile, mit ihrem Zeilenende.
    pub line: String,
}

/// Woran die Erfassung den Beginn des Traces erkennt.
const TRACE_START: &[u8] = b"t=0 ";

/// Die Konsolenzeile fuer `tune <name> <wert>` (8.4): Index des Parameters
/// und Wert in kanonischer Byteform als Hexziffern, wie
/// `takt_rt_baremetal::console` sie liest. Das Board kennt keine Namen und
/// rechnet keine Dezimalzahl um; Range und Typ prueft sein Rahmen. `None`,
/// wenn das Programm kein solches Tunable hat oder der Text kein Wert seines
/// Typs ist.
pub fn tune_line(p: &takt_mir::program::Program, name: &str, text: &str) -> Option<String> {
    let index = p.params.iter().position(|q| q.name == name && q.tunable)?;
    let bytes = crate::harness::tune_bytes(p, index, text)?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("tune {index} {hex}\n"))
}

impl Options {
    /// Ein Konformitaetslauf ueber `ticks` Ticks mit leerem Journal.
    pub fn fresh(ticks: u64) -> Options {
        Options { ticks, fresh: true, ..Options::default() }
    }

    /// Derselbe Lauf in der Form `form` (12.11).
    pub fn in_form(self, form: Form) -> Options {
        Options { form, ..self }
    }

    /// Derselbe Lauf mit der Hardware-Konfiguration `path`.
    pub fn with_hardware(self, path: PathBuf) -> Options {
        Options { hardware: Some(path), ..self }
    }

    /// Derselbe Lauf mit verstellter Fliesskomma-Umgebung ([`Options::hostile_fpu`]).
    pub fn with_hostile_fpu(self) -> Options {
        Options { hostile_fpu: true, ..self }
    }

    /// Derselbe Lauf, und `after` nach der ersten Zeile des Traces geht
    /// `line` an die Konsole des Boards.
    pub fn with_console(mut self, after: Duration, line: &str) -> Options {
        self.console.push(ConsoleLine { after, line: line.to_string() });
        self
    }

    /// Ein Lauf ueber `ticks` Ticks in Echtzeit, fuer das, was nur die
    /// Uhr zeigt: Tick-Jitter und Stack unter Last (13.8).
    pub fn timed(ticks: u64) -> Options {
        Options { ticks, fresh: true, timed: true, ..Options::default() }
    }

    /// Derselbe Lauf als Messung der Stack-Reserve ([`Options::calibrating`]).
    pub fn calibrating(self) -> Options {
        Options { calibrating: true, ..self }
    }

    /// Derselbe Lauf mit einem absichtlich falschen Port ([`Flaw`]).
    pub fn with_flaw(self, flaw: Flaw) -> Options {
        Options { flaw: Some(flaw), ..self }
    }

    /// Derselbe Lauf fuer die Portpruefung ([`Options::judged`]).
    pub fn judged(self) -> Options {
        Options { judged: true, ..self }
    }

    /// Derselbe Lauf als Messlauf, der nur seine Bilanz belegt ([`Options::lossy`]).
    pub fn lossy(self) -> Options {
        Options { lossy: true, ..self }
    }

    /// Teil `part` des Messprogramms mit `runs` Messungen je Reihe, um
    /// `shift` Byte verschoben.
    pub fn bench(runs: u64, shift: u32, part: crate::bench::Part) -> Options {
        Options { ticks: runs, fresh: true, bin: Bin::Bench { shift, part }, ..Options::default() }
    }

    /// Wie lange ein Lauf hoechstens dauert, bis `takt end` kommt: das
    /// Messprogramm misst alle Kerne in einem Lauf.
    pub fn within(&self) -> std::time::Duration {
        match self.bin {
            Bin::Bench { .. } => std::time::Duration::from_secs(900),
            _ => std::time::Duration::from_secs(30),
        }
    }

    /// Die Vektoren der kuratierten Natives.
    pub fn natives() -> Options {
        Options { bin: Bin::Natives, ..Options::default() }
    }

    /// Der Vorgaenger, der eine Region der MPU stehen laesst ([`Bin::Leftover`]).
    pub fn leftover() -> Options {
        Options { bin: Bin::Leftover, ..Options::default() }
    }
}

/// Die Umgebungsvariable des Laufs, der nur baut ([`build_only`]).
pub const BUILD_ONLY: &str = "TAKT_BOARD_BUILD_ONLY";

/// Der Fehler, mit dem im Lauf, der nur baut, jeder Zugriff auf ein Board
/// endet: Das Abbild ist gebaut, und mehr ist nicht zu tun.
pub const BUILT_ONLY: &str = "nur gebaut (TAKT_BOARD_BUILD_ONLY)";

/// **Ein Lauf, der nur baut** (`TAKT_BOARD_BUILD_ONLY=1`, M11 Schritt 34d):
/// Die Boards bauen jedes Abbild, das ihre Tests verlangen, in den
/// Zwischenspeicher, und jeder Zugriff auf die Hardware endet mit
/// [`BUILT_ONLY`]. Ein Runner ohne Boards baut so genau die Abbilder, die ein
/// Lauf am Board braucht, ohne eine zweite Liste der Programme; der lokale
/// Lauf schreibt und prueft sie nur noch (`tools/board-run.ps1 -FromCi`).
pub fn build_only() -> bool {
    std::env::var_os(BUILD_ONLY).is_some_and(|v| v == "1")
}

/// Der Port eines Boards; ohne Port, im Lauf, der nur baut, [`BUILT_ONLY`].
pub(crate) fn attached(port: Option<&str>) -> Result<&str, String> {
    port.ok_or_else(|| BUILT_ONLY.to_string())
}

/// Baut die Abbilder eines Boards, ohne das Board zu belegen
/// ([`Board::builder`]).
pub type Builder = Box<dyn Fn(&Path, &Options) -> Result<PathBuf, String> + Send + Sync>;

/// So viele Abbilder entstehen zugleich, jedes in seinem eigenen
/// Zielverzeichnis: Cargo sperrt ein Zielverzeichnis fuer einen Bau, und die
/// LTO eines Abbilds rechnet auf einem Kern (FB-464).
pub const BUILDS: usize = 2;

/// Ein Board am Host.
pub trait Board {
    /// Der Name in Meldungen und Berichten.
    fn name(&self) -> &'static str;

    /// Die Zielklasse, wie `takt build --target` sie nennt (12.8).
    fn target(&self) -> &'static str;

    /// Baut das Bring-up mit `program` und liefert das ELF; das Abbild
    /// kommt aus dem Zwischenspeicher, wenn es dort liegt.
    fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        (self.builder(0))(program, options)
    }

    /// Wie [`Board::build`], ohne das Board und im Bauplatz `slot` (unter
    /// [`BUILDS`]): Damit bauen andere Faeden die naechsten Abbilder,
    /// waehrend das Board das jetzige schreibt und ausfuehrt.
    fn builder(&self, slot: usize) -> Builder;

    /// Schreibt das Abbild, startet es und liest den Trace bis [`END`].
    fn run(&mut self, elf: &Path, options: &Options) -> Result<String, String>;

    /// Die Lagen, in denen `takt bench` misst, als Verschiebung des
    /// Programms in Byte (FB-367). Wer aus dem RAM rechnet, braucht eine.
    fn placements(&self) -> &'static [u32] {
        &[0]
    }

    /// In wie viele Teile `takt bench` sein Messprogramm teilt, damit jedes
    /// Abbild mit seiner Mindestreserve passt (FB-740).
    fn bench_parts(&self) -> u32 {
        1
    }

    /// Wie viel Platz das Abbild `elf` in den Speicherbereichen des Boards
    /// laesst, in denen es eng werden kann (FB-740); leer, wo das Board keine
    /// Grenze kennt.
    fn room(&self, _elf: &Path) -> Result<Vec<Room>, String> {
        Ok(Vec::new())
    }

    /// Die Formen, in denen das Bring-up laufen kann ([`Options::form`],
    /// 12.11): In jeder misst `takt bench` die Stack-Reserve.
    fn forms(&self) -> &'static [Form] {
        &[Form::Own]
    }
}

/// Wie viel Platz ein Abbild in einem Speicherbereich des Boards laesst
/// (FB-740).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Room {
    /// Der Bereich, wie Meldungen ihn nennen.
    pub region: &'static str,
    /// Seine Groesse in Byte.
    pub size: u64,
    /// Was das Abbild davon frei laesst.
    pub free: u64,
}

impl Room {
    /// Der Bereich, in dem ein Abbild von `used` Byte liegt.
    pub fn of(region: &'static str, size: u64, used: u64) -> Room {
        Room { region, size, free: size.saturating_sub(used) }
    }

    /// Das RAM bis zum oberen Ende `top` des Hauptstacks: Was zwischen dem
    /// Boden des Stacks (`floor`) und `TICK_STACK_BYTES` (`stack`) frei
    /// bleibt, hat das Abbild uebrig (12.3, `takt_stack.x`). `origin` ist der
    /// Anfang des RAM.
    pub fn below_stack(origin: u64, top: u64, floor: u64, stack: u64) -> Room {
        Room { region: "RAM", size: top.saturating_sub(origin), free: top.saturating_sub(floor).saturating_sub(stack) }
    }
}

/// Der Wert eines Symbols in `symbols`, ein Fehler mit seinem Namen, wenn es
/// fehlt.
pub(crate) fn symbol(symbols: &[takt_llvm::inspect::Symbol], name: &str) -> Result<u64, String> {
    symbols.iter().find(|s| s.name == name).map(|s| s.address).ok_or_else(|| format!("das Abbild nennt `{name}` nicht"))
}

/// Die Wurzel des Repositorys ([`crate::repo_root`]).
pub fn root() -> PathBuf {
    crate::repo_root()
}

/// Ein Programm des Differentialkorpus.
pub fn corpus_path(name: &str) -> PathBuf {
    root().join("corpus-try").join(name)
}

/// Das Zielverzeichnis des Bauplatzes `slot` ab dem zweiten ([`BUILDS`]).
fn slot_dir(slot: usize) -> PathBuf {
    crate::target_dir().join("takt-board-build").join(slot.to_string())
}

/// So oft baut ein Prozess neu, wenn ihm ein anderer das Binary zwischen
/// Bau und Kopie ueberschrieben hat (KON1-009).
pub(crate) const ATTEMPTS: usize = 5;

/// Das Bring-up eines Boards, wie `cargo` es baut.
pub(crate) struct Bringup {
    /// Das Crate, relativ zur Wurzel.
    pub dir: &'static str,
    /// Das Zieltripel.
    pub triple: &'static str,
    /// Dateien, die der Bau liest, relativ zum Manifest: auch die
    /// Hardware-Konfiguration des Boards, die `build.rs` ohne
    /// `TAKT_HARDWARE` nimmt (FB-514).
    pub inputs: &'static [&'static str],
    /// Umgebung des Baus, die `.cargo/config.toml` des Bring-ups sonst
    /// setzt: Von aussen gebaut greift jene Datei nicht.
    pub env: &'static [(&'static str, &'static str)],
}

impl Bringup {
    /// Das ELF fuer `program`, aus dem Zwischenspeicher oder frisch gebaut.
    ///
    /// Der Bau kostet einige Sekunden je Programm und ergibt bei gleichen
    /// Eingaben dasselbe Abbild (FB-270). Der Speicher liegt im
    /// Zielverzeichnis unter `takt-board-images/<tripel>-<stand>` und haelt
    /// je Board nur den Stand der Quellen, gegen den gebaut wird: Ein neuer
    /// Stand raeumt die frueheren weg. Ohne das wuchs er mit jeder Aenderung
    /// an `src`, bis das Temp-Laufwerk voll war.
    ///
    /// Baut ein zweiter Prozess in dasselbe Zielverzeichnis, ueberschreibt er
    /// womoeglich das ELF zwischen dem Bau und dem Kopieren. Darum legt jeder
    /// Bau seinen Schluessel als Symbol ins ELF (`TAKT_IMAGE_KEY`,
    /// `takt_board_support::image_key`), und veroeffentlicht wird nur eine
    /// Kopie, die ihn traegt; sonst wird neu gebaut. Geprueft wird das ELF:
    /// Das Rohabbild fuer den Bootloader entsteht erst aus ihm, und `objcopy`
    /// laesst die Symbole weg.
    ///
    /// Gebaut wird im Bauplatz `slot` ([`BUILDS`]): ab dem zweiten in einem
    /// eigenen Zielverzeichnis, in denselben Zwischenspeicher.
    ///
    /// Stand und Schluessel haengen nur an Inhalten ([`Fingerprint`]): Ein
    /// Abbild, das ein Runner unter Linux baut, findet ein Lauf unter Windows
    /// wieder (M11 Schritt 34d).
    pub fn build(&self, program: &Path, options: &Options, slot: usize) -> Result<PathBuf, String> {
        let stand = self.stand()?;
        let key = format!("{:016x}", self.key(program, options)?);
        let cached = stand.join(format!("{key}.elf"));
        if cached.is_file() {
            return Ok(cached);
        }
        missed(&stand, &cached, &program.display().to_string())?;
        let ours = |part: &Path| -> Result<bool, String> {
            let elf = std::fs::read(part).map_err(|e| format!("{}: {e}", part.display()))?;
            Ok(takt_board_support::image_key::carries(&elf, &key))
        };
        for _ in 0..ATTEMPTS {
            let elf = self.build_uncached(program, options, &key, slot)?;
            self.enter(&stand)?;
            if publish_checked(&elf, &cached, ours)? {
                return Ok(cached);
            }
        }
        Err(format!("{key}: {ATTEMPTS}-mal gebaut, jedes Mal hatte ein anderer Prozess das ELF ueberschrieben"))
    }

    /// Das Verzeichnis des Stands im Zwischenspeicher: `<tripel>-<stand>`
    /// unter [`IMAGES`].
    pub(crate) fn stand(&self) -> Result<PathBuf, String> {
        Ok(images().join(format!("{}-{:016x}", self.triple, self.sources()?)))
    }

    /// Legt das Verzeichnis des Stands an ([`enter_stand`]).
    pub(crate) fn enter(&self, stand: &Path) -> Result<(), String> {
        enter_stand(self.triple, stand)
    }

    /// Der Stand, gegen den gebaut wird: die Quellen und Verdrahtungen aller
    /// Crates und die Dateien des Bring-ups.
    fn sources(&self) -> Result<u64, String> {
        let mut f = Fingerprint::new();
        let dir = root().join(self.dir);
        for name in self.inputs {
            f.str(name);
            f.file_if_any(&dir.join(name));
        }
        hash_crates(&mut f)?;
        hash_build_inputs(&dir, &mut f)?;
        Ok(f.finish())
    }

    /// Der Schluessel eines Abbilds in seinem Stand ([`image_key`]).
    fn key(&self, program: &Path, options: &Options) -> Result<u64, String> {
        image_key(self.triple, program, options)
    }

    /// `cargo build` des Bring-ups mit dem Programm; `key` landet als Symbol
    /// im ELF.
    fn build_uncached(&self, program: &Path, options: &Options, key: &str, slot: usize) -> Result<PathBuf, String> {
        let bin = match options.bin {
            Bin::Takt => "takt",
            Bin::Bench { .. } => "bench",
            Bin::Natives => "natives",
            Bin::Leftover => "leftover",
        };
        let target = crate::target_dir();
        let tool = target.join("release").join(if cfg!(windows) { "takt.exe" } else { "takt" });
        // Ein Kern bleibt frei; die Plaetze teilen sich die uebrigen, sonst
        // reichte ein kalter Bau in zwei Plaetzen an die Auslagerungsgrenze.
        let cores = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);
        let jobs = (cores.saturating_sub(1) / BUILDS).max(1);
        let mut cargo = Command::new("cargo");
        cargo
            .args(["build", "--release", "--target", self.triple, "--bin", bin])
            .args(["-j", &jobs.to_string()])
            .arg("--message-format=json-render-diagnostics")
            .arg("--manifest-path")
            .arg(root().join(self.dir).join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", if slot == 0 { target } else { slot_dir(slot) })
            .env(crate::bringup::TOOL, tool)
            .env("TAKT_PROGRAM", program)
            .env("TAKT_TICKS", options.ticks.to_string())
            .env("TAKT_IMAGE_KEY", key)
            .env("TAKT_BUILD", if options.build == takt_sema::Build::Hw { "hw" } else { "sim" })
            .envs(self.env.iter().copied());
        if options.fresh {
            cargo.env("TAKT_FRESH_JOURNAL", "1");
        } else {
            cargo.env_remove("TAKT_FRESH_JOURNAL");
        }
        match &options.hardware {
            Some(hw) => cargo.env("TAKT_HARDWARE", hw),
            None => cargo.env_remove("TAKT_HARDWARE"),
        };
        if options.timed {
            cargo.env("TAKT_TIMED", "1");
        } else {
            cargo.env_remove("TAKT_TIMED");
        }
        if options.hostile_fpu {
            cargo.env("TAKT_HOSTILE_FPU", "1");
        } else {
            cargo.env_remove("TAKT_HOSTILE_FPU");
        }
        match options.flaw {
            Some(flaw) => cargo.env("TAKT_PORT_FLAW", flaw.name()),
            None => cargo.env_remove("TAKT_PORT_FLAW"),
        };
        if let Some(feature) = options.form.feature() {
            cargo.args(["--features", feature]);
        }
        match &options.bin {
            Bin::Bench { shift, part } => cargo
                .args(["--features", "bench"])
                .env("TAKT_BENCH_SHIFT", shift.to_string())
                .env(crate::bringup::BENCH_PART, part.to_string()),
            _ => cargo.env_remove("TAKT_BENCH_SHIFT").env_remove(crate::bringup::BENCH_PART),
        };
        built_executable(&mut cargo)
    }
}

/// Legt das Verzeichnis des Stands `stand` an. Ist es neu, gehen zuerst die
/// Abbilder frueherer Staende derselben Art `kind` (`<kind>-<stand>`, das
/// Tripel eines Bring-ups): Gegen sie baut niemand mehr.
pub(crate) fn enter_stand(kind: &str, stand: &Path) -> Result<(), String> {
    if !stand.is_dir() {
        clear_older_stands(kind, stand);
    }
    std::fs::create_dir_all(stand).map_err(|e| format!("{}: {e}", stand.display()))
}

/// Raeumt die Staende der Art `kind` weg, ausser `stand` selbst (FB-737):
/// Zwei Bauten desselben neuen Stands, in zwei Faeden eines Prozesses oder
/// in zwei Prozessen, sehen ihn beide als neu. Raeumte der zweite alles
/// seiner Art, naehme er dem ersten das Verzeichnis samt dem Abbild, das der
/// gerade veroeffentlicht.
fn clear_older_stands(kind: &str, stand: &Path) {
    let prefix = format!("{kind}-");
    let own = stand.file_name();
    for entry in std::fs::read_dir(images()).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) && Some(name.as_os_str()) != own {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Die Quellen aller Crates und die Verdrahtung des Bring-ups und der
/// Treiber-Crates (12.6).
pub(crate) fn hash_crates(f: &mut Fingerprint) -> Result<(), String> {
    for (name, krate) in crate_dirs()? {
        f.str(&name);
        let src = krate.join("src");
        if src.is_dir() {
            hash_tree(&src, f)?;
        }
        f.file_if_any(&krate.join(crate::bringup::WIRING));
    }
    Ok(())
}

/// Der Schluessel eines Abbilds fuer `triple` in seinem Stand: das Programm
/// samt den Konfigurationen, die es importiert, die Optionen und die
/// C-Referenz. Aendert sich nichts davon, ist das Abbild dasselbe.
pub(crate) fn image_key(triple: &str, program: &Path, options: &Options) -> Result<u64, String> {
    let mut f = Fingerprint::new();
    hash_program(program, &mut f)?;
    f.str(triple);
    f.u64(options.ticks);
    for flag in [options.fresh, options.timed, options.hostile_fpu, options.build == takt_sema::Build::Hw] {
        f.u64(u64::from(flag));
    }
    f.str(options.form.feature().unwrap_or("own"));
    f.str(options.flaw.map_or("", Flaw::name));
    match &options.hardware {
        Some(hw) => {
            f.u64(1);
            f.file(hw)?;
        }
        None => f.u64(0),
    }
    match &options.bin {
        Bin::Takt => f.str("takt"),
        // Die Kerne stehen ausserhalb von `src`; ihre Kennung deckt sie.
        Bin::Bench { shift, part } => {
            f.str("bench");
            f.u64(u64::from(*shift));
            f.str(&part.to_string());
            f.str(&crate::bench::suite_id(&crate::bench::suite(), &crate::bench::math_vectors()?));
        }
        // Die Vektoren der Natives und der Mathematik, die das Bauskript
        // liest, stehen in den Spezifikationen, nicht in `src`.
        Bin::Natives => {
            f.str("natives");
            f.file(&crate::natives::spec_path())?;
            f.file(&crate::math::spec_path())?;
        }
        Bin::Leftover => f.str("leftover"),
    }
    Ok(f.finish())
}

/// Fuehrt `cargo build --message-format=json-render-diagnostics` aus und
/// nennt das gebaute Binary.
pub(crate) fn built_executable(cargo: &mut Command) -> Result<PathBuf, String> {
    let out = cargo.output().map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    // Die JSON-Meldungen nennen das Binary; ein Parser fuer eine
    // Zeichenkette in einer Zeile ist kuerzer als eine Abhaengigkeit.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let rest = &l[l.find("\"executable\":\"")? + "\"executable\":\"".len()..];
            Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
        })
        .next_back()
        .map(PathBuf::from)
        .ok_or_else(|| "cargo meldete kein Binary".to_string())
}

/// Das Verzeichnis des Zwischenspeichers der Abbilder im Zielverzeichnis.
pub const IMAGES: &str = "takt-board-images";

/// Die Datei in [`IMAGES`], in die jeder Bau eines Abbilds, das der
/// Zwischenspeicher nicht hielt, vorher eine Zeile schreibt: `<stand>/<datei>
/// <programm>`. `tools/board-run.ps1 -FromCi` zaehlt sie; Ziel ist keine.
pub const MISSES: &str = "misses.log";

/// Der Zwischenspeicher der Abbilder ([`IMAGES`]).
pub fn images() -> PathBuf {
    crate::target_dir().join(IMAGES)
}

/// Vermerkt in [`MISSES`], dass `cached` im Stand `stand` fehlte und fuer
/// `what` gebaut wird.
pub(crate) fn missed(stand: &Path, cached: &Path, what: &str) -> Result<(), String> {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let log = images().join(MISSES);
    std::fs::create_dir_all(images())
        .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(&log))
        .and_then(|mut file| file.write_all(format!("{}/{} {what}\n", name(stand), name(cached)).as_bytes()))
        .map_err(|e| format!("{}: {e}", log.display()))
}

/// Ein Fingerabdruck ueber Inhalte, auf jedem Wirt derselbe: SHA-256, davon
/// die ersten acht Byte (M11 Schritt 34d).
///
/// `DefaultHasher` taugte dafuer nicht: Seine Ausgabe ist ueber Versionen von
/// Rust nicht festgelegt, und ein `OsStr` hasht unter Windows andere Bytes als
/// unter Linux. Hier geht jedes Stueck mit seiner Laenge davor ein, Namen als
/// UTF-8 und der Inhalt einer Datei ohne `\r`: Dieselbe Datei mit CRLF und
/// mit LF ist dieselbe.
pub(crate) struct Fingerprint(takt_native::sha256::Ctx);

impl Fingerprint {
    pub(crate) fn new() -> Fingerprint {
        Fingerprint(takt_native::sha256::Ctx::new())
    }

    /// Eine Zahl, acht Byte Little Endian.
    pub(crate) fn u64(&mut self, n: u64) {
        self.0.update(&n.to_le_bytes());
    }

    /// Bytes mit ihrer Laenge davor: Zwei Folgen von Stuecken ergeben nur
    /// dann denselben Strom, wenn sie gleich sind.
    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        self.u64(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        self.0.update(bytes);
    }

    /// Ein Name oder Wert.
    pub(crate) fn str(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    /// Der Inhalt einer Datei, ohne `\r`.
    pub(crate) fn text(&mut self, content: &[u8]) {
        let lf: Vec<u8> = content.iter().copied().filter(|&b| b != b'\r').collect();
        self.bytes(&lf);
    }

    /// Die Datei `path` wie [`Fingerprint::text`].
    pub(crate) fn file(&mut self, path: &Path) -> Result<(), String> {
        let content = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        self.text(&content);
        Ok(())
    }

    /// Die Datei `path`, wenn es sie gibt; sonst ein Zeichen dafuer, dass
    /// sie fehlt.
    pub(crate) fn file_if_any(&mut self, path: &Path) {
        match std::fs::read(path) {
            Ok(content) => {
                self.u64(1);
                self.text(&content);
            }
            Err(_) => self.u64(0),
        }
    }

    /// Die ersten acht Byte, so gelesen, dass `{:016x}` die ersten sechzehn
    /// Hexziffern des SHA-256 ergibt.
    pub(crate) fn finish(self) -> u64 {
        let [a, b, c, d, e, f, g, h, ..] = self.0.finish();
        u64::from_be_bytes([a, b, c, d, e, f, g, h])
    }
}

/// Die Crates des Repositorys: je Verzeichnis unter `crates` mit einem
/// `Cargo.toml` sein Name und Pfad, nach Namen geordnet.
pub(crate) fn crate_dirs() -> Result<Vec<(String, PathBuf)>, String> {
    let crates: Vec<(String, PathBuf)> = entries(&root().join("crates"))?;
    Ok(crates.into_iter().filter(|(_, dir)| dir.join("Cargo.toml").is_file()).collect())
}

/// Die Eintraege eines Verzeichnisses mit ihrem Namen als UTF-8, nach Namen
/// geordnet: eine Ordnung, die nicht am System haengt.
fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    out.sort();
    Ok(out)
}

/// Ein Programm und die Konfigurationen, die es per `import channels`
/// liest (8.2): Sie liegen neben ihm und gehen in sein Abbild ein.
pub(crate) fn hash_program(program: &Path, f: &mut Fingerprint) -> Result<(), String> {
    let text = std::fs::read_to_string(program).map_err(|e| format!("{}: {e}", program.display()))?;
    f.text(text.as_bytes());
    let dir = program.parent().unwrap_or(Path::new("."));
    for file in takt_sema::channel_imports(&text) {
        f.str(&file);
        f.file(&dir.join(&file))?;
    }
    Ok(())
}

/// Was ein Abbild bestimmt, ohne in `src` zu stehen: Bauskripte und
/// Manifeste aller Crates, Manifest und Lock des Workspace und die Versionen
/// von rustc (wie `cargo` sie im Bring-up `dir` waehlt) und clang.
///
/// Von `--version` zaehlt nur die erste Zeile, die Version samt Quelle und
/// Commit: Die Zeilen danach nennen Zielplattform und Installationspfad, und
/// die sind unter Windows andere als unter Linux.
pub(crate) fn hash_build_inputs(dir: &Path, f: &mut Fingerprint) -> Result<(), String> {
    let root = root();
    for name in ["Cargo.toml", "Cargo.lock"] {
        f.str(name);
        f.file_if_any(&root.join(name));
    }
    for (name, krate) in crate_dirs()? {
        f.str(&name);
        for file in ["build.rs", "Cargo.toml"] {
            f.file_if_any(&krate.join(file));
        }
    }
    let first_line = |tool: &Path| {
        Command::new(tool)
            .arg("--version")
            .current_dir(dir)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or_default().trim().to_string())
            .unwrap_or_default()
    };
    f.str(&first_line(Path::new("rustc")));
    if let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() {
        f.str(&first_line(&clang));
    }
    Ok(())
}

/// Kopiert ein gebautes Abbild in den Zwischenspeicher: erst unter einem
/// Zwischennamen, dann umbenannt. Ein abgebrochener Lauf hinterlaesst so
/// kein halbes Abbild, das der naechste fuer gueltig hielte.
///
/// `ours` prueft die Kopie, bevor sie ihren Namen bekommt: Baut ein anderer
/// Prozess in dasselbe Zielverzeichnis, kann er das Binary zwischen dem Bau
/// und dem Kopieren ueberschrieben haben. Die Kopie aendert niemand mehr;
/// ist sie nicht die eigene, gilt nichts (`Ok(false)`). Hat ein anderer
/// Prozess denselben Schluessel schon veroeffentlicht, bleibt dessen Abbild
/// — es laeuft womoeglich gerade.
pub(crate) fn publish_checked(
    built: &Path,
    cached: &Path,
    ours: impl Fn(&Path) -> Result<bool, String>,
) -> Result<bool, String> {
    let name = cached.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Die Endung bleibt am Ende: Ein Wirtsabbild muss sich so ausfuehren lassen.
    let part = cached.with_file_name(format!("{}.part.{name}", std::process::id()));
    let fail = |e: std::io::Error| format!("{}: {e}", cached.display());
    std::fs::copy(built, &part).map_err(fail)?;
    let published = match ours(&part) {
        Ok(true) if cached.is_file() => Ok(true),
        Ok(true) => match std::fs::rename(&part, cached) {
            Err(_) if cached.is_file() => Ok(true),
            other => other.map(|()| true).map_err(fail),
        },
        other => other,
    };
    let _ = std::fs::remove_file(&part);
    published
}

/// Namen und Inhalte eines Verzeichnisbaums, in fester Ordnung: die Zahl der
/// Eintraege, dann je Eintrag sein Name und der Inhalt der Datei oder der
/// Baum des Verzeichnisses. Wo der Baum liegt, zaehlt nicht.
pub(crate) fn hash_tree(dir: &Path, f: &mut Fingerprint) -> Result<(), String> {
    let entries = entries(dir)?;
    f.u64(u64::try_from(entries.len()).unwrap_or(u64::MAX));
    for (name, path) in entries {
        f.str(&name);
        if path.is_dir() {
            f.u64(1);
            hash_tree(&path, f)?;
        } else {
            f.u64(0);
            f.file(&path)?;
        }
    }
    Ok(())
}

/// Warum ein fremder Prozess keine Ausgabe lieferte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Er endete mit Fehler; das ist seine Fehlerausgabe.
    Status(String),
    /// Er kehrte binnen der Frist nicht zurueck und wurde beendet.
    Timeout,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Status(e) => f.write_str(e.trim_end()),
            Failure::Timeout => f.write_str("kehrt binnen der Frist nicht zurueck"),
        }
    }
}

/// Ein fremder Prozess mit Frist; seine Ausgabe bei Erfolg.
///
/// Die Pipes liest je ein Thread, damit ein gespraechiger Aufruf nicht an
/// ihnen haengt. Ein `probe-rs reset` hing einmal 19 Minuten (FB-266):
/// Ohne Frist steht dann die ganze Suite.
pub(crate) fn run_bounded(program: &str, args: &[&str], within: Duration) -> Result<String, Failure> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Failure::Status(format!("{program}: {e}")))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let until = Instant::now() + within;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() > until => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::Timeout);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(Failure::Status(format!("{program}: {e}"))),
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    if status.success() { Ok(stdout) } else { Err(Failure::Status(format!("{stderr}{stdout}"))) }
}

/// Liest eine Pipe in einem eigenen Thread zu Ende.
fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut text);
        }
        String::from_utf8_lossy(&text).into_owned()
    })
}

/// Oeffnet `port`, laesst `start` das Programm starten und liest dann bis
/// [`END`] oder bis `within` verstrichen ist.
///
/// Der Leser laeuft ab dem Oeffnen, damit nichts verloren geht, was das
/// Board gleich nach dem Start schreibt; die Frist beginnt erst, wenn
/// `start` zurueckkehrt, weil Schreiben und Starten ihre eigenen Fristen
/// haben. Der Leser ist ein eigener Thread: Ein `read`, das trotz Timeout
/// nicht zurueckkehrt (FB-266), haelt so nur ihn, nicht den Aufrufer.
///
/// `flow` ist die Flusskontrolle der Leitung: XON/XOFF, wo sie keinen
/// eigenen Rueckstau hat (FB-306).
///
/// `fresh`: `start` startet ein neues Abbild. Was vorher ankommt, schrieb
/// das vorige — der USB-Serial-JTAG des C6 haelt 64 Byte ueber Flashen und
/// Reset, ein Adapter seinen Puffer — und wird bis zur ersten Stille
/// verworfen (FB-437). Ohne `fresh` gehoert alles zum Lauf.
///
/// Ohne [`END`] ist das Ergebnis der Text, der bis dahin kam: Ob das ein
/// Befund ist, entscheidet der Aufrufer.
pub(crate) fn capture(
    port: &str,
    baud: u32,
    flow: serialport::FlowControl,
    within: Duration,
    fresh: bool,
    console: &[ConsoleLine],
    start: impl FnOnce() -> Result<(), String>,
) -> Result<String, String> {
    // Kurz: Zwischen zwei Lesevorgaengen schreibt derselbe Faden, was an die
    // Konsole geht. Ein zweiter Griff auf den Port schriebe unter Windows
    // erst, wenn das Lesen zurueckkommt — synchrone Zugriffe auf eine Datei
    // reiht das System hintereinander, und ein Tune kam eine Sekunde zu spaet.
    let mut serial = serialport::new(port, baud)
        .flow_control(flow)
        .timeout(Duration::from_millis(20))
        .open()
        .map_err(|e| format!("{port}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let (send, lines) = mpsc::channel::<String>();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = std::thread::spawn({
        let stop = Arc::clone(&stop);
        move || {
            raise_reader();
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::Relaxed) {
                while let Ok(line) = lines.try_recv() {
                    if let Err(e) = serial.write_all(line.as_bytes()).and_then(|()| serial.flush()) {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                }
                match serial.read(&mut buf) {
                    Ok(n) => {
                        if tx.send(Ok(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        break;
                    }
                }
            }
        }
    });
    let stale = if fresh { discard_stale(&rx).map_err(|e| format!("{port}: {e}")) } else { Ok(()) };
    let started = stale.and_then(|()| start());
    let deadline = Instant::now() + within;
    // Rohbytes, erst am Ende dekodiert: Ein Zeichen, das auf zwei Pakete
    // faellt, bliebe sonst zweimal ein Ersatzzeichen, und die Bilanz
    // (FB-304) zaehlte falsch.
    let mut raw: Vec<u8> = Vec::new();
    let end = END.as_bytes();
    // Die Zeilen an die Konsole, ab dem Beginn des Traces gezaehlt.
    let mut began: Option<Instant> = None;
    let mut pending: Vec<&ConsoleLine> = console.iter().collect();
    pending.sort_by_key(|c| c.after);
    pending.reverse();
    let result = started.and_then(|()| {
        loop {
            let now = Instant::now();
            while let (Some(at), Some(next)) = (began, pending.last())
                && at + next.after <= now
            {
                send.send(next.line.clone()).map_err(|_| format!("{port}: der Lesefaden ist beendet"))?;
                pending.pop();
            }
            let until = match (began, pending.last()) {
                (Some(at), Some(next)) => deadline.min(at + next.after),
                _ => deadline,
            };
            match rx.recv_timeout(until.saturating_duration_since(Instant::now())) {
                Ok(Ok(bytes)) => {
                    let from = raw.len().saturating_sub(end.len().max(TRACE_START.len()));
                    raw.extend_from_slice(&bytes);
                    if began.is_none() && raw[from..].windows(TRACE_START.len()).any(|w| w == TRACE_START) {
                        began = Some(Instant::now());
                    }
                    if raw[from..].windows(end.len()).any(|w| w == end) {
                        break Ok(());
                    }
                }
                Ok(Err(e)) => break Err(format!("{port}: {e}")),
                Err(_) if Instant::now() < deadline => {}
                Err(_) => break Ok(()),
            }
        }
    });
    stop.store(true, Ordering::Relaxed);
    let until = Instant::now() + Duration::from_secs(1);
    while !reader.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    result.map(|()| String::from_utf8_lossy(&raw).into_owned())
}

/// Hebt den Lesefaden auf die hoechste Prioritaet, die das System einem Faden
/// ohne Sonderrechte gibt (unter Windows `TIME_CRITICAL`). Die Leitung wird in
/// Echtzeit gelesen: Kam der Leser unter Last des Rechners zu spaet, lief der
/// Puffer des Treibers voll, er sandte XOFF, und das Board verwarf Trace
/// (FB-530). Der Faden wartet fast immer in `read` und nimmt niemandem Zeit.
/// Verweigert das System die Prioritaet, liest er mit der ueblichen und sagt es.
fn raise_reader() {
    use thread_priority::{ThreadPriority, ThreadPriorityValue, set_current_thread_priority};
    if let Err(e) = set_current_thread_priority(ThreadPriority::Crossplatform(ThreadPriorityValue::MAX)) {
        eprintln!("Lesefaden der Leitung ohne erhoehte Prioritaet: {e:?}");
    }
}

/// Verwirft, was der Leser bringt, bis eine Lesefrist lang nichts kommt —
/// hoechstens eine Sekunde: Ein angehaltenes Abbild schreibt nicht nach.
fn discard_stale(rx: &mpsc::Receiver<Result<Vec<u8>, String>>) -> Result<(), String> {
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let quiet = Duration::from_millis(200).min(until.saturating_duration_since(Instant::now()));
        match rx.recv_timeout(quiet) {
            Ok(Ok(_)) if Instant::now() < until => {}
            Ok(Err(e)) => return Err(e),
            _ => return Ok(()),
        }
    }
}

/// Wartet, bis das System den Port fuehrt (`present`) oder nicht mehr.
pub(crate) fn port_listed(port: &str, present: bool, within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        let listed =
            serialport::available_ports().is_ok_and(|ps| ps.iter().any(|p| p.port_name.eq_ignore_ascii_case(port)));
        if listed == present {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Ein neuer Stand raeumt nur fruehere Staende seiner Art weg, nie sich
    /// selbst** (FB-737): Sah ein zweiter Bau denselben Stand noch als neu,
    /// nahm er dem ersten das Verzeichnis, und dessen Abbild fehlte
    /// (`….part.….elf: No such file or directory`).
    #[test]
    fn a_new_stand_clears_only_older_stands_of_its_kind() {
        let kind = format!("test-stand-{}", std::process::id());
        let older = images().join(format!("{kind}-0000000000000001"));
        let stand = images().join(format!("{kind}-0000000000000002"));
        let foreign = images().join(format!("other-{kind}-0000000000000003"));
        for dir in [&older, &stand, &foreign] {
            std::fs::create_dir_all(dir).expect("Verzeichnis");
        }
        let part = stand.join("1.part.0000000000000004.elf");
        std::fs::write(&part, b"ELF").expect("Datei");
        clear_older_stands(&kind, &stand);
        let (older_left, part_left, foreign_left) = (older.exists(), part.is_file(), foreign.exists());
        for dir in [&older, &stand, &foreign] {
            let _ = std::fs::remove_dir_all(dir);
        }
        assert!(!older_left, "ein frueherer Stand bleibt");
        assert!(part_left, "der eigene Stand verlor sein Abbild");
        assert!(foreign_left, "eine andere Art verlor ihren Stand");
    }

    /// **Jede Eingabe eines Bring-up-Baus gibt es**, auch die
    /// Hardware-Konfiguration des Boards: Sie geht so in den Stand jedes
    /// Abbilds ein, und eine neue Kalibrierung baut neu (FB-514).
    #[test]
    fn every_input_of_a_bringup_build_exists() {
        for bringup in [&super::stm32f401::BRINGUP, &super::esp32c6::BRINGUP] {
            for name in bringup.inputs {
                assert!(super::root().join(bringup.dir).join(name).is_file(), "{}: {name}", bringup.dir);
            }
            assert!(bringup.inputs.iter().any(|n| n.ends_with(".hw")), "{}: ohne Hardware-Konfiguration", bringup.dir);
        }
    }

    /// Ein Lauf des Boards: Kopf, Marke, `body`, Bilanz mit `sent` — in der
    /// Form, die `takt_rt_baremetal::run::report` schreibt (dessen Test
    /// `the_balance_line_names_every_counter`).
    fn run(body: &str, dropped: u32, sent: usize) -> String {
        format!(
            "takt auf stm32f401\r\n\r\ntakt trace\r\n{body}takt schlief 0 ueberlaeufe 0 verspaetet 0 verloren 0 \
             rueckstand 0 ns verworfen {dropped} gesendet {sent} journal geschrieben 0 fehlgeschlagen 0 flush 1 \
             nvm loeschen 0 ns programmieren 0 ns stack 2048\r\ntakt end\r\n"
        )
    }

    /// Verworfene Bytes machen den Lauf unbrauchbar, ein vollstaendiger
    /// geht durch, und ein Messkern hat weder Marke noch Bilanz.
    #[test]
    fn a_trace_with_dropped_bytes_is_refused() {
        let body = "t=1 out a 1\r\n";
        let any = Options::default();
        let refused = complete(run(body, 25076, body.len()), &any).expect_err("verworfen");
        assert!(refused.contains("25076"), "{refused}");
        // Der Text des verworfenen Laufs liegt in der Datei, die die Meldung nennt.
        let file = refused.split_once("der Lauf: ").map(|(_, f)| f.trim()).expect("eine Datei");
        assert!(std::fs::read_to_string(file).is_ok_and(|kept| kept.contains(body)), "{file}");
        assert!(complete(run(body, 0, body.len()), &any).is_ok());
        let bench = Options::bench(1, 0, crate::bench::Part::WHOLE);
        assert!(complete("bench takt frame min 1\ntakt end\n".to_string(), &bench).is_ok());
        // 12.3: ein Stack ueber seiner Schranke ist ein Fehler, darunter keiner.
        let with = |stacks: &str| run(body, 0, body.len()).replace("stack 2048\r\n", &format!("{stacks}\r\n"));
        let deep = with("stack 2048 schranke 1024");
        assert!(complete(deep, &any).is_err_and(|e| e.contains("TICK_STACK_BYTES")));
        let job = with("stack 512 schranke 1024 jobstack 700 jobschranke 600");
        assert!(complete(job, &any).is_err_and(|e| e.contains("JOB_STACK_BYTES")));
        assert!(complete(with("stack 512 schranke 1024 jobstack 500 jobschranke 600"), &any).is_ok());
        // 13.8: Wer die Reserve misst, haelt die Tiefe nicht gegen die alte.
        assert!(complete(with("stack 2048 schranke 1024"), &Options::default().calibrating()).is_ok());
    }

    /// **Der Lastkern in Echtzeit belegt nur seine Bilanz** (FB-741): Holt der
    /// Kern nach einem blockierenden Journal die verpassten Grenzen nach,
    /// verwirft die Telemetrie, was Ring und Leitung nicht fassen (12.2), und
    /// der Lauf zaehlt trotzdem. Was ankam, muss stimmen (FB-304), und die
    /// Tickschleife, deren Zeitzeilen den Jitter tragen, verwirft nichts.
    #[test]
    fn a_lossy_measuring_run_may_drop_but_not_lose_on_the_way() {
        let body = "t=1 out a 1\r\n";
        let load = Options::timed(1000).lossy().calibrating();
        assert!(complete(run(body, 401, body.len()), &load).is_ok());
        assert!(complete(run(body, 401, body.len() + 1), &load).is_err_and(|e| e.contains("FB-304")));
        let looped = Options::timed(1000).calibrating();
        assert!(complete(run(body, 401, body.len()), &looped).is_err_and(|e| e.contains("401") && !e.contains("XOFF")));
        // Hielt der Adapter die Leitung an, sagt es die Meldung (FB-530).
        let halted = run(body, 79, body.len()).replace(" journal geschrieben", " angehalten 3 journal geschrieben");
        assert!(complete(halted, &looped).is_err_and(|e| e.contains("die Leitung stand 3-mal (XOFF")));
    }

    /// Der Platz eines Abbilds (FB-740): im Flash, was das Abbild nicht
    /// belegt; im RAM, was zwischen dem Boden des Hauptstacks und
    /// `TICK_STACK_BYTES` bleibt — die Zahlen des F401 an bdeae801.
    #[test]
    fn the_room_of_an_image_is_what_it_leaves() {
        assert_eq!(Room::of("Flash", 196_608, 194_400).free, 2208);
        assert_eq!(Room::of("Flash", 196_608, 200_000).free, 0);
        let ram = Room::below_stack(0x2000_0000, 0x2001_0000, 0x2000_28c0, 2652);
        assert_eq!((ram.size, ram.free), (65_536, 52_452));
        let symbols =
            [takt_llvm::inspect::Symbol { name: "_stack_start".into(), address: 0x2001_0000, size: 0, kind: 'A' }];
        assert_eq!(symbol(&symbols, "_stack_start"), Ok(0x2001_0000));
        assert!(symbol(&symbols, "_stack_end").is_err_and(|e| e.contains("_stack_end")));
    }

    /// **Ein Lauf des Programms braucht Marke, Bilanz und alle Ticks**
    /// (KON1-010): Ohne Marke und Bilanz, oder mit einem Trace, der vor dem
    /// letzten Tick endet, ist er unvollstaendig; ein Programm, das selbst
    /// endet (12.7), ein Schlaf bis zum Ende und eine Zeitzeile je zwanzig
    /// Ticks (Tick unter einer Millisekunde) reichen.
    #[test]
    fn a_program_run_needs_the_mark_the_count_and_every_tick() {
        let options = Options::fresh(10);
        let times = |ticks: std::ops::Range<u64>| -> String {
            ticks.map(|k| format!("t={k} time took=0 drift=0 slept=0\r\n")).collect()
        };
        assert!(complete("t=0 out a 1\r\ntakt end\r\n".to_string(), &options).is_err_and(|e| e.contains("Bilanz")));
        // Die Zeitzeilen nennen die Ticks des Traces, 1 bis 10 (FB-687).
        let body = times(1..11);
        assert!(complete(run(&body, 0, body.len()), &options).is_ok());
        let short = times(1..7);
        assert!(complete(run(&short, 0, short.len()), &options).is_err_and(|e| e.contains("bis Tick 6")));
        let ended = format!("{short}t=6 end now\r\n");
        assert!(complete(run(&ended, 0, ended.len()), &options).is_ok());
        let slept = "t=1 time took=0 drift=0 slept=0\r\nt=2 time took=0 drift=0 slept=8\r\n";
        assert!(complete(run(slept, 0, slept.len()), &options).is_ok());
        let sparse: String = (1..4).map(|k| format!("t={} time took=0 drift=0 slept=0\r\n", k * 20)).collect();
        assert!(complete(run(&sparse, 0, sparse.len()), &Options::fresh(79)).is_ok());
        assert!(complete(run(&sparse, 0, sparse.len()), &Options::fresh(80)).is_err());
        assert!(complete(run("", 0, 0), &Options::timed(10)).is_ok(), "in Echtzeit ohne Zeitzeilen");
    }

    /// Was zwischen Board und Wirt verloren ging, zeigt die Bilanz: Sie
    /// zaehlt ab der Marke, auch ueber Zeilen hinweg, deren Umbruch fehlt.
    #[test]
    fn a_trace_that_lost_bytes_on_the_way_is_refused() {
        let body = "t=1 out a 1\r\nt=2 out a 2\r\n";
        let any = Options::default();
        assert!(complete(run(body, 0, body.len()), &any).is_ok());
        let lost = run("t=1 out a 1t=2 out a 2\r\n", 0, body.len());
        assert!(complete(lost, &any).is_err_and(|e| e.contains("sandte 26 Byte, angekommen sind 24")));
    }

    /// Eine Leitung, die alles annimmt.
    struct Memory<'a>(&'a std::cell::RefCell<Vec<u8>>);

    impl takt_rt_baremetal::Port for Memory<'_> {
        fn try_write(&mut self, b: u8) -> bool {
            self.0.borrow_mut().push(b);
            true
        }
    }

    /// **Die Bilanzzeile der Bring-ups geht durch den Leser des Wirts**
    /// (12.5, 13.8; RT-035): `takt_rt_baremetal::report` schreibt sie, wie
    /// ein Board sie sendet; `complete` nimmt den Lauf an, und [`counter`]
    /// liest jede Kennzahl, die die Board-Tests lesen, mit ihrem Wert. Benennt
    /// `report` ein Wort um, faellt das hier auf und nicht erst am Board.
    #[test]
    fn the_balance_line_of_a_board_reads_back_on_the_host() {
        let line = std::cell::RefCell::new(Vec::new());
        let mut t = takt_rt_baremetal::Telemetry::new(Memory(&line), vec![0; 4096].leak());
        t.write("takt auf wirt\r\n");
        t.mark();
        for k in 1..5 {
            t.write(&format!("t={k} time took=0 drift=0 slept=0\r\n"));
        }
        let mut overrun = takt_rt_core::Overrun::new(takt_rt_core::Policy::Fault);
        (overrun.late, overrun.lost, overrun.worst_drift) = (1, 2, 2_500_000);
        let stats = takt_rt_baremetal::Stats { slept: 9, overruns: 3, flushed: true, next_run: None };
        let journal = takt_rt_baremetal::JournalStats { writes: 4, failures: 1, erase_ns: 12, program_ns: 5 };
        let stacks = takt_rt_baremetal::Stacks {
            tick: Some(2048),
            tick_bound: Some(4096),
            tick_program: Some(120),
            job: Some(640),
            job_bound: Some(1056),
            job_shared: false,
        };
        takt_rt_baremetal::report(&mut t, &overrun, &stats, &journal, &stacks);
        let text = String::from_utf8(line.into_inner()).expect("ASCII");
        let text = complete(text, &Options::fresh(4)).unwrap_or_else(|e| panic!("{e}"));
        let body = "t=1 time took=0 drift=0 slept=0\r\n".len() * 4;
        let counters = [
            ("schlief", 9),
            ("ueberlaeufe", 3),
            ("verspaetet", 1),
            ("verloren", 2),
            ("rueckstand", 2_500_000),
            ("verworfen", 0),
            ("gesendet", body as u64),
            ("journal geschrieben", 4),
            ("fehlgeschlagen", 1),
            ("flush", 1),
            ("nvm loeschen", 12),
            ("programmieren", 5),
            ("stack", 2048),
            ("schranke", 4096),
            ("programm", 120),
            ("jobstack", 640),
            ("jobschranke", 1056),
        ];
        for (label, want) in counters {
            assert_eq!(counter(&text, label), Some(want), "`{label}` in:\n{text}");
        }
    }

    /// Fehlt die Marke oder die Bilanz, ist nichts zu zaehlen — und das
    /// ist selbst ein Befund, solange die andere Haelfte da ist.
    #[test]
    fn a_trace_needs_both_the_mark_and_the_count() {
        let body = "t=1 out a 1\r\n";
        let unmarked = run(body, 0, body.len()).replace("takt trace", "takt trce");
        assert!(complete(unmarked, &Options::default()).is_err_and(|e| e.contains("fehlt `takt trace`")));
        let uncounted = run(body, 0, body.len()).replace("gesendet", "gesndet");
        assert!(complete(uncounted, &Options::default()).is_err_and(|e| e.contains("fehlt die Bilanz")));
    }

    /// Ein Verzeichnis fuer einen Test im Zielverzeichnis, je Prozess und Name.
    fn scratch(name: &str) -> PathBuf {
        let dir = crate::target_dir().join(format!("takt-board-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        dir
    }

    /// **Eine Aenderung an einer importierten Konfiguration baut neu**
    /// (KON1-009): Der Schluessel eines Abbilds haengt an den Dateien, die
    /// das Programm per `import channels` liest (8.2), auf dem Board wie auf
    /// dem Wirt.
    #[test]
    fn the_key_follows_the_imported_channels() {
        let dir = scratch("imports");
        let program = dir.join("recorded.takt");
        std::fs::write(&program, "import channels from \"recorded.hw\"\n").expect("Programm");
        std::fs::write(dir.join("recorded.hw"), "a\n").expect("Konfiguration");
        let bringup = Bringup { dir: "crates/takt-bringup-host", triple: "x", inputs: &[], env: &[] };
        let host = host::Host::new();
        let options = Options::fresh(10);
        let (board_before, host_before) =
            (bringup.key(&program, &options).expect("Schluessel"), host.key(&program, &options).expect("Schluessel"));
        std::fs::write(dir.join("recorded.hw"), "b\n").expect("Konfiguration");
        let (board_after, host_after) =
            (bringup.key(&program, &options).expect("Schluessel"), host.key(&program, &options).expect("Schluessel"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(board_before, board_after, "das Board baute mit der alten Konfiguration");
        assert_ne!(host_before, host_after, "der Wirt baute mit der alten Konfiguration");
    }

    /// **Stand und Schluessel haengen am Inhalt, nicht an Zeilenenden oder
    /// Ort** (M11 Schritt 34d): Derselbe Baum mit CRLF und mit LF, an zwei
    /// Orten, ergibt denselben Fingerabdruck, ebenso das Programm darin; ein
    /// anderer Inhalt einen anderen. Sonst faende ein Lauf unter Windows die
    /// Abbilder eines Runners unter Linux nicht.
    #[test]
    fn the_fingerprint_follows_the_content_not_the_line_ends() {
        let tree = |name: &str, eol: &str, tick: u32| -> PathBuf {
            let dir = scratch(name);
            std::fs::create_dir_all(dir.join("sub")).expect("Verzeichnis");
            std::fs::write(dir.join("a.rs"), format!("fn a() {{{eol}    {tick}{eol}}}{eol}")).expect("Datei");
            let program = format!("system:{eol}    language = 1{eol}    tick     = {tick} ms{eol}");
            std::fs::write(dir.join("sub/b.takt"), program).expect("Programm");
            dir
        };
        let stand = |dir: &Path| {
            let mut f = Fingerprint::new();
            hash_tree(dir, &mut f).expect("Baum");
            f.finish()
        };
        let bringup = Bringup { dir: "crates/takt-bringup-host", triple: "x", inputs: &[], env: &[] };
        let key = |dir: &Path| bringup.key(&dir.join("sub/b.takt"), &Options::fresh(10)).expect("Schluessel");
        let dirs = [tree("lf", "\n", 1), tree("crlf", "\r\n", 1), tree("other", "\n", 2)];
        let stands = dirs.each_ref().map(|d| stand(d));
        let keys = dirs.each_ref().map(|d| key(d));
        for dir in &dirs {
            let _ = std::fs::remove_dir_all(dir);
        }
        assert_eq!(stands[0], stands[1], "derselbe Baum mit CRLF und mit LF");
        assert_ne!(stands[0], stands[2], "ein anderer Inhalt");
        assert_eq!(keys[0], keys[1], "dasselbe Programm mit CRLF und mit LF");
        assert_ne!(keys[0], keys[2], "ein anderes Programm");
    }

    /// Der Fingerabdruck ist SHA-256, die ersten acht Byte in der Folge der
    /// Hexziffern: Ueber leeren Eingaben `e3b0c442…`.
    #[test]
    fn the_fingerprint_is_the_head_of_sha256() {
        assert_eq!(format!("{:016x}", Fingerprint::new().finish()), "e3b0c44298fc1c14");
    }

    /// **Ein abgebrochenes Kopieren hinterlaesst kein gueltiges Abbild**
    /// (KON1-009): Das Abbild entsteht unter einem Zwischennamen und erhaelt
    /// seinen Namen erst, wenn es ganz geschrieben ist.
    #[test]
    fn an_image_gets_its_name_only_when_complete() {
        let dir = scratch("publish");
        let (from, to) = (dir.join("built.elf"), dir.join("cached.elf"));
        std::fs::write(&from, [7u8; 64]).expect("Abbild");
        assert_eq!(publish_checked(&from, &to, |_| Ok(true)), Ok(true));
        let names: Vec<String> = std::fs::read_dir(&dir)
            .expect("lesbar")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into())
            .collect();
        let copied = std::fs::read(&to).expect("Abbild");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(copied, [7u8; 64]);
        assert_eq!(names.len(), 2, "kein Zwischenstand bleibt liegen: {names:?}");
    }

    /// **Ein fremdes Binary bekommt keinen Namen, ein veroeffentlichtes
    /// bleibt** (KON1-009 fuer zwei Prozesse): Hat ein anderer Prozess das
    /// gebaute Binary ueberschrieben, faellt die Kopie durch die Pruefung,
    /// und nichts liegt danach im Zwischenspeicher. Hat er denselben
    /// Schluessel schon veroeffentlicht, bleibt sein Abbild unberuehrt.
    #[test]
    fn a_foreign_binary_is_not_published_and_a_published_one_stays() {
        let dir = scratch("publish-checked");
        let (from, to) = (dir.join("built.exe"), dir.join("cached.exe"));
        std::fs::write(&from, b"key B").expect("Abbild");
        let ours =
            |part: &Path| -> Result<bool, String> { Ok(std::fs::read(part).map_err(|e| e.to_string())? == b"key A") };
        assert_eq!(publish_checked(&from, &to, ours), Ok(false));
        let names = |dir: &Path| -> Vec<String> {
            std::fs::read_dir(dir).expect("lesbar").flatten().map(|e| e.file_name().to_string_lossy().into()).collect()
        };
        assert_eq!(names(&dir), ["built.exe"], "weder Abbild noch Zwischenstand");
        std::fs::write(&from, b"key A").expect("Abbild");
        std::fs::write(&to, b"key A, schon da").expect("Abbild");
        assert_eq!(publish_checked(&from, &to, ours), Ok(true));
        let kept = std::fs::read(&to).expect("Abbild");
        let left = names(&dir).len();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(kept, b"key A, schon da");
        assert_eq!(left, 2, "kein Zwischenstand bleibt liegen");
    }

    /// Jede Zeile des Rands faellt in ihre Zeile der Tabelle; Erholung,
    /// Ausgaben und ein Strom, dessen `malformed` nicht steigt, sind keine
    /// Verstoesse.
    #[test]
    fn every_edge_line_falls_into_its_row() {
        let trace = "t=1 driver edge_a warped p\nt=2 driver edge_a degraded window\nt=3 driver edge_a recovered\n\
                     t=4 runtime Driver o\nt=4 fault actor Runtime(Driver)\nt=5 driver edge_u degraded seq\n\
                     t=6 stream pairs dropped=0 overflowed=0 malformed=1\nt=7 stream pairs dropped=1 overflowed=0 malformed=1\n\
                     t=8 out up 1 \nt=9 runtime Hardware\n";
        let found = violations(trace);
        let rows: Vec<(u8, &str)> = found.iter().map(|v| (v.row, &v.line[..3])).collect();
        assert_eq!(rows, [(1, "t=1"), (1, "t=2"), (6, "t=4"), (2, "t=5"), (5, "t=6"), (7, "t=9")]);
    }

    /// **Was vor dem Start ankommt, gehoert dem vorigen Abbild** (FB-437):
    /// verworfen bis zur ersten Stille; ein Fehler der Leitung bleibt einer.
    #[test]
    fn stale_bytes_are_discarded_until_the_line_is_quiet() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(b"t=819 out previous WATCHDOG\r\n".to_vec())).unwrap();
        assert_eq!(discard_stale(&rx), Ok(()));
        assert!(rx.try_recv().is_err(), "nichts bleibt fuer den Lauf");
        tx.send(Err("weg".to_string())).unwrap();
        assert_eq!(discard_stale(&rx), Err("weg".to_string()));
    }
}
