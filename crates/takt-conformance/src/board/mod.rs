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
//! | STM32F401 ([`stm32f401`]) | `dfu-util` ueber den DFU-Bootloader im ROM | `TAKT` auf der Trace-Leitung: Die Anwendung springt in den Bootloader (FB-275) |

pub mod esp32c6;
pub mod stm32f401;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Der Differentialkorpus, soweit der MCU-Rahmen ihn traegt: ohne
/// Systemkanaele (32, 34), geplante Ausgaben (28), Jobs (40) und den Port
/// auf `mmio` (68), die er noch nicht kennt (plan/m10.md Schritte 5–7, 9).
pub const CORPUS: &[&str] = &[
    "01_minimal.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "19_faults.takt",
    "20_native.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
    "23_patterns.takt",
    "24_send_has.takt",
    "25_format.takt",
    "26_samples.takt",
    "27_every.takt",
    "33_enum_param.takt",
    "35_persist.takt",
    "36_int_units.takt",
    "37_follows.takt",
    "39_sha256.takt",
    "41_tunables.takt",
    "42_map.takt",
    "43_sent.takt",
    "45_journal_cut.takt",
    "46_matrices.takt",
    "47_monitors.takt",
    "49_record_streams.takt",
    "50_clause_words.takt",
    "51_text_into_bytes.takt",
    "52_padding_fields.takt",
    "53_stream_kinds.takt",
    "54_inout.takt",
    "55_frames_with_bytes.takt",
    "56_idle_timer.takt",
    "57_persist_often.takt",
    "58_persist_alert.takt",
    "59_persist_idle.takt",
    "60_resume.takt",
    "62_type_generics.takt",
    "63_scoped_instances.takt",
    "64_scoped_exit.takt",
    "69_qp_box.takt",
    "70_padded_record.takt",
    "71_places.takt",
    "72_handler_levels.takt",
    "73_after_levels.takt",
    "74_instance_index.takt",
    "75_implicit_checks.takt",
    "76_stream_views.takt",
    "77_float_faults.takt",
    "78_length_guards.takt",
    "79_byte_literals.takt",
    "80_payload_variants.takt",
    "81_persist_variants.takt",
];

/// Die Zeile, mit der jedes Bring-up seinen Lauf beendet.
pub const END: &str = "takt end";

/// Ein Trace mit Luecken ist keiner: Verwirft die Telemetrie des Boards
/// Bytes (`takt schlief … verworfen N`, 12.2), meldete der Vergleich jede
/// Zeile hinter der Luecke als Abweichung (FB-292).
pub(crate) fn complete(text: String) -> Result<String, String> {
    let dropped = text.lines().find(|l| l.contains("takt schlief ")).and_then(|line| {
        let mut words = line.split_whitespace();
        words.by_ref().find(|w| *w == "verworfen")?;
        words.next()?.parse::<u64>().ok()
    });
    match dropped {
        Some(n) if n > 0 => Err(format!("Trace unvollstaendig: das Board verwarf {n} Byte (FB-292)")),
        _ => Ok(text),
    }
}

/// Welches Programm des Bring-ups das Takt-Programm bindet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bin {
    /// `takt`: das Programm unter der Tickschleife, mit Trace.
    Takt,
    /// `bench`: das Programm als Messkern von `takt bench` (13.8), mit
    /// einer C-Referenz, wenn eine genannt ist.
    Bench {
        /// Die C-Datei mit `takt_bench_reference` und
        /// `takt_bench_reference_digest`.
        reference: Option<PathBuf>,
    },
    /// `natives`: die Vektoren der kuratierten Natives mit dem
    /// Stack-Bedarf je Aufruf (13.8); das Takt-Programm bleibt ungenutzt.
    Natives,
}

/// Wie ein Programm auf das Board kommt.
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

impl Options {
    /// Ein Konformitaetslauf ueber `ticks` Ticks mit leerem Journal.
    pub fn fresh(ticks: u64) -> Options {
        Options { ticks, fresh: true, bin: Bin::Takt, timed: false }
    }

