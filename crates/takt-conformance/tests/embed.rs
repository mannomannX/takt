//! Takt als Baustein in einem fremden Programm (12.11, M11): was der Wirt
//! an seiner Umgebung aendern darf, ohne dass Takt anders rechnet.

mod common;

use takt_conformance::harness;
use takt_conformance::run::{compare, f32_outputs, widen_f32};
use takt_conformance::stimulus::Stimulus;
use takt_conformance::testhost::Run;
use takt_frame::mcu::Frame;
use takt_llvm::symbols::Prefix;
use takt_llvm::toolchain::Clang;

/// MXCSR des Wirts: Flush-to-Zero, alle Ausnahmen maskiert,
/// Denormals-are-Zero — alles, was die IEEE-Vorgabe nicht ist —, je in
/// einem der vier Rundungsmodi (Bits 13 und 14): naechste, gegen minus
/// unendlich, gegen plus unendlich, gegen null.
const HOSTILE: [(&str, u32); 4] = [("RN", 0x9FC0), ("RD", 0xBFC0), ("RU", 0xDFC0), ("RZ", 0xFFC0)];

/// Uebersetzt Programm, MCU-Rahmen mit Stummeln und `main` fuer den Wirt,
/// laeuft und liefert die Standardausgabe.
fn run_frame(clang: &std::path::Path, p: &takt_mir::Program, label: &str, main: &str) -> Result<String, String> {
    run_frame_with(clang, p, label, main, None)
}

/// Wie [`run_frame`], mit einer Hardware-Konfiguration fuer den Rahmen
/// (8.10): `guard` und `jitter` je Ausgang.
fn run_frame_with(
    clang: &std::path::Path,
    p: &takt_mir::Program,
    label: &str,
    main: &str,
    hardware: Option<&takt_mir::hardware::Hardware>,
) -> Result<String, String> {
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-embed-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (ll, c, main_c, exe) = (
        dir.join("programm.ll"),
        dir.join("rahmen.c"),
        dir.join("main.c"),
        dir.join(format!("lauf{}", std::env::consts::EXE_SUFFIX)),
    );
    std::fs::write(&ll, takt_llvm::lower::program(p, host.triple, &Prefix::default()).ir).map_err(|e| e.to_string())?;
    let frame = takt_frame::mcu::build_with(p, Frame { stubs: true, hardware, ..Default::default() });
    std::fs::write(&c, frame.source).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("app.h"), frame.header).map_err(|e| e.to_string())?;
    std::fs::write(&main_c, main).map_err(|e| e.to_string())?;
    let natives = harness::native_library()?;
    let mut cmd = std::process::Command::new(clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .args([&ll, &c, &main_c, &natives])
        .arg("-o")
        .arg(&exe)
        .output()
        .map_err(|e| e.to_string())?;
    if !build.status.success() {
        return Err(String::from_utf8_lossy(&build.stderr).into_owned());
    }
    let run = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
    if !run.status.success() {
        return Err(format!("{} {}", run.status, String::from_utf8_lossy(&run.stderr)));
    }
    Ok(String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n"))
}

/// Programme ausserhalb der Suite `ieee`, deren Rahmen mit Stroemen,
/// geplanten Ausgaben, Monitoren und Schlaf mehr Felder der Arena fuehrt.
const AROUND: [&str; 4] = ["13_framing.takt", "28_scheduled.takt", "47_monitors.takt", "56_idle_timer.takt"];

/// **Der Wirt verstellt seine Umgebung, und Takt rechnet wie der
/// Interpreter** (4.2, 12.11; FB-382). Jeder Lauf der Programme der Suite
/// `ieee` und aus [`AROUND`] — still, mit Stimulus, mit den erzeugten
/// Eingaben und auf den Pfaden des Solvers ([`takt_conformance::cases`]) —
/// laeuft im Produktrahmen mit dem Kern des SDK (M11 Schritt 9) unter einem
/// eigenen Praefix, mit MXCSR aus [`HOSTILE`] reihum und der Arena vor
/// `init` mit 0xA5 gefuellt. Nach jedem Aufruf des SDK ist MXCSR wieder der
/// des Wirts, und der Trace gleicht dem des Interpreters bitgenau: Ein
/// Einstieg, der die Umgebung des Wirts erbt oder verstellt, oder ein Feld,
/// das `init` nicht setzt, fiele hier auf. Was im Produkt keinen Weg hat,
/// faehrt auch diese Probe nicht; `GAPS` in `differential.rs` fuehrt es.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "die Probe setzt MXCSR, das es nur auf x86-64 gibt")]
fn a_hostile_host_environment_changes_nothing() {
    let Some(clang) = common::clang() else { return };
    let queue =
        std::sync::Mutex::new(takt_conformance::suites::programs("ieee").into_iter().chain(AROUND).collect::<Vec<_>>());
    let failed = std::sync::Mutex::new(Vec::new());
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..(cores - 1).clamp(1, 4) {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().expect("Warteschlange").pop();
                    let Some(name) = next else { break };
                    failed.lock().expect("Befunde").extend(hostile_runs(&clang, name));
                }
            });
        }
    });
    let failed = failed.into_inner().expect("Befunde");
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Die Laeufe eines Programms unter verstellter Umgebung; je Abweichung eine
/// Meldung.
fn hostile_runs(clang: &Clang, name: &str) -> Vec<String> {
    let p = common::board::corpus(name);
    let f32s = f32_outputs(&p);
    let mut failed = Vec::new();
    for (i, case) in takt_conformance::cases::cases(name, &p).iter().enumerate() {
        let (mode, mxcsr) = HOSTILE[i % HOSTILE.len()];
        let label = format!("{name} ({}, {mode})", case.label);
        let stimulus = takt_interp::Trace::parse(&case.stimulus).expect("Stimulus");
        let Ok(inputs) = Stimulus::from_trace(&stimulus) else { continue };
        let run = Run {
            ticks: case.ticks,
            stimulus: &inputs,
            prefix: Some("wirt"),
            mxcsr: Some(mxcsr),
            fill: 0xA5,
            ..Run::default()
        };
        if takt_conformance::testhost::sources(&p, &run, takt_conformance::testhost::host_triple(), false).is_err() {
            continue;
        }
        let options = takt_interp::RunOptions { ticks: case.ticks, inputs: true, ..Default::default() };
        let interpreted = match takt_interp::run(&p, &stimulus, &options) {
            Ok(r) => widen_f32(&r.trace.render(), &f32s),
            Err(e) => {
                failed.push(format!("{label}: der Interpreter bricht ab: {e:?}"));
                continue;
            }
        };
        match common::run_product(clang, &p, name, &run) {
            Ok(native) => {
                let diffs = compare(&interpreted, &native);
                if !diffs.is_empty() {
                    let shown: Vec<String> = diffs.iter().take(6).map(ToString::to_string).collect();
                    failed.push(format!("{label}: {} Abweichungen\n{}", diffs.len(), shown.join("\n")));
                }
            }
            Err(e) => failed.push(format!("{label}: {e}")),
        }
    }
    failed
}

