//! Board 2, ESP32-C6: der Korpus auf dem Chip gegen den Interpreter
//! (13.8, Satz 9.4.4; plan/esp32c6.md Schritt 5).
//!
//! **Laeuft nur mit Board.** `TAKT_ESP32C6_PORT` nennt den seriellen Port
//! des USB-Serial-JTAG (`COM4`, `/dev/ttyACM0`); ohne ihn wird der Test
//! uebersprungen, wie die LLVM-Tests ohne clang. Je Programm: das
//! Bring-up-Binary mit `TAKT_PROGRAM` und `TAKT_TICKS` bauen, mit
//! `probe-rs` flashen, zuruecksetzen, den Trace bis `takt end` lesen und
//! wie im Linux-Vergleich pruefen.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use takt_conformance::compare;
use takt_mir::program::Program;

const TICKS: u64 = 60;

/// Ein Board, drei Tests: cargo fuehrt Tests nebenlaeufig aus, das Board
/// und sein Port vertragen nur einen Lauf zugleich.
static BOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn board() -> std::sync::MutexGuard<'static, ()> {
    BOARD.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Der Differentialkorpus ohne das, was der MCU-Rahmen nicht traegt:
/// Systemkanaele (32, 34), geplante Ausgaben (28) und Jobs (40).
const KORPUS: &[&str] = &[
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
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus(name: &str) -> Program {
    let path = root().join("corpus-try").join(name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options =
        takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

fn run_interpreted(p: &Program) -> String {
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// Baut das Bring-up-Binary fuer `name` und liefert den Pfad des ELF;
/// `fresh` loescht das Journal vor dem Lauf (wie der Interpreter ohne
/// Speicher), sonst laedt der Lauf, was der vorige schrieb.
fn build(name: &str, fresh: bool) -> Result<PathBuf, String> {
    let manifest = root().join("crates/takt-bringup-esp32c6/Cargo.toml");
    let program = root().join("corpus-try").join(name);
    let mut cargo = Command::new("cargo");
    cargo
        .args(["build", "--release", "--target", "riscv32imac-unknown-none-elf", "--bin", "takt"])
        .arg("--message-format=json-render-diagnostics")
        .arg("--manifest-path")
        .arg(&manifest)
        .env("TAKT_PROGRAM", &program)
        .env("TAKT_TICKS", TICKS.to_string())
        // 12.3: RAM-Residenz des Takt-Programms. `.cargo/config.toml` des
        // Bring-ups greift hier nicht, weil `--manifest-path` von aussen baut.
        .env("ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK", "true");
    if fresh {
        cargo.env("TAKT_FRESH_JOURNAL", "1");
    } else {
        cargo.env_remove("TAKT_FRESH_JOURNAL");
    }
    let out = cargo.output().map_err(|e| format!("cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    // Die JSON-Meldungen nennen das Binary; ein eigener Parser fuer eine
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

fn probe_rs(args: &[&str]) -> Result<(), String> {
    let out = Command::new("probe-rs").args(args).output().map_err(|e| format!("probe-rs: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
}

/// Setzt das Board zurueck und liest den Trace bis `takt end`.
///
/// Kommt gar nichts, nicht einmal der ROM-Bootloader, hat der
/// USB-Serial-JTAG nach dem Flashen noch nicht wieder angelegt; ein
/// zweiter Reset genuegt dann. Ein Lauf, der etwas sagt und nicht endet,
/// wird nicht wiederholt — das waere ein Befund.
fn capture(port: &str) -> Result<String, String> {
    for attempt in 0..2 {
        // Nach dem Flashen legt der USB-Serial-JTAG neu an; ein Handle von
        // davor liefert nichts. Darum kurz warten und je Versuch neu oeffnen.
        std::thread::sleep(Duration::from_millis(500));
        let mut serial = serialport::new(port, 115_200)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| format!("{port}: {e}"))?;
        probe_rs(&["reset", "--chip", "esp32c6"])?;
        let start = Instant::now();
        let mut text = String::new();
        let mut buf = [0u8; 4096];
        while start.elapsed() < Duration::from_secs(30) {
            match serial.read(&mut buf) {
                Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(format!("{port}: {e}")),
            }
            if text.contains("takt end") {
                return Ok(text);
            }
        }
        if !text.is_empty() || attempt == 1 {
            return Err(format!("kein `takt end` binnen 30 s; gelesen:\n{text}"));
        }
    }
    unreachable!("zwei Versuche")
}

/// Die Namen der Ausgaenge, die ein Trace nennt.
fn output_names(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out").then(|| w.next().map(String::from))?
        })
        .collect()
}

/// Der letzte `out`-Wert eines Ausgangs im Trace.
fn last_output(text: &str, name: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            w.next()?.strip_prefix("t=")?;
            (w.next()? == "out" && w.next()? == name).then(|| w.collect::<Vec<_>>().join(" "))
        })
        .next_back()
}

/// Die Zahl hinter einem Wort der Abschlusszeile (`takt schlief 0
/// ueberlaeufe 0 journal geschrieben 3 …`).
fn counter(text: &str, label: &str) -> Option<u64> {
    let line = text.lines().find(|l| l.starts_with("takt schlief "))?;
    let rest = line.split_once(label)?.1;
    rest.split_whitespace().next()?.parse().ok()
}

/// Die Zahl hinter `takt schlief`.
fn slept(text: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix("takt schlief ")?.split_whitespace().next()?.parse().ok())
}

/// **`persist` ueberlebt einen Reset** (5.9, 12.3; plan/esp32c6.md 6).
///
/// `35_persist` zaehlt `cycles` hoch; der Lauf schreibt das Journal am
/// Ende (`flush`). Ein zweiter Lauf desselben Abbilds nach einem Reset —
/// ohne frisches Journal — muss mit dem letzten Stand des ersten beginnen.
#[test]
fn persistence_survives_a_reset() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "35_persist.takt";
    let first = build(name, false)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let end = last_output(&first, "count").unwrap_or_else(|| panic!("kein `count` im ersten Lauf:\n{first}"));
    assert!(first.contains("flush 1"), "das Journal wurde am Ende nicht geschrieben:\n{first}");
    // Der zweite Lauf: derselbe Chip, ein Reset, das Journal bleibt.
    let second = capture(&port).unwrap_or_else(|e| panic!("{name}, zweiter Lauf: {e}"));
    let start = second
        .lines()
        .find_map(|l| l.strip_prefix("t=0 out count "))
        .map(str::trim)
        .unwrap_or_else(|| panic!("kein `count` bei t=0 im zweiten Lauf:\n{second}"));
    assert_eq!(start, end.trim(), "der zweite Lauf beginnt nicht mit dem Stand des ersten:\n{second}");
    assert!(second.contains("journal: Eintrag"), "das Journal wurde nicht geladen:\n{second}");
}

/// **`idle` schlaeft** (9.9, Satz 9.9.1; plan/esp32c6.md 6):
/// `56_idle_timer` verbringt 500 von 700 ms im Schlaf, mit dem Timer als
/// einziger Wake-Quelle; die Schleife zaehlt die Ticks dazwischen als
/// virtuelle, und der Trace bleibt der des Interpreters.
#[test]
fn an_idle_state_sleeps_in_virtual_ticks() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "56_idle_timer.takt";
    let text = build(name, true)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let slept = slept(&text).unwrap_or_else(|| panic!("keine Schlafzeile:\n{text}"));
    assert!(slept >= 40, "60 Ticks mit 500 ms `idle` bei 10 ms Tick: mehr als 40 virtuelle erwartet, {slept}:\n{text}");
    let p = corpus(name);
    let diffs = compare(&run_interpreted(&p), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

/// **Das Journal kostet Zeit, aber keine Semantik** (12.3, `xip_flash`;
/// plan/esp32c6.md Schritt 7, FB-197).
///
/// `57_persist_often` schreibt in jedem Tick (`min_interval = 0`). Eine
/// Sektorloeschung dauert auf diesem Chip rund 25 ms und ist unteilbar:
/// Bei 10 ms Tick vergehen dabei Perioden, gleich wo der Code liegt. Was
/// Schritt 7 erreicht, ist darum nicht ihre Abwesenheit, sondern dass sie
/// die *logische* Zeit nicht beruehren — der Trace bleibt der des
/// Interpreters (Satz 9.4.4), und die Tickzahl kommt aus dem SYSTIMER,
/// nicht aus einem ISR-Zaehler (FB-198).
///
/// Die verpassten Perioden zaehlt der Test und schreibt sie hin, statt
/// sie zuzusichern: Sie sind eine Eigenschaft des Flash, kein Ergebnis
/// des Compilers. Gemessen am 17.09.2026: 400 mit RAM-Residenz, 429 ohne,
/// bei 16 Schreibvorgaengen.
#[test]
fn the_journal_costs_time_but_not_semantics() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    let name = "57_persist_often.takt";
    let text = build(name, true)
        .and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
    let writes = counter(&text, "journal geschrieben").unwrap_or_else(|| panic!("keine Journalzeile:\n{text}"));
    assert!(writes > 1, "das Journal schrieb nur {writes}-mal; der Fall aus 12.3 trat nicht ein:\n{text}");
    assert_eq!(counter(&text, "fehlgeschlagen"), Some(0), "ein Schreibvorgang scheiterte:\n{text}");
    let missed =
        text.lines().filter_map(|l| l.trim().strip_prefix("verpasste Ticks: ")?.parse::<u64>().ok()).next_back();
    eprintln!("{name}: {writes} Journal-Schreibvorgaenge, {} verpasste Perioden", missed.unwrap_or(0));
    let p = corpus(name);
    let diffs = compare(&run_interpreted(&p), &text);
    assert!(diffs.is_empty(), "{diffs:?}");
}

#[test]
fn the_board_agrees_with_the_interpreter() {
    let Ok(port) = std::env::var("TAKT_ESP32C6_PORT") else {
        eprintln!("uebersprungen: TAKT_ESP32C6_PORT nennt kein Board");
        return;
    };
    let _board = board();
    // `TAKT_ESP32C6_ONLY=42_map.takt` fuer einen einzelnen Fall.
    let only = std::env::var("TAKT_ESP32C6_ONLY").ok();
    let mut failed = Vec::new();
    for name in KORPUS.iter().filter(|n| only.as_deref().is_none_or(|o| o == **n)) {
        let p = corpus(name);
        let board = match build(name, true).and_then(|elf| {
            probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()])?;
            capture(&port)
        }) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein Lauf auf dem Board:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
        let missing: Vec<String> = output_names(&interpreted).difference(&output_names(&board)).cloned().collect();
        let diffs = compare(&interpreted, &board);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- Board ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                board.lines().filter(|l| l.starts_with("t=")).take(12).collect::<Vec<_>>().join("\n")
            ));
        }
        eprintln!("{name}: {} Abweichungen", diffs.len());
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