    /// Ein Lauf ueber `ticks` Ticks in Echtzeit, fuer das, was nur die
    /// Uhr zeigt: Tick-Jitter und Stack unter Last (13.8).
    pub fn timed(ticks: u64) -> Options {
        Options { ticks, fresh: true, bin: Bin::Takt, timed: true }
    }

    /// Ein Messkern mit `runs` Messungen und seiner C-Referenz.
    pub fn bench(runs: u64, reference: Option<PathBuf>) -> Options {
        Options { ticks: runs, fresh: true, bin: Bin::Bench { reference }, timed: false }
    }

    /// Die Vektoren der kuratierten Natives.
    pub fn natives() -> Options {
        Options { ticks: 0, fresh: false, bin: Bin::Natives, timed: false }
    }
}

/// Ein Board am Host.
pub trait Board {
    /// Der Name in Meldungen und Berichten.
    fn name(&self) -> &'static str;

    /// Die Zielklasse, wie `takt build --target` sie nennt (12.8).
    fn target(&self) -> &'static str;

    /// Baut das Bring-up mit `program` und liefert das ELF; das Abbild
    /// kommt aus dem Zwischenspeicher, wenn es dort liegt.
    fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String>;

    /// Schreibt das Abbild, startet es und liest den Trace bis [`END`].
    fn run(&mut self, elf: &Path, options: &Options) -> Result<String, String>;
}

/// Die Wurzel des Repositorys.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Ein Programm des Differentialkorpus.
pub fn corpus_path(name: &str) -> PathBuf {
    root().join("corpus-try").join(name)
}

/// Das Bring-up eines Boards, wie `cargo` es baut.
pub(crate) struct Bringup {
    /// Das Crate, relativ zur Wurzel.
    pub dir: &'static str,
    /// Das Zieltripel.
    pub triple: &'static str,
    /// Dateien neben dem Manifest, die der Bau liest.
    pub inputs: &'static [&'static str],
    /// Umgebung des Baus, die `.cargo/config.toml` des Bring-ups sonst
    /// setzt: Von aussen gebaut greift jene Datei nicht.
    pub env: &'static [(&'static str, &'static str)],
}

impl Bringup {
    /// Das ELF fuer `program`, aus dem Zwischenspeicher oder frisch gebaut.
    ///
    /// Der Bau kostet einige Sekunden je Programm und ergibt bei gleichen
    /// Eingaben dasselbe Abbild (FB-270). Der Speicher liegt unter
    /// `takt-board-images` im Temp-Verzeichnis; loeschen erzwingt den Bau.
    pub fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        let cache = std::env::temp_dir().join("takt-board-images");
        let cached = cache.join(format!("{:016x}.elf", self.key(program, options)?));
        if cached.is_file() {
            return Ok(cached);
        }
        let elf = self.build_uncached(program, options)?;
        std::fs::create_dir_all(&cache)
            .and_then(|()| std::fs::copy(&elf, &cached))
            .map_err(|e| format!("{}: {e}", cached.display()))?;
        Ok(cached)
    }

    /// Der Schluessel eines Abbilds: das Programm, die Optionen, die
    /// C-Referenz und alles, was der Bau liest — die Quellen aller Crates
    /// und die Dateien des Bring-ups. Aendert sich nichts davon, ist das
    /// Abbild dasselbe.
    fn key(&self, program: &Path, options: &Options) -> Result<u64, String> {
        let mut h = DefaultHasher::new();
        std::fs::read(program).map_err(|e| format!("{}: {e}", program.display()))?.hash(&mut h);
        (options.ticks, options.fresh, options.timed, self.triple).hash(&mut h);
        match &options.bin {
            Bin::Takt => 0u8.hash(&mut h),
            Bin::Bench { reference } => {
                1u8.hash(&mut h);
                if let Some(c) = reference {
                    std::fs::read(c).map_err(|e| format!("{}: {e}", c.display()))?.hash(&mut h);
                }
            }
            // Die Vektoren stehen in der Spezifikation, nicht in `src`.
            Bin::Natives => {
                2u8.hash(&mut h);
                let spec = crate::natives::spec_path();
                std::fs::read(&spec).map_err(|e| format!("{}: {e}", spec.display()))?.hash(&mut h);
            }
        }
        let dir = root().join(self.dir);
        for name in self.inputs {
            std::fs::read(dir.join(name)).unwrap_or_default().hash(&mut h);
        }
        let crates = root().join("crates");
        let mut sources: Vec<PathBuf> =
            std::fs::read_dir(&crates).map_err(|e| e.to_string())?.flatten().map(|e| e.path().join("src")).collect();
        sources.sort();
        for dir in sources.iter().filter(|d| d.is_dir()) {
            hash_tree(dir, &mut h)?;
        }
        Ok(h.finish())
    }

    /// `cargo build` des Bring-ups mit dem Programm.
    fn build_uncached(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        let bin = match options.bin {
            Bin::Takt => "takt",
            Bin::Bench { .. } => "bench",
            Bin::Natives => "natives",
        };
        let mut cargo = Command::new("cargo");
        cargo
            .args(["build", "--release", "--target", self.triple, "--bin", bin])
            .arg("--message-format=json-render-diagnostics")
            .arg("--manifest-path")
            .arg(root().join(self.dir).join("Cargo.toml"))
            .env("TAKT_PROGRAM", program)
            .env("TAKT_TICKS", options.ticks.to_string())
            .envs(self.env.iter().copied());
        if options.fresh {
            cargo.env("TAKT_FRESH_JOURNAL", "1");
        } else {
            cargo.env_remove("TAKT_FRESH_JOURNAL");
        }
        if options.timed {
            cargo.env("TAKT_TIMED", "1");
        } else {
            cargo.env_remove("TAKT_TIMED");
        }
        match &options.bin {
            Bin::Bench { reference: Some(c) } => cargo.env("TAKT_BENCH_C", c),
            _ => cargo.env_remove("TAKT_BENCH_C"),
        };
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
}

