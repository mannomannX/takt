//! Takt als Baustein in einem fremden Programm (12.11, M11): was der Wirt
//! an seiner Umgebung aendern darf, ohne dass Takt anders rechnet.

mod common;

use takt_conformance::harness;
use takt_conformance::run::{compare, f32_outputs, widen_f32};
use takt_frame::mcu::Frame;
use takt_llvm::symbols::Prefix;
use takt_llvm::toolchain::{Clang, find};

/// Programme, deren Ergebnis an der Fliesskomma-Umgebung haengt:
/// Subnormale in beiden Breiten (Flush-to-Zero, Denormals-are-Zero),
/// korrekt gerundete Mathematik in beiden Breiten und Grenzen von
/// Bereichen (Rundungsmodus).
const PROGRAMS: [&str; 5] = [
    "91_subnormals.takt",
    "105_subnormals_f32.takt",
    "101_correct_math.takt",
    "102_correct_math_f32.takt",
    "94_float_ranges.takt",
];

/// MXCSR des Wirts: Flush-to-Zero, Rundung gegen null, alle Ausnahmen
/// maskiert, Denormals-are-Zero — alles, was die IEEE-Vorgabe nicht ist.
const HOSTILE: u32 = 0xFFC0;

/// **Der Wirt verstellt die Fliesskomma-Umgebung, und Takt rechnet wie der
/// Interpreter** (4.2, 12.11). Ein `main` setzt MXCSR auf [`HOSTILE`] und
/// ruft die Einstiege des MCU-Rahmens; nach jedem muss MXCSR wieder sein
/// eigenes sein, und der Trace gleicht dem des Interpreters bitgenau.
#[test]
fn a_hostile_floating_point_environment_changes_nothing() {
    if !cfg!(target_arch = "x86_64") {
        eprintln!("uebersprungen: die Pruefung setzt MXCSR");
        return;
    }
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang fehlt");
        return;
    };
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let natives = harness::native_library().expect("Natives");
    let mut failed = Vec::new();
    for name in PROGRAMS {
        let dir =
            std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-embed-{}", name.replace('.', "_")));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("Verzeichnis");
        let p = common::board::corpus(name);
        let (ll, c, main, exe) = (
            dir.join("programm.ll"),
            dir.join("rahmen.c"),
            dir.join("main.c"),
            dir.join(format!("lauf{}", std::env::consts::EXE_SUFFIX)),
        );
        std::fs::write(&ll, takt_llvm::lower::program(&p, host.triple, &Prefix::default()).ir).expect("IR");
        std::fs::write(&c, takt_frame::mcu::build_with(&p, Frame { stubs: true, ..Default::default() }).source)
            .expect("Rahmen");
        std::fs::write(&main, main_c(common::board::TICKS)).expect("main");
        let mut cmd = std::process::Command::new(&clang);
        let build = Clang::deterministic(&mut cmd)
            .args(["-Wno-override-module", "-O1"])
            .args([&ll, &c, &main, &natives])
            .arg("-o")
            .arg(&exe)
            .output()
            .expect("clang");
        assert!(build.status.success(), "{name}: {}", String::from_utf8_lossy(&build.stderr));
        let run = std::process::Command::new(&exe).output().expect("Lauf");
        if !run.status.success() {
            failed.push(format!("{name}: {} {}", run.status, String::from_utf8_lossy(&run.stderr)));
            continue;
        }
        let native = String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n");
        // Ein `f32` schreibt der Rahmen als seinen Wert in `f64` (4.2, FB-356).
        let interpreted = widen_f32(&common::board::run_interpreted(&p), &f32_outputs(&p));
        let diffs = compare(&interpreted, &native);
        if !diffs.is_empty() || !native.lines().any(|l| l.contains(" out ")) {
            let shown: Vec<String> = diffs.iter().take(6).map(ToString::to_string).collect();
            failed.push(format!("{name}: {} Abweichungen\n{}", diffs.len(), shown.join("\n")));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// **Ein Ausgangstreiber bekommt die Grenze des Ticks, den er stellt, auch
/// nach einem Schlaf** (8.10, 9.9, 12.6; FB-371). Der Schlaf rueckt die
/// Tickzahl vor, bevor die Schleife den Latch stellt; `commit` muss trotzdem
/// die Zeit des gerechneten Ticks nennen. Ein eigener Treiber fuer `ui/led`
/// merkt sie sich, das `main` rechnet wie die Schleife: Tick, Schlaf nach
/// 9.9, Commit.
#[test]
fn an_output_driver_sees_the_tick_it_commits() {
    let Clang::At(clang) = find() else {
        eprintln!("uebersprungen: clang fehlt");
        return;
    };
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
    std::fs::write(&c, takt_frame::mcu::build(&p).source).expect("Rahmen");
    std::fs::write(
        &main,
        format!(
            r#"#include <stdint.h>
#include <stdio.h>

#define T {tick}LL

void takt_board_trace(const char *line) {{ (void)line; }}
void takt_board_trace_i64(long long v) {{ (void)v; }}
void takt_board_trace_u64(unsigned long long v) {{ (void)v; }}
void takt_board_trace_f64(double v) {{ (void)v; }}
void takt_board_trace_hex8(unsigned char v) {{ (void)v; }}

static int64_t seen = -1;
uint8_t app_out_ui_led(void *user, int64_t now, uint8_t value) {{ (void)user; (void)value; seen = now; return 1; }}
uint8_t app_alive_ui(void *user, int64_t now) {{ (void)user; (void)now; return 1; }}

void app_init(void *user);
void app_tick(int64_t k);
void app_commit(void);
uint8_t app_idle(void);
int64_t app_deadline(void);
void app_advance(int64_t n);

int main(void) {{
    int64_t k = 1, slept = 0;
    app_init(0);
    while (k <= 300) {{
        int64_t n = 0;
        app_tick(k);
        if (app_idle()) {{
            int64_t d = app_deadline() - (k + 1) * T;
            if (d > T) n = d / T - 1;
        }}
        if (n > 0) {{ app_advance(n); slept += n; }}
        app_commit();
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

/// Das Programm des Wirts: Leitung auf die Standardausgabe, MXCSR auf
/// [`HOSTILE`], nach jedem Einstieg die Probe, dass es so geblieben ist.
fn main_c(ticks: u64) -> String {
    format!(
        r#"#include <stdio.h>
#include <stdlib.h>
#include <xmmintrin.h>

void takt_board_trace(const char *line) {{ fputs(line, stdout); }}
void takt_board_trace_i64(long long v) {{ printf("%lld ", v); }}
void takt_board_trace_u64(unsigned long long v) {{ printf("%llu ", v); }}
void takt_board_trace_f64(double v) {{ printf("%.17g ", v); }}
void takt_board_trace_hex8(unsigned char v) {{ printf("0x%02x", v); }}

void app_init(void *user);
void app_tick(long long k);
void app_commit(void);
void app_dump(int all);

/* Die Flags (5:0) darf jeder setzen; der Rest muss dem Wirt gehoeren. */
static void unchanged(const char *entry) {{
    unsigned now = _mm_getcsr();
    if ((now & ~0x3Fu) != {HOSTILE:#x}u) {{
        fprintf(stderr, "MXCSR nach %s: %#x statt %#x\n", entry, now, {HOSTILE:#x}u);
        exit(3);
    }}
}}

int main(void) {{
    _mm_setcsr({HOSTILE:#x}u);
    app_init(0); unchanged("init");
    app_dump(1); unchanged("dump");
    for (long long k = 1; k <= {ticks}; k++) {{
        app_tick(k); unchanged("tick");
        app_commit(); unchanged("commit");
        app_dump(0); unchanged("dump");
    }}
    fflush(stdout);
    return 0;
}}
"#
    )
}