/// **Ein Ausgangstreiber bekommt die Grenze des Ticks, den er stellt, auch
/// nach einem Schlaf** (8.10, 9.9, 12.6; FB-371). Der Schlaf rueckt die
/// Tickzahl vor, bevor die Schleife den Latch stellt; `commit` muss trotzdem
/// die Zeit des gerechneten Ticks nennen. Ein eigener Treiber fuer `ui/led`
/// merkt sie sich, das `main` rechnet wie die Schleife: Tick, Schlaf nach
/// 9.9, Commit.
#[test]
fn an_output_driver_sees_the_tick_it_commits() {
    let Some(clang) = common::clang_path() else { return };
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-embed-commit-now");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let p = common::board::corpus("56_idle_timer.takt");
    let tick = p.config.tick;
    let (ll, c, main, exe) = (
        dir.join("programm.ll"),
        dir.join("rahmen.c"),
        dir.join("main.c"),
        dir.join(format!("lauf{}", std::env::consts::EXE_SUFFIX)),
    );
    std::fs::write(&ll, takt_llvm::lower::program(&p, host.triple, &Prefix::default()).ir).expect("IR");
    let frame = takt_frame::mcu::build(&p);
    std::fs::write(&c, frame.source).expect("Rahmen");
    std::fs::write(dir.join("app.h"), frame.header).expect("Kopf");
    std::fs::write(
        &main,
        format!(
            r#"#include <stdint.h>
#include <stdio.h>

#include "app.h"

#define T {tick}LL

void takt_trace_text(void *s, const char *t) {{ (void)s; (void)t; }}
void takt_trace_i64(void *s, long long v) {{ (void)s; (void)v; }}
void takt_trace_u64(void *s, unsigned long long v) {{ (void)s; (void)v; }}
void takt_trace_f64(void *s, double v) {{ (void)s; (void)v; }}
void takt_trace_hex8(void *s, unsigned char v) {{ (void)s; (void)v; }}

static int64_t seen = -1;
uint8_t app_out_ui_led(void *user, int64_t now, uint8_t value) {{ (void)user; (void)value; seen = now; return 1; }}
uint8_t app_alive_ui(void *user, int64_t now) {{ (void)user; (void)now; return 1; }}

static struct app_arena arena;

int main(void) {{
    int64_t k = 1, slept = 0;
    app_init(&arena, 0, 0);
    while (k <= 300) {{
        int64_t n = 0;
        app_tick(&arena, k);
        if (app_idle(&arena)) {{
            int64_t d = app_deadline(&arena) - (k + 1) * T;
            if (d > T) n = d / T - 1;
        }}
        if (n > 0) {{ app_advance(&arena, n); slept += n; }}
        app_commit(&arena);
        if (seen != k * T) {{
            fprintf(stderr, "Tick %lld: der Treiber bekam now=%lld statt %lld\n", (long long)k, (long long)seen, (long long)(k * T));
            return 3;
        }}
        k += 1 + n;
    }}
    if (slept == 0) {{
        fprintf(stderr, "der Lauf hat nie geschlafen\n");
        return 4;
    }}
    return 0;
}}
"#
        ),
    )
    .expect("main");
    let natives = harness::native_library().expect("Natives");
    let mut cmd = std::process::Command::new(&clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .args([&ll, &c, &main, &natives])
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(run.status.success(), "{}: {}", run.status, String::from_utf8_lossy(&run.stderr));
}

/// Das Programm des Wirts: die Senke ist die Standardausgabe, MXCSR auf
/// `mxcsr`, die Arena vor `init` mit `fill` gefuellt, nach jedem Einstieg die
/// Probe, dass MXCSR so geblieben ist. Die Schleife rechnet wie die der
/// Runtime: Tick, Schlaf nach 9.9, Commit, ein Auftrag an den Job-Kontext.
fn main_c(ticks: u64, tick: i64, mxcsr: u32, fill: u8) -> String {
    format!(
        r#"#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xmmintrin.h>

#include "app.h"

void takt_trace_text(void *s, const char *t) {{ fputs(t, (FILE *)s); }}
void takt_trace_i64(void *s, long long v) {{ fprintf((FILE *)s, "%lld", v); }}
void takt_trace_u64(void *s, unsigned long long v) {{ fprintf((FILE *)s, "%llu", v); }}
void takt_trace_f64(void *s, double v) {{ fprintf((FILE *)s, "%.17g", v); }}
void takt_trace_hex8(void *s, unsigned char v) {{ fprintf((FILE *)s, "0x%02x", v); }}

static struct app_arena arena;
static unsigned char journal[8192];

/* Die Flags (5:0) darf jeder setzen; der Rest muss dem Wirt gehoeren. */
static void unchanged(const char *entry) {{
    unsigned now = _mm_getcsr();
    if ((now & ~0x3Fu) != {mxcsr:#x}u) {{
        fprintf(stderr, "MXCSR nach %s: %#x statt %#x\n", entry, now, {mxcsr:#x}u);
        exit(3);
    }}
}}

int main(void) {{
    int32_t n;
    _mm_setcsr({mxcsr:#x}u);
    memset(&arena, {fill:#04x}, sizeof arena);
    app_init_with(&arena, 0, stdout, 0, 0); unchanged("init_with");
    /* Das Journal einmal hin und zurueck: derselbe Stand, keine andere Rechnung. */
    n = app_persist_snapshot(&arena, journal, (int32_t)sizeof journal); unchanged("persist_snapshot");
    if (n > 0) {{ app_persist_restore(&arena, journal, n); unchanged("persist_restore"); }}
    app_dump(&arena, 1); unchanged("dump");
    for (long long k = 1; k <= {ticks}; ) {{
        long long sleep = 0;
        app_tick(&arena, k); unchanged("tick");
        if (app_idle(&arena)) {{
            long long d = app_deadline(&arena) - (k + 1) * {tick}LL; unchanged("deadline");
            if (d > {tick}LL) sleep = d / {tick}LL - 1;
            if (k + sleep > {ticks}) sleep = {ticks} - k;
        }}
        unchanged("idle");
        if (sleep > 0) {{ app_advance(&arena, sleep); unchanged("advance"); }}
        app_commit(&arena); unchanged("commit");
        app_dump(&arena, 0); unchanged("dump");
        if (app_job_dispatch(&arena)) {{ app_job_work(&arena); unchanged("job_work"); }}
        k += 1 + sleep;
    }}
    fflush(stdout);
    return 0;
}}
"#
    )
}

/// Ein Programm, das in Tick 2 eine Ausgabe auf `at` plant (9.8); ein
/// Timing-Fault fuehrt nach `SAFE` (probe 9), sonst steht probe 2.
fn scheduled(at: &str) -> takt_mir::Program {
    let src = format!(
        "system:\n    language = 1\n    tick     = 1 ms\n\n\
         output o     : bool        @ hw(\"gpio/loop_out\") with safe = false\n\
         output probe : int in 0..9 @ hw(\"o/probe\")       with safe = 0\n\n\
         machine m:\n    fault -> SAFE\n    initial RUN\n\n\
         \x20   state RUN:\n        enter:\n            probe = 1\n        after 2 ms: -> GO\n\n\
         \x20   state GO:\n        enter:\n            at {at}:\n                o = true\n            probe = 2\n\n\
         \x20   state SAFE:\n        enter:\n            probe = 9\n"
    );
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    out.program.unwrap_or_else(|| panic!("{at}: {:?}", out.diagnostics))
}

/// **Die Grenze von `guard` im MCU-Rahmen** (9.8: `T <= now + guard` ist ein
/// Timing-Fault; KON2-009). Ohne Konfiguration ist `guard` null wie in der
/// Simulation: `at` genau jetzt faultet, eine Nanosekunde spaeter nicht —
/// im Rahmen wie im Interpreter. Mit `guard_ns = 4338` liegt die Grenze
/// 4338 ns weiter, und dieselbe Regel gilt dort.
#[test]
fn the_guard_boundary_faults_at_equality() {
    let Some(clang) = common::clang_path() else { return };
    for (at, want) in [("2 ms", "9"), ("2000001 ns", "2")] {
        let p = scheduled(at);
        let native = run_frame(&clang, &p, &format!("guard-{}", want), &main_c(8, p.config.tick, 0x1F80, 0))
            .unwrap_or_else(|e| panic!("{at}: {e}"));
        let options = takt_interp::RunOptions { ticks: 8, ..Default::default() };
        let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
        assert!(interpreted.contains(&format!("t=2 out probe {want}")), "{at}:\n{interpreted}");
        let diffs = compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{at}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    }
    let hw = takt_mir::hardware::parse("# takt-hw 9\n[channel gpio/loop_out]\nguard_ns = 4338\n").expect("lesbar");
    for (at, want) in [("2004338 ns", "9"), ("2004339 ns", "2")] {
        let p = scheduled(at);
        let native =
            run_frame_with(&clang, &p, &format!("guard-hw-{want}"), &main_c(8, p.config.tick, 0x1F80, 0), Some(&hw))
                .unwrap_or_else(|e| panic!("{at}: {e}"));
        assert!(native.contains(&format!("t=2 out probe {want}")), "guard 4338, `at {at}`:\n{native}");
    }
}

/// **Eine geplante Ausgabe haelt den Rahmen wach** (9.9, Konjunkt
/// „keine geplanten Ausgaben"; KON2-009). Die Maschine steht im
/// `idle`-Zustand und hat `o` fuer 5 ms geplant; ihre Frist liegt bei 20 ms.
/// Schliefe der Rahmen bis zur Frist, kaeme die Ausgabe zu spaet; sie steht
/// im selben Tick wie im Interpreter.
#[test]
fn a_scheduled_output_keeps_the_frame_awake() {
    let Some(clang) = common::clang_path() else { return };
    let src = "system:\n    language = 1\n    tick     = 1 ms\n\n\
               output o     : bool        @ hw(\"gpio/loop_out\") with safe = false\n\
               output probe : int in 0..9 @ hw(\"o/probe\")       with safe = 0\n\n\
               machine m:\n    initial WAIT\n\n\
               \x20   state WAIT idle:\n        enter:\n            at 5 ms:\n                o = true\n\
               \x20       after 20 ms: -> DONE\n\n\
               \x20   state DONE:\n        enter:\n            probe = 3\n";
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let native =
        run_frame(&clang, &p, "sched-awake", &main_c(30, p.config.tick, 0x1F80, 0)).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 30, ..Default::default() };
    let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    assert!(interpreted.contains("t=5 out o true") && interpreted.contains("t=20 out probe 3"), "{interpreted}");
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **`o.jitter` ohne Tickraster** (7.5; KON2-009): Mit `tick_granular =
/// false` ist der Jitter der gemessene, ohne den Tick dazu; der Rahmen
/// liefert ihn dem Programm im Lauf.
#[test]
fn the_jitter_without_tick_granularity_is_the_measured_one() {
    let Some(clang) = common::clang_path() else { return };
    let src = "system:\n    language = 1\n    tick = 1 ms\n\n\
               output probe  : bool     @ hw(\"gpio/loop_out\") with safe = false\n\
               output spread : Duration @ hw(\"o/spread\")      with safe = 0 s\n\n\
               machine m:\n    initial RUN\n    state RUN:\n        loop:\n            spread = probe.jitter\n";
    let options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    for (granular, want) in [("false", "36563 ns"), ("true", "1036563 ns")] {
        let hw = takt_mir::hardware::parse(&format!(
            "# takt-hw 9\n[channel gpio/loop_out]\njitter_ns = 36563\ntick_granular = {granular}\n"
        ))
        .expect("lesbar");
        let native =
            run_frame_with(&clang, &p, &format!("jitter-{granular}"), &main_c(3, p.config.tick, 0x1F80, 0), Some(&hw))
                .unwrap_or_else(|e| panic!("{e}"));
        assert!(native.contains(&format!("out spread {want}")), "tick_granular = {granular}:\n{native}");
    }
}
