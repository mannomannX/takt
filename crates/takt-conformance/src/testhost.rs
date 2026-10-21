//! Der Testwirt des Differentials (13.8, M11 Schritt 9): der erzeugte Code im
//! Produktrahmen auf dem Wirt, gefahren vom Kern des C-SDK.
//!
//! **Ein Rahmen fuer alles.** Was das Differential gegen den Interpreter
//! haelt, ist dasselbe, was die Boards und jeder Wirt binden: der Rahmen aus
//! `takt_frame::mcu`, die Tabelle `P_ops`, der Kopf `P.h` und das SDK-Archiv
//! mit dem Kern ohne Warten (12.11). Ein Fehler im Produktrahmen faellt so
//! schon hier auf und nicht erst am Board (plan/m11.md 1.6).
//!
//! **Der Stimulus ist ein Testtreiber.** Was eine Zeile des Stimulus
//! liefert, kommt ueber den Weg, den die Referenz dafuer vorsieht:
//!
//! | Zeile | Weg |
//! |---|---|
//! | `in` an einen Skalar | `P_in_<adr>` liefert Wert, Qualitaet, Grund, Alter, Zeitstempel (12.6) |
//! | `in` an einen Eingang zusammengesetzten Typs | `P_in_<adr>` liefert die Bytes in Speicherform |
//! | `in` an einen Eingabestrom | `P_poll_<adr>` liefert je Aufruf ein Element (8.6) |
//! | `cmd` | `P_cmd_<name>` meldet das Command an der Grenze seines Ticks (8.5) |
//! | `tune` | die Tunables des Wirts (`takt_core_service_with`, 8.4) |
//! | `abort` | `takt_core_abort` vor dem Aufruf, der den Tick rechnet (5.4) |
//! | `runtime Hardware` | `takt_core_hardware_fault` ebenso (12.3) |
//! | `runtime Driver o` | der Treiber von `o` bestaetigt den Commit des Ticks davor nicht (12.6 Zeile 6) |
//! | `runtime Overrun` | der Wirt ruft den Tick davor mehr als eine Periode zu spaet (7.3) |
//!
//! Der Wirt ruft `takt_core_service` zu jeder Frist, die der Kern nennt, und
//! an der Grenze jedes Ticks, in dem der Stimulus etwas liefert; schlaeft der
//! Kern dort, weckt er ihn (`takt_core_wake`): Ein Ereignis vom Rand weckt
//! (9.9), wie im Interpreter, dessen Frist die naechste Stimuluszeile kennt.
//! Was keinen Weg hat — eine Lieferung an einen Kanal ohne Treiber, ein
//! Runtime-Fault ohne Quelle im Produkt —, ist ein Fehler mit Grund, kein
//! stilles Weglassen.
//!
//! **Der Wirt darf seine Umgebung verstellen** (4.2, 12.11): Ein Lauf kann
//! das Programm unter einem anderen Praefix binden, die Arena vor `init` mit
//! einem Muster fuellen und auf x86-64 MXCSR verstellen; nach jedem Aufruf
//! des SDK muss MXCSR wieder der des Wirts sein, und der Trace bleibt der
//! des Interpreters.
//!
//! **Die Leitung ist die Senke.** Den Trace schreibt der Rahmen ueber die
//! Leitung des SDK nach stdout, wie auf jedem Board: der Anfangszustand ganz,
//! danach die Aenderungen und je Tick die Zeitzeile, am Ende die Bilanz und
//! `takt end`. Ein Programm mit `persist` beginnt ueber sein Journal auf einem
//! Flash im Speicher; die Nutzlast des Laufs schreibt das Journal aus
//! `takt-rt-core` vorher hinein, und nach dem Lauf steht der Stand als Zeile
//! `persist`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use takt_interp::value::{Quality, Reason};
use takt_llvm::symbols::Prefix;
use takt_mir::program::{Binding, Direction, Program};
use takt_mir::stimulus::{self as admissible, Event};
use takt_mir::types::Type;

use crate::stimulus::Stimulus;

/// Der Kopf des C-SDK, wie die Lieferform ihn neben `P.h` legt.
const SDK_HEADER: &str = include_str!("../../takt-embed-c/include/takt_sdk.h");

/// Die Jobs aus `takt-crypto`, die der Korpus ruft (4.5): als Features des
/// SDK-Archivs.
pub const SDK_FEATURES: [&str; 3] = ["ecdsa", "rsa", "aes-gcm"];

/// Die Groesse eines Slots des Flashs im Speicher, auf dem ein Programm mit
/// `persist` sein Journal schreibt (5.9).
const SLOT: usize = 65536;

/// Ein Lauf.
#[derive(Clone, Copy, Debug, Default)]
pub struct Run<'a> {
    /// So viele Ticks nach Tick 0.
    pub ticks: u64,
    /// Was der Stimulus liefert.
    pub stimulus: &'a [Stimulus],
    /// Das Szenario, das mitschreitet (13.6).
    pub scenario: Option<&'a str>,
    /// Was das Journal vor dem Lauf haelt (5.9, kanonische Form); leer: ein
    /// geloeschtes Flash.
    pub payload: &'a [u8],
    /// Das Praefix des Programms (12.11); ohne: `app` wie die Bring-ups.
    pub prefix: Option<&'a str>,
    /// MXCSR des Wirts waehrend des Laufs (4.2), nur auf x86-64; ohne: die
    /// Vorgabe der Plattform.
    pub mxcsr: Option<u32>,
    /// Das Muster, mit dem der Wirt die Arena vor `init` fuellt (12.11).
    pub fill: u8,
    /// Die Hardware-Konfiguration, gegen die der Rahmen gebaut ist (8.10):
    /// `guard`, `jitter` und `tick_granular` je Ausgang; ohne sie die Nullen
    /// der Simulation (7.5).
    pub hardware: Option<&'a takt_mir::hardware::Hardware>,
}

/// Die Quellen eines Testwirts, je Dateiname.
#[derive(Clone, Debug)]
pub struct Sources {
    /// `app.ll`, `app_frame.c`, `app.h`, `app_ops.c`, `takt_sdk.h`, `host.c`.
    pub files: Vec<(String, String)>,
}