/// Namen und Inhalte eines Verzeichnisbaums, in fester Ordnung.
fn hash_tree(dir: &Path, h: &mut DefaultHasher) -> Result<(), String> {
    let mut entries: Vec<PathBuf> =
        std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        path.file_name().hash(h);
        if path.is_dir() {
            hash_tree(&path, h)?;
        } else {
            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?.hash(h);
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
/// Ohne [`END`] ist das Ergebnis der Text, der bis dahin kam: Ob das ein
/// Befund ist, entscheidet der Aufrufer.
pub(crate) fn capture(
    port: &str,
    baud: u32,
    within: Duration,
    start: impl FnOnce() -> Result<(), String>,
) -> Result<String, String> {
    let mut serial =
        serialport::new(port, baud).timeout(Duration::from_millis(200)).open().map_err(|e| format!("{port}: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = std::thread::spawn({
        let stop = Arc::clone(&stop);
        move || {
            let mut buf = [0u8; 4096];
            while !stop.load(Ordering::Relaxed) {
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
    let started = start();
    let deadline = Instant::now() + within;
    let mut text = String::new();
    let result = started.and_then(|()| {
        loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(bytes)) => {
                    text.push_str(&String::from_utf8_lossy(&bytes));
                    if text.contains(END) {
                        break Ok(());
                    }
                }
                Ok(Err(e)) => break Err(format!("{port}: {e}")),
                Err(_) => break Ok(()),
            }
        }
    });
    stop.store(true, Ordering::Relaxed);
    let until = Instant::now() + Duration::from_secs(1);
    while !reader.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    result.map(|()| text)
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

    /// Verworfene Bytes machen den Lauf unbrauchbar, ein vollstaendiger
    /// und einer ohne Abschlusszeile gehen durch.
    #[test]
    fn a_trace_with_dropped_bytes_is_refused() {
        let summary = |n: u32| format!("t=1 out a 1\ntakt schlief 0 ueberlaeufe 0 verworfen {n} journal 0\ntakt end\n");
        assert!(complete(summary(25076)).is_err_and(|e| e.contains("25076")));
        assert!(complete(summary(0)).is_ok());
        assert!(complete("bench takt min 1\ntakt end\n".to_string()).is_ok());
    }
}