/// Das Praefix eines Laufs (12.11).
fn prefix(run: &Run<'_>) -> Result<Prefix, String> {
    run.prefix.map_or_else(|| Ok(Prefix::default()), Prefix::new)
}

/// Das Tripel des Wirts.
pub fn host_triple() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

/// Die Quellen des Testwirts fuer `p` und `run`, die IR fuer `triple`; mit
/// `sanitized` erzeugt der Codegen sie fuer AddressSanitizer (FB-461).
pub fn sources(p: &Program, run: &Run<'_>, triple: &str, sanitized: bool) -> Result<Sources, String> {
    let x = prefix(run)?;
    let lowered = if sanitized {
        takt_llvm::lower::program_sanitized(p, triple, &x)
    } else {
        takt_llvm::lower::program(p, triple, &x)
    };
    if !lowered.complete() {
        let missing: Vec<String> = lowered
            .skipped
            .iter()
            .map(|s| format!("{} fehlt: {}", takt_llvm::lower::Skipped::what(s), s.reason))
            .collect();
        return Err(format!("der Codegen senkt nicht alles:\n{}", missing.join("\n")));
    }
    if let Some(name) = run.scenario
        && !p.machines.iter().any(|m| m.name == name)
    {
        return Err(format!("kein Szenario `{name}`"));
    }
    let frame = takt_frame::mcu::build_with(
        p,
        takt_frame::mcu::Frame {
            prefix: x.clone(),
            scenario: run.scenario,
            hardware: run.hardware,
            ..Default::default()
        },
    );
    let header = takt_frame::embed::c_header(&frame.header, p, &x, 0, 0, frame.job_stack_bytes);
    let host = host(p, run)?;
    Ok(Sources {
        files: vec![
            (format!("{x}.ll"), lowered.ir),
            (format!("{x}_frame.c"), frame.source),
            (format!("{x}.h"), header),
            (format!("{x}_ops.c"), takt_frame::embed::c_ops(&x)),
            ("takt_sdk.h".to_string(), SDK_HEADER.to_string()),
            ("host.c".to_string(), host),
        ],
    })
}

/// Uebersetzt den Testwirt in `dir` mit `clang` und dem SDK-Archiv `sdk`,
/// faehrt ihn und liefert seinen Trace. `asan` ist der Suchpfad eines Laufs
/// unter AddressSanitizer.
pub fn run(
    clang: &Path,
    sdk: &Path,
    p: &Program,
    run: &Run<'_>,
    dir: &Path,
    asan: Option<&std::ffi::OsStr>,
) -> Result<String, String> {
    let sources = sources(p, run, host_triple(), asan.is_some())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, text) in &sources.files {
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    let x = prefix(run)?;
    let exe = dir.join(format!("wirt{}", std::env::consts::EXE_SUFFIX));
    let mut cmd = std::process::Command::new(clang);
    takt_llvm::toolchain::Clang::deterministic(&mut cmd).args(["-Wno-override-module", "-O1"]);
    if asan.is_some() {
        cmd.arg("-fsanitize=address");
    }
    let build = cmd
        .arg("-I")
        .arg(dir)
        .arg(dir.join(format!("{x}.ll")))
        .arg(dir.join(format!("{x}_frame.c")))
        .arg(dir.join(format!("{x}_ops.c")))
        .arg(dir.join("host.c"))
        .arg(sdk)
        .args(takt_llvm::toolchain::Clang::HOST_LIBS)
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| format!("{}: {e}", clang.display()))?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).into_owned());
    }
    let mut command = std::process::Command::new(&exe);
    if let Some(search) = asan {
        command.env("PATH", search);
    }
    let out = command.output().map_err(|e| format!("{}: {e}", exe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    if !out.status.success() {
        let tail: Vec<&str> = text.lines().rev().take(4).collect();
        let stderr: Vec<String> = String::from_utf8_lossy(&out.stderr).lines().take(24).map(str::to_string).collect();
        return Err(format!(
            "der Lauf brach ab ({}); zuletzt:\n{}\n{}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
            stderr.join("\n")
        ));
    }
    complete(&text, run.ticks)?;
    Ok(text)
}

/// Der Lauf reichte bis zum letzten Tick (KON1-010): Die Bilanz steht da,
/// und die Zeitzeilen decken jeden Tick — oder das Programm beendete den
/// Lauf selbst (12.7).
pub fn complete(text: &str, ticks: u64) -> Result<(), String> {
    if !text.lines().any(|l| l.trim_end() == "takt end") {
        return Err(format!("der Lauf endete ohne `takt end`:\n{text}"));
    }
    let ended = text
        .lines()
        .any(|l| l.strip_prefix("t=").and_then(|r| r.split_once(' ')).is_some_and(|(_, r)| r.starts_with("end ")));
    match reached(text) {
        _ if ended || ticks == 0 => Ok(()),
        Some(last) if last >= ticks => Ok(()),
        last => Err(format!(
            "der Lauf reichte bis Tick {} von {ticks} ohne Ende des Programms:\n{text}",
            last.map_or_else(|| "keinem".to_string(), |t| t.to_string())
        )),
    }
}

/// Der letzte Tick des Traces, bis zu dem ein Lauf reicht. Die Zeitzeile
/// `t=<k> time ... slept=<n>` deckt die Ticks `k` bis `k + n`; unter einer
/// Millisekunde Tick steht sie nur jeden `m`-ten Tick (FB-271), und der
/// groesste gemeinsame Teiler ihrer `k` ist eine obere Schranke fuer die
/// Luecke nach der letzten.
pub fn reached(text: &str) -> Option<u64> {
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

/// Eine Lieferung an einen Eingang mit Platz im Abbild, als C-Anweisungen.
struct Reading {
    /// Die Anweisung, die den Wert schreibt; keine ohne Wert.
    value: Option<String>,
    quality: u8,
    reason: u8,
    age: i64,
    t: i64,
}

/// Ein Element eines Eingabestroms.
struct Element {
    at: i64,
    bytes: Vec<u8>,
    t: i64,
    seq: Option<i64>,
}

/// Was der Testwirt aus dem Stimulus macht.
#[derive(Default)]
struct Plan {
    /// Je Adresse eines Eingangs die Lieferungen nach der Zeit ihres Ticks.
    readings: BTreeMap<String, BTreeMap<i64, Reading>>,
    /// Je Adresse eines Eingabestroms die Elemente in ihrer Reihenfolge.
    elements: BTreeMap<String, Vec<Element>>,
    /// Je Command die Zeiten seiner Ticks.
    commands: BTreeMap<String, Vec<i64>>,
    /// Je Adresse eines Ausgangs die Zeiten, zu denen sein Treiber den
    /// Commit nicht bestaetigt.
    failing: BTreeMap<String, Vec<i64>>,
    /// Die Tunables: Grenze des Kerns, Parameter, Wert in kanonischer Form.
    tunes: Vec<(u64, usize, Vec<u8>)>,
    /// Die Aufrufe des Wirts, die der Stimulus verlangt, je Zeit.
    calls: BTreeMap<i64, Call>,
    /// Der erste Aufruf: ein Ueberlauf in Tick 2 verlangt ihn zu spaet.
    start: i64,
}

/// Ein Aufruf von `takt_core_service`, den der Stimulus verlangt.
#[derive(Clone, Copy, Debug, Default)]
struct Call {
    /// Vorher `takt_core_wake`: Ein Ereignis vom Rand weckt (9.9).
    wake: bool,
    /// Vorher `takt_core_abort` (5.4).
    abort: bool,
    /// Vorher `takt_core_hardware_fault` (12.3).
    hardware: bool,
    /// Der Wirt ruft zu dieser Zeit, auch wenn der Kern eine fruehere Frist
    /// nennt: Er kommt zu spaet (7.3).
    late: bool,
}

/// Die Zeit, zu der der Kern den Tick `tick` des Rahmens rechnet: Tick 0 ist
/// `init`, Tick `k` die Grenze `k - 1` des Kerns.
fn boundary(tick: u64, tick_ns: i64) -> Result<i64, String> {
    i64::try_from(tick.saturating_sub(1))
        .ok()
        .and_then(|k| k.checked_mul(tick_ns))
        .ok_or_else(|| format!("Tick {tick} liegt jenseits der Zeitachse"))
}

/// Der Plan des Testwirts aus dem Stimulus bis Tick `ticks`. Eine Zeile
/// hinter dem Lauf erreicht den Wirt nicht, wie den Interpreter (12.5,
/// FB-620).
fn plan(p: &Program, stimulus: &[Stimulus], ticks: u64) -> Result<Plan, String> {
    let tick_ns = p.config.tick;
    let layout = takt_frame::layout::of(p);
    let mut out = Plan::default();
    let mut overruns: Vec<u64> = Vec::new();
    let event = |out: &mut Plan, tick: u64| -> Result<(), String> {
        if tick > 0 {
            out.calls.entry(boundary(tick, tick_ns)?).or_default().wake = true;
        }
        Ok(())
    };
    for stim in stimulus.iter().filter(|s| s.tick() <= ticks) {
        let tick = stim.tick();
        let now =
            i64::try_from(tick).ok().and_then(|t| t.checked_mul(tick_ns)).ok_or("Tick ausserhalb der Zeitachse")?;
        match stim {
            Stimulus::Input { channel, sample, .. } => {
                let ch = p
                    .channels
                    .iter()
                    .find(|ch| ch.name == *channel && ch.dir == Direction::Input)
                    .ok_or_else(|| format!("t={tick}: kein Eingang `{channel}`"))?;
                // 12.5: von aussen nur ueber den Treiber einer `hw`-Adresse (FB-688).
                admissible::input(ch).map_err(|e| format!("t={tick}: {e}"))?;
                let Binding::Hw(addr) = &ch.binding else {
                    return Err(format!(
                        "t={tick}: `{channel}` hat keine `hw`-Bindung und darum keinen Treiber (8.10)"
                    ));
                };
                let t = sample.t.unwrap_or(now);
                if let Some(Type::Stream(elem)) = p.types.list.get(ch.ty.index()) {
                    let text = sample.value.as_deref().unwrap_or_default();
                    let bytes = crate::deliveries::element_bytes(p, *elem, text)
                        .ok_or_else(|| format!("t={tick}: Element an `{channel}` unlesbar"))?;
                    out.elements.entry(addr.text()).or_default().push(Element { at: now, bytes, t, seq: sample.seq });
                } else {
                    let slot = layout
                        .inputs
                        .iter()
                        .find(|sl| sl.name == *channel)
                        .ok_or_else(|| format!("t={tick}: `{channel}` hat keinen Platz im Abbild"))?;
                    let reading = reading(p, ch.ty, slot, sample, t)
                        .map_err(|e| format!("t={tick}: Lieferung an `{channel}`: {e}"))?;
                    if out.readings.entry(addr.text()).or_default().insert(now, reading).is_some() {
                        return Err(format!(
                            "t={tick}: zwei Lieferungen an `{channel}`; ein Treiber liefert je Tick eine Abtastung (12.6)"
                        ));
                    }
                }
                event(&mut out, tick)?;
            }
            Stimulus::Command { name, .. } => {
                if !p.commands.iter().any(|c| c.name == *name) {
                    return Err(format!("t={tick}: kein Command `{name}`"));
                }
                out.commands.entry(name.clone()).or_default().push(now);
                event(&mut out, tick)?;
            }
            Stimulus::Tune { name, text, .. } => {
                let index = p
                    .params
                    .iter()
                    .position(|q| q.name == *name && q.tunable)
                    .ok_or_else(|| format!("t={tick}: kein Tunable `{name}`"))?;
                let bytes = crate::harness::tune_bytes(p, index, text)
                    .ok_or_else(|| format!("t={tick}: `{text}` ist kein Wert von `{name}`"))?;
                if tick == 0 {
                    return Err(format!(
                        "t=0: `tune {name}` ist ein Startwert (12.5); der Testwirt baut das Programm mit seinen Vorgaben"
                    ));
                }
                out.tunes.push((tick - 1, index, bytes));
                event(&mut out, tick)?;
            }
            Stimulus::Abort { .. } => {
                admissible::at(Event::Abort, tick).map_err(|e| format!("t={tick}: `abort`: {e}"))?;
                out.calls.entry(boundary(tick, tick_ns)?).or_default().abort = true;
                event(&mut out, tick)?;
            }
            Stimulus::Runtime { kind, output, .. } => {
                use takt_mir::machine::RuntimeKind;
                // 12.5: nur ein Runtime-Fault mit Quelle, ab seinem ersten Tick (FB-689).
                let channel = match output {
                    Some(o) => Some(
                        p.channels
                            .iter()
                            .find(|c| c.name == *o && c.dir == Direction::Output)
                            .ok_or_else(|| format!("t={tick}: kein Ausgang `{o}`"))?,
                    ),
                    None => None,
                };
                admissible::runtime(p, *kind, channel)
                    .and_then(|()| admissible::at(Event::Runtime(*kind), tick))
                    .map_err(|e| format!("t={tick}: `runtime {kind:?}`: {e}"))?;
                match (kind, channel) {
                    (RuntimeKind::Driver, Some(ch)) => {
                        let Binding::Hw(addr) = &ch.binding else {
                            return Err(format!("t={tick}: `runtime Driver {}`: kein Treiber (8.10)", ch.name));
                        };
                        // Der Commit des Ticks davor scheitert; der Fault wirkt hier.
                        let before = (now - tick_ns).max(0);
                        out.failing.entry(addr.text()).or_default().push(before);
                        event(&mut out, tick - 1)?;
                        event(&mut out, tick)?;
                    }
                    (RuntimeKind::Hardware, _) => {
                        out.calls.entry(boundary(tick, tick_ns)?).or_default().hardware = true;
                        event(&mut out, tick)?;
                    }
                    (RuntimeKind::Overrun, _) => overruns.push(tick),
                    (other, _) => {
                        return Err(format!(
                            "t={tick}: `runtime {other:?}`: Diesen Runtime-Fault erhebt im Produkt kein Weg des Wirts"
                        ));
                    }
                }
            }
        }
    }
    overruns.sort_unstable();
    overruns.dedup();
    if overruns.windows(2).any(|w| w[1] == w[0] + 1) {
        return Err("`runtime Overrun` in zwei Ticks nacheinander: Der Wirt kann nur einen Tick zu spaet rufen".into());
    }
    for s in overruns {
        // Der Tick davor rechnet mehr als eine Periode zu spaet: Er endet
        // nach dem nominalen Beginn von Tick `s`, und der Kern meldet den
        // Ueberlauf vor dessen Schritt (7.3). Kurz vor der Grenze von `s - 1`
        // weckt der Wirt, damit der Kern dort nicht schlaeft.
        let late = boundary(s, tick_ns)? + 1;
        let earlier = boundary(s - 1, tick_ns)?;
        let mut merged = Call { late: true, ..Call::default() };
        for at in [earlier, boundary(s, tick_ns)?] {
            if let Some(c) = out.calls.remove(&at) {
                merged.wake |= c.wake;
                merged.abort |= c.abort;
                merged.hardware |= c.hardware;
            }
        }
        if out.calls.range(earlier..late).next().is_some() {
            return Err(format!("t={s}: `runtime Overrun` neben einem anderen Ueberlauf"));
        }
        out.calls.insert(late, merged);
        if s == 2 {
            out.start = late;
        } else {
            out.calls.entry(earlier - 1).or_default().wake = true;
        }
    }
    out.tunes.sort_by_key(|(k, _, _)| *k);
    Ok(out)
}

/// Eine Lieferung an einen Eingang mit Platz im Abbild (12.6): Wert in der
/// C-Form des Treibers, Qualitaet und Grund in der Zahl des Abbilds, Alter
/// und Zeitstempel.
fn reading(
    p: &Program,
    ty: takt_mir::TypeId,
    slot: &takt_frame::layout::Slot,
    sample: &takt_interp::trace::SampleText,
    t: i64,
) -> Result<Reading, String> {
    let s = takt_interp::trace::sample_from_text(sample, ty, p)?;
    let quality = match s.quality {
        Quality::Good => 0,
        Quality::Suspect => 1,
        Quality::Stale => 2,
        Quality::Bad => 3,
    };
    let reason = match s.reason {
        Some(Reason::OutOfRange) => 1,
        Some(Reason::Implausible) => 2,
        Some(Reason::Driver) => 3,
        Some(Reason::Node) => 4,
        Some(Reason::Stale) | None => 0,
    };
    // 8.9: Ein Tick-Array unter `N` Samples gilt nicht — `Stale`, ohne Wert,
    // wie `Image::through_edge`.
    let short = matches!((&s.value, p.types.get(ty)), (Some(takt_interp::Value::Samples(items)), Type::Samples { len, .. })
        if items.len() < *len as usize);
    if short {
        return Ok(Reading { value: None, quality: 2, reason: 0, age: 0, t });
    }
    let value = match &s.value {
        None => None,
        Some(v) => Some(match takt_frame::layout::c_type(&slot.ty, slot.signed) {
            Some(_) => {
                let literal = crate::deliveries::literal(p, ty, v).ok_or("Wert ohne C-Form")?;
                format!("*value = {literal};")
            }
            None => {
                let bytes = crate::deliveries::memory(p, ty, &slot.ty, v).ok_or("Wert ohne Speicherform")?;
                let list: Vec<String> = bytes.iter().map(u8::to_string).collect();
                format!(
                    "{{ static const uint8_t d[] = {{ {} }}; memcpy(value, d, (size_t)size < sizeof d ? (size_t)size : sizeof d); }}",
                    list.join(", ")
                )
            }
        }),
    };
    Ok(Reading { value, quality, reason, age: s.age, t })
}

/// Der Testwirt `host.c`.
fn host(p: &Program, run: &Run<'_>) -> Result<String, String> {
    let x = prefix(run)?;
    let upper = x.as_str().to_uppercase();
    let plan = plan(p, run.stimulus, run.ticks)?;
    let layout = takt_frame::layout::of(p);
    let drivers = takt_frame::drivers::of(p, &layout);
    let mut s = String::new();
    let _ = writeln!(s, "/* Testwirt des Differentials (13.8); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "#include <stdint.h>\n#include <stdio.h>\n#include <string.h>\n");
    let _ = writeln!(s, "#include \"takt_sdk.h\"\n#include \"{x}.h\"\n");
    if let Some(mxcsr) = run.mxcsr {
        // Die Flags (5:0) darf jeder setzen; der Rest gehoert dem Wirt (4.2).
        let _ = writeln!(s, "#include <stdlib.h>\n#include <xmmintrin.h>\n");
        let _ = writeln!(s, "static void host_fenv(const char *after) {{");
        let _ = writeln!(s, "    unsigned int now = _mm_getcsr();");
        let _ = writeln!(s, "    if ((now & ~0x3Fu) != {mxcsr:#x}u) {{");
        let _ =
            writeln!(s, "        fprintf(stderr, \"takt: MXCSR nach %s: %#x statt %#x\\n\", after, now, {mxcsr:#x}u);");
        let _ = writeln!(s, "        exit(3);");
        let _ = writeln!(s, "    }}");
        let _ = writeln!(s, "}}\n");
    }
    drivers_c(&mut s, &drivers, &plan, &x);
    tunables_c(&mut s, &plan);
    let persists = takt_mir::persist::max_payload(p).is_some_and(|n| n > 0);
    if persists {
        flash_c(&mut s, p, run.payload)?;
    } else if !run.payload.is_empty() {
        return Err("eine Journal-Nutzlast fuer ein Programm ohne `persist` (5.9)".into());
    }
    let calls: Vec<String> = plan
        .calls
        .iter()
        .map(|(at, c)| {
            let flags = [c.wake, c.abort, c.hardware, c.late].map(|f| u8::from(f).to_string());
            format!("{{ {at}LL, {} }}", flags.join(", "))
        })
        .collect();
    let last_call = plan.calls.keys().next_back().copied().unwrap_or(0);
    let horizon = boundary(run.ticks, p.config.tick)?.max(last_call);
    let _ = writeln!(s, "/* Die Aufrufe, die der Stimulus verlangt: Zeit, Wecken, Abbruch, zu spaet. */");
    let _ = writeln!(s, "struct host_call {{ int64_t at; uint8_t wake, abort, hardware, late; }};");
    let _ = writeln!(
        s,
        "static const struct host_call g_calls[{}] = {{ {} }};",
        calls.len().max(1),
        if calls.is_empty() { "{ 0, 0, 0, 0 }".to_string() } else { calls.join(", ") }
    );
    let _ = writeln!(s, "static const size_t g_ncalls = {};\n", calls.len());
    let _ = writeln!(
        s,
        "static uint8_t host_put(void *ctx, uint8_t byte) {{ return fputc((int)byte, (FILE *)ctx) != EOF; }}"
    );
    let _ = writeln!(s, "static void host_kick(void *ctx) {{ (void)ctx; }}\n");
    // Nach jedem Aufruf des SDK: MXCSR ist wieder der des Wirts.
    let fenv = |s: &mut String, indent: &str, after: &str| {
        if run.mxcsr.is_some() {
            let _ = writeln!(s, "{indent}host_fenv(\"{after}\");");
        }
    };
    let _ = writeln!(s, "int main(void) {{");
    let _ = writeln!(s, "    static uint8_t ring[4096];");
    let _ = writeln!(s, "    static struct {x}_arena arena;");
    let _ = writeln!(s, "    static takt_core core;");
    if let Some(mxcsr) = run.mxcsr {
        let _ = writeln!(s, "    _mm_setcsr({mxcsr:#x}u);");
    }
    if run.fill != 0 {
        let _ = writeln!(s, "    memset(&arena, {:#04x}, sizeof arena);", run.fill);
    }
    let _ = writeln!(s, "    const struct takt_line line = {{ stdout, host_put, NULL }};");
    let _ = writeln!(s, "    const struct takt_watchdog watchdog = {{ NULL, host_kick }};");
    let _ = writeln!(s, "    const struct takt_tunables tunables = {{ NULL, host_tune, NULL }};");
    let _ = writeln!(s, "    int32_t status = takt_line_init(&line, ring, sizeof ring);");
    let _ = writeln!(s, "    if (status == TAKT_OK) {{");
    let _ = writeln!(s, "        takt_line_mark();");
    if persists {
        let _ = writeln!(s, "        static uint8_t current[{upper}_PERSIST_BOUND], stored[{upper}_PERSIST_BOUND];");
        let _ = writeln!(s, "        host_flash_init();");
        let _ = writeln!(
            s,
            "        status = takt_core_init_journaled(&core, &{x}_ops, &arena, NULL, TAKT_PROFILE_BAREMETAL, &watchdog, \
             &host_nvm, current, stored, {upper}_PERSIST_BOUND);"
        );
    } else {
        let _ = writeln!(
            s,
            "        status = takt_core_init(&core, &{x}_ops, &arena, NULL, TAKT_PROFILE_BAREMETAL, &watchdog);"
        );
    }
    fenv(&mut s, "        ", "init");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if (status != TAKT_OK) {{");
    let _ = writeln!(s, "        fprintf(stderr, \"takt: Leitung oder Kern: %d\\n\", (int)status);");
    let _ = writeln!(s, "        return 2;");
    let _ = writeln!(s, "    }}");
    // Logische Zeit (12.11): die Uhr zu jeder Frist und zu jedem Ereignis des
    // Stimulus; jeder Job rechnet zu Ende, bevor sie weiterspringt.
    let _ = writeln!(s, "    int64_t now = {}LL;", plan.start);
    let _ = writeln!(s, "    size_t e = 0;");
    let _ = writeln!(s, "    for (;;) {{");
    let _ = writeln!(s, "        for (; e < g_ncalls && g_calls[e].at <= now; e++) {{");
    let _ = writeln!(s, "            if (g_calls[e].abort) takt_core_abort(&core);");
    let _ = writeln!(s, "            if (g_calls[e].hardware) takt_core_hardware_fault(&core);");
    let _ = writeln!(s, "            if (g_calls[e].wake) takt_core_wake(&core);");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        takt_next next = takt_core_service_with(&core, now, &tunables);");
    fenv(&mut s, "        ", "service");
    let _ = writeln!(s, "        if (next.jobs) {{");
    let _ = writeln!(s, "            takt_core_job_work(&core);");
    fenv(&mut s, "            ", "job_work");
    let _ = writeln!(s, "            continue;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        if (next.ended != 0) break;");
    let _ = writeln!(s, "        int64_t at = next.deadline_ns;");
    let _ = writeln!(s, "        if (e < g_ncalls && (g_calls[e].at < at || g_calls[e].late)) at = g_calls[e].at;");
    let _ = writeln!(s, "        if (at > {horizon}LL) break;");
    let _ = writeln!(s, "        now = at;");
    let _ = writeln!(s, "    }}");
    if persists {
        // Der Stand nach dem Lauf (5.9), wie die Zeile `persist` des Interpreters.
        let _ = writeln!(s, "    takt_core_finish(&core);");
        fenv(&mut s, "    ", "finish");
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "        static uint8_t out[{upper}_PERSIST_BOUND];");
        let _ = writeln!(s, "        int32_t n = {x}_persist_snapshot(&arena, out, (int32_t)sizeof out);");
        let _ = writeln!(s, "        fflush(stdout);");
        let _ = writeln!(s, "        printf(\"t={} persist \");", run.ticks);
        let _ = writeln!(s, "        for (int32_t i = 0; i < n; i++) printf(\"%02x\", out[i]);");
        let _ = writeln!(s, "        printf(\"\\n\");");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    takt_core_report(&core, NULL, NULL);");
    fenv(&mut s, "    ", "report");
    let _ = writeln!(s, "    fflush(stdout);");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    Ok(s)
}

/// Die Treiber des Programms als Testtreiber: Was der Stimulus einer Adresse
/// liefert, geben sie an der Grenze seines Ticks, sonst nichts; ein Ausgang
/// bestaetigt jeden Commit ausser denen, die der Stimulus scheitern laesst,
/// jedes Geraet lebt, ein Sendepuffer meldet keinen Platz, und sein Sender ist
/// fertig (8.8, FB-439).
fn drivers_c(s: &mut String, drivers: &[takt_frame::drivers::Driver], plan: &Plan, x: &Prefix) {
    use takt_frame::drivers::Kind;
    let prototypes = takt_frame::drivers::c_prototypes(drivers, x);
    let _ = writeln!(
        s,
        "/* Ein Element eines Eingabestroms: Zeit seines Ticks, Bytes, Zeitstempel, Folgenummer. */\n\
         struct host_element {{ int64_t at; const uint8_t *bytes; int32_t len; int64_t t, seq; uint8_t has_seq; }};\n"
    );
    for d in drivers {
        let symbol = d.symbol(x);
        let signature = prototypes
            .lines()
            .find(|l| l.contains(&format!(" {symbol}(")))
            .and_then(|l| l.split(';').next())
            .unwrap_or_default()
            .to_string();
        let elements = plan.elements.get(&d.address).map_or(&[][..], Vec::as_slice);
        if d.kind == Kind::Poll && !elements.is_empty() {
            let rows: Vec<String> = elements
                .iter()
                .map(|e| {
                    let bytes: Vec<String> = e.bytes.iter().map(u8::to_string).collect();
                    format!(
                        "{{ {}LL, (const uint8_t[]){{ {} }}, {}, {}LL, {}LL, {} }}",
                        e.at,
                        if bytes.is_empty() { "0".to_string() } else { bytes.join(", ") },
                        e.bytes.len(),
                        e.t,
                        e.seq.unwrap_or(0),
                        u8::from(e.seq.is_some())
                    )
                })
                .collect();
            let _ =
                writeln!(s, "static const struct host_element g_{symbol}[{}] = {{ {} }};", rows.len(), rows.join(", "));
        }
        let _ = writeln!(s, "{signature} {{");
        let _ = writeln!(s, "    (void)user;");
        match d.kind {
            Kind::Input | Kind::Sys | Kind::Block => {
                let _ = writeln!(s, "    (void)value; (void)quality; (void)reason; (void)age; (void)t;");
                if d.kind == Kind::Block {
                    let _ = writeln!(s, "    (void)size;");
                }
                let _ = writeln!(s, "    switch (now) {{");
                // 1 mit Wert, 2 ohne (3.5, 12.6 Zeile 2).
                for (at, r) in plan.readings.get(&d.address).into_iter().flatten() {
                    let (value, got) = r.value.as_ref().map_or((String::new(), 2), |v| (format!("{v} "), 1));
                    let _ = writeln!(
                        s,
                        "    case {at}LL: {value}*quality = {}; *reason = {}; *age = {}LL; *t = {}LL; return {got};",
                        r.quality, r.reason, r.age, r.t
                    );
                }
                let _ = writeln!(s, "    default: return 0;");
                let _ = writeln!(s, "    }}");
            }
            Kind::Poll => {
                if elements.is_empty() {
                    let _ = writeln!(s, "    (void)now; (void)buf; (void)cap; (void)len; (void)t; (void)seq;");
                    let _ = writeln!(s, "    return 0;");
                } else {
                    let (g, n) = (format!("g_{symbol}"), elements.len());
                    let _ = writeln!(s, "    static size_t next = 0;");
                    // Was ein Tick nicht abholte, verfaellt mit ihm: Der Stimulus
                    // nennt die Lieferungen seines Ticks (12.6 Zeile 2).
                    let _ = writeln!(s, "    while (next < {n} && {g}[next].at < now) next++;");
                    let _ = writeln!(s, "    if (next >= {n} || {g}[next].at != now) return 0;");
                    let _ = writeln!(s, "    int32_t n = {g}[next].len < cap ? {g}[next].len : cap;");
                    let _ = writeln!(s, "    if (n > 0) memcpy(buf, {g}[next].bytes, (size_t)n);");
                    let _ = writeln!(s, "    *len = n; *t = {g}[next].t;");
                    let _ = writeln!(s, "    if ({g}[next].has_seq) *seq = {g}[next].seq;");
                    let _ = writeln!(s, "    next++;");
                    let _ = writeln!(s, "    return 1;");
                }
            }
            Kind::Output => {
                let _ = writeln!(s, "    (void)value;");
                failing_c(s, plan.failing.get(&d.address), "0", "1");
            }
            Kind::Free => {
                // 12.6 Zeile 6: Ein Sendepuffer, der mehr freien Platz meldet,
                // als er hat, ist ueberfahren.
                failing_c(s, plan.failing.get(&d.address), "0x7fffffff", "-1");
            }
            Kind::Idle | Kind::Alive => {
                let _ = writeln!(s, "    (void)now;");
                let _ = writeln!(s, "    return 1;");
            }
            Kind::Command => {
                let at = plan.commands.get(&d.address).map_or(&[][..], Vec::as_slice);
                let _ = writeln!(s, "    switch (now) {{");
                for t in at {
                    let _ = writeln!(s, "    case {t}LL:");
                }
                if !at.is_empty() {
                    let _ = writeln!(s, "        return 1;");
                }
                let _ = writeln!(s, "    default: return 0;");
                let _ = writeln!(s, "    }}");
            }
        }
        let _ = writeln!(s, "}}\n");
    }
}

/// Ein Treiber, der zu den Zeiten `failing` `fail` liefert, sonst `ok`.
fn failing_c(s: &mut String, failing: Option<&Vec<i64>>, fail: &str, ok: &str) {
    let at = failing.map_or(&[][..], Vec::as_slice);
    if at.is_empty() {
        let _ = writeln!(s, "    (void)now;");
        let _ = writeln!(s, "    return {ok};");
        return;
    }
    let _ = writeln!(s, "    switch (now) {{");
    for t in at {
        let _ = writeln!(s, "    case {t}LL:");
    }
    let _ = writeln!(s, "        return {fail};");
    let _ = writeln!(s, "    default: return {ok};");
    let _ = writeln!(s, "    }}");
}

/// Die Tunables des Stimulus als Quelle des Wirts (`struct takt_tunables`,
/// 8.4): je Grenze des Kerns ihre Aenderungen in Reihenfolge.
fn tunables_c(s: &mut String, plan: &Plan) {
    let rows: Vec<String> = plan
        .tunes
        .iter()
        .map(|(k, param, bytes)| {
            let list: Vec<String> = bytes.iter().map(u8::to_string).collect();
            format!(
                "{{ {k}ULL, {param}u, (const uint8_t[]){{ {} }}, {} }}",
                if list.is_empty() { "0".to_string() } else { list.join(", ") },
                bytes.len()
            )
        })
        .collect();
    let _ = writeln!(s, "/* Die Tunables des Stimulus (8.4): Grenze des Kerns, Parameter, Wert. */");
    let _ = writeln!(s, "struct host_tune {{ uint64_t k; uint32_t param; const uint8_t *value; uint32_t len; }};");
    let _ = writeln!(
        s,
        "static const struct host_tune g_tunes[{}] = {{ {} }};",
        rows.len().max(1),
        if rows.is_empty() { "{ 0, 0, 0, 0 }".to_string() } else { rows.join(", ") }
    );
    let _ = writeln!(
        s,
        "static int32_t host_tune(void *ctx, uint64_t k, uint32_t *param, const uint8_t **value, uint32_t *len) {{"
    );
    let _ = writeln!(s, "    static uint64_t at = UINT64_MAX;");
    let _ = writeln!(s, "    static size_t i = 0;");
    let _ = writeln!(s, "    (void)ctx;");
    let _ = writeln!(s, "    if (k != at) {{ at = k; i = 0; }}");
    let _ = writeln!(s, "    for (; i < {}; i++) {{", rows.len());
    let _ = writeln!(s, "        if (g_tunes[i].k != k) continue;");
    let _ = writeln!(s, "        *param = g_tunes[i].param; *value = g_tunes[i].value; *len = g_tunes[i].len;");
    let _ = writeln!(s, "        i++;");
    let _ = writeln!(s, "        return 1;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    (void)param; (void)value; (void)len;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}\n");
}

/// Das Flash im Speicher mit dem Journal des Laufs (5.9): zwei Slots, jeder
/// Vorgang sofort fertig. Die Nutzlast `payload` schreibt das Journal aus
/// `takt-rt-core` vorher hinein, mit dem Logik-Hash des Programms als
/// Schluessel — derselbe Weg, auf dem ein voriger Lauf sie hinterliesse.
fn flash_c(s: &mut String, p: &Program, payload: &[u8]) -> Result<(), String> {
    let image = journal_image(p, payload)?;
    let _ = writeln!(s, "/* Das Flash des Journals (5.9). */");
    let _ = writeln!(s, "#define HOST_SLOT {SLOT}u");
    let _ = writeln!(s, "static uint8_t host_slots[2][HOST_SLOT];");
    let _ = writeln!(s, "static int32_t host_pending = TAKT_NVM_IDLE;");
    for (i, slot) in image.iter().enumerate() {
        let used = slot.iter().rposition(|b| *b != 0xff).map_or(0, |n| n + 1);
        let list: Vec<String> = slot[..used].iter().map(u8::to_string).collect();
        let _ = writeln!(
            s,
            "static const uint8_t host_image{i}[{}] = {{ {} }};",
            used.max(1),
            if list.is_empty() { "0xff".to_string() } else { list.join(", ") }
        );
    }
    let _ = writeln!(s, "static void host_flash_init(void) {{");
    let _ = writeln!(s, "    memset(host_slots, 0xff, sizeof host_slots);");
    for i in 0..2 {
        let _ = writeln!(s, "    memcpy(host_slots[{i}], host_image{i}, sizeof host_image{i});");
    }
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static uint8_t host_erase(void *ctx, uint8_t slot) {{ (void)ctx; memset(host_slots[slot & 1u], 0xff, HOST_SLOT); \
         host_pending = TAKT_NVM_DONE; return 1; }}"
    );
    let _ = writeln!(
        s,
        "static uint8_t host_write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {{\n    \
         (void)ctx;\n    if (offset > HOST_SLOT || len > HOST_SLOT - offset) {{ host_pending = TAKT_NVM_FAILED; return 1; }}\n    \
         memcpy(&host_slots[slot & 1u][offset], bytes, len);\n    host_pending = TAKT_NVM_DONE;\n    return 1;\n}}"
    );
    let _ = writeln!(
        s,
        "static int32_t host_poll(void *ctx) {{ (void)ctx; int32_t state = host_pending; host_pending = TAKT_NVM_IDLE; \
         return state; }}"
    );
    let _ = writeln!(
        s,
        "static uint8_t host_read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {{\n    \
         (void)ctx;\n    if (offset > HOST_SLOT || len > HOST_SLOT - offset) return 0;\n    \
         memcpy(into, &host_slots[slot & 1u][offset], len);\n    return 1;\n}}"
    );
    let _ = writeln!(
        s,
        "static const struct takt_nvm host_nvm = {{ NULL, HOST_SLOT, -1, host_erase, host_write, host_poll, host_read }};\n"
    );
    Ok(())
}

/// Die beiden Slots eines Flashs, auf dem das Journal von `takt-rt-core` die
/// Nutzlast `payload` geschrieben hat; leer bleiben sie geloescht.
fn journal_image(p: &Program, payload: &[u8]) -> Result<[Vec<u8>; 2], String> {
    let device = takt_rt_core::FakeNvm::<SLOT>::new();
    if payload.is_empty() {
        return Ok([device.slot(0).to_vec(), device.slot(1).to_vec()]);
    }
    let hash = takt_mir::hash::logic_hash(p);
    let key = hash.0.first_chunk::<8>().map_or(0, |b| u64::from_le_bytes(*b));
    let mut journal = takt_rt_core::Journal::new(device, key, 0);
    let mut scratch = vec![0u8; payload.len()];
    journal.load(&mut scratch);
    let (mut stored, mut stored_len) = (vec![0u8; payload.len()], 0usize);
    if !journal.flush(payload, &mut stored, &mut stored_len, || true) {
        return Err(format!("das Journal nahm die Nutzlast von {} Byte nicht an (5.9)", payload.len()));
    }
    let device = journal.into_inner();
    Ok([device.slot(0).to_vec(), device.slot(1).to_vec()])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(src: &str) -> Program {
        let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
        let out = takt_sema::compile(src, &options);
        assert!(!out.has_errors(), "{:?}", out.diagnostics);
        out.program.expect("Programm")
    }

    const SRC: &str = "system:
    language = 1
    tick     = 10 ms

command go

input  p : int in 0..9 @ hw(\"i/p\")
output o : int in 0..9 @ hw(\"o/o\") with safe = 0

machine m:
    initial RUN

    state RUN:
        loop:
            o = p.or(0)
";

    fn stimulus(text: &str) -> Vec<Stimulus> {
        Stimulus::from_trace(&takt_interp::Trace::parse(text).expect("lesbar")).expect("Stimulus")
    }

    /// **Jede Zeile des Stimulus hat ihren Weg** (M11 Schritt 9): eine
    /// Lieferung an den Treiber ihrer Adresse an der Grenze ihres Ticks, ein
    /// Command an den Eintrag des Wirts, ein Abbruch vor dem Aufruf, der den
    /// Tick rechnet; zu jedem ruft der Wirt den Kern und weckt ihn.
    #[test]
    fn every_stimulus_line_has_its_path() {
        let p = program(SRC);
        let run = Run { ticks: 10, stimulus: &stimulus("t=3 in p 7\nt=4 cmd go\nt=5 abort\n"), ..Run::default() };
        let host = host(&p, &run).expect("Testwirt");
        assert!(host.contains("case 30000000LL: *value = 7LL; *quality = 0; *reason = 0;"), "{host}");
        assert!(host.contains("uint8_t app_cmd_go(void *user, int64_t now) {"), "{host}");
        assert!(host.contains("case 40000000LL:\n        return 1;"), "{host}");
        assert!(
            host.contains("{ 20000000LL, 1, 0, 0, 0 }, { 30000000LL, 1, 0, 0, 0 }, { 40000000LL, 1, 1, 0, 0 }"),
            "{host}"
        );
    }

    /// **Was keinen Weg hat, ist ein Fehler mit Grund** (M11 Schritt 9): eine
    /// zweite Lieferung im selben Tick, ein Runtime-Fault ohne Quelle im
    /// Produkt, ein Ueberlauf vor Tick 2.
    #[test]
    fn a_line_without_a_path_is_an_error() {
        let p = program(SRC);
        for (text, why) in [
            ("t=3 in p 1\nt=3 in p 2\n", "zwei Lieferungen"),
            ("t=3 runtime Node\n", "ohne Knoten"),
            ("t=1 runtime Overrun\n", "erst ab Tick 2"),
        ] {
            let run = Run { ticks: 10, stimulus: &stimulus(text), ..Run::default() };
            let e = host(&p, &run).expect_err(text);
            assert!(e.contains(why), "{text}: {e}");
        }
    }

    /// **Ein Ueberlauf ist ein spaeter Aufruf** (7.3): Der Wirt weckt kurz vor
    /// der Grenze des Ticks davor und ruft den Kern erst eine Periode und eine
    /// Nanosekunde danach; die Grenze dazwischen ruft er nicht.
    #[test]
    fn an_overrun_is_a_late_call() {
        let p = program(SRC);
        let run = Run { ticks: 10, stimulus: &stimulus("t=5 runtime Overrun\n"), ..Run::default() };
        let plan = plan(&p, run.stimulus, run.ticks).expect("Plan");
        let calls: Vec<(i64, bool, bool)> = plan.calls.iter().map(|(at, c)| (*at, c.wake, c.late)).collect();
        assert_eq!(calls, [(29_999_999, true, false), (40_000_001, false, true)]);
    }

    /// Die Zeitzeilen decken die Ticks ab ihrem eigenen bis zum letzten
    /// geschlafenen; ohne `takt end` oder vor dem letzten Tick ist der Lauf
    /// unvollstaendig.
    #[test]
    fn a_run_reaches_its_last_tick() {
        let text = "t=1 time took=0 drift=0 slept=0\nt=2 time took=0 drift=0 slept=8\ntakt end\n";
        assert_eq!(reached(text), Some(10));
        assert!(complete(text, 10).is_ok());
        assert!(complete(text, 11).is_err_and(|e| e.contains("bis Tick 10 von 11")));
        assert!(complete("t=1 time took=0 drift=0 slept=20\n", 10).is_err(), "ohne `takt end`");
    }

    /// Das Journal des Laufs haelt die Nutzlast: Ein Journal ueber den Slots
    /// liest sie zurueck.
    #[test]
    fn the_flash_holds_the_payload() {
        let p = program(SRC);
        let image = journal_image(&p, &[1, 2, 3]).expect("Abbild");
        assert!(image.iter().any(|slot| slot.iter().any(|b| *b != 0xff)), "nichts geschrieben");
        assert!(journal_image(&p, &[]).expect("leer").iter().all(|slot| slot.iter().all(|b| *b == 0xff)));
    }
}
