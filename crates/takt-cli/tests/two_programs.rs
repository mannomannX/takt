//! Zwei Programme auf einem Kern im C-Wirt (12.11, M11 Schritt 16): `pair_fast`
//! (1 ms) und `pair_slow` (10 ms) aus den Board-Tests des F401, je mit dem
//! Kern des SDK und seiner Senke, gekoppelt ueber zwei Briefkaesten in C. Der
//! Wirt ruft beide in der Ordnung des Binarys `pair`: zuerst `fast` bis zum
//! Commit von Tick 0, dann `slow`; an einer gemeinsamen Grenze `fast` vor
//! `slow`. Was am Board nur die Zeitachse zeigt, zeigt hier die logische.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use takt_conformance::couple::{self, Writer};
use takt_llvm::toolchain::find;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    dir
}

/// Das Tripel des Wirts, wie ein Bausystem es nennt.
fn host_triple() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

const PROGRAMS: &str = "crates/takt-conformance/tests/programs";

/// So viele Ticks von `fast`: 200 von `slow`.
const TICKS: i64 = 2000;

/// Der Wirt: zwei Kerne, zwei Senken (`fast` nach stdout, `slow` nach stderr), zwei Briefkaesten.
const MAIN: &str = r#"#include <stdint.h>
#include <stdio.h>

#include "fast.h"
#include "slow.h"
#include "takt_sdk.h"

struct box {
    int has;
    int64_t value;
    int64_t stamp;
};

static struct box count_box;
static struct box last_box;

static void put_box(struct box *b, int64_t now, int64_t value) {
    b->has = 1;
    b->value = value;
    b->stamp = now;
}

static uint8_t take_box(const struct box *b, int64_t *value, uint8_t *quality, uint8_t *reason, int64_t *age,
                        int64_t *t) {
    if (b->has == 0) {
        return 0u;
    }
    *value = b->value;
    *quality = 0u;
    *reason = 0u;
    *age = 0;
    *t = b->stamp;
    return 1u;
}

uint8_t fast_in_pair_back(void *user, int64_t now, int64_t *value, uint8_t *quality, uint8_t *reason, int64_t *age,
                          int64_t *t) {
    (void)user;
    (void)now;
    return take_box(&last_box, value, quality, reason, age, t);
}

uint8_t fast_out_pair_count(void *user, int64_t now, int64_t value) {
    (void)user;
    put_box(&count_box, now, value);
    return 1u;
}

uint8_t fast_out_pair_echo(void *user, int64_t now, int64_t value) {
    (void)user;
    (void)now;
    (void)value;
    return 1u;
}

uint8_t fast_alive_pair(void *user, int64_t now) {
    (void)user;
    (void)now;
    return 1u;
}

uint8_t slow_in_pair_seen(void *user, int64_t now, int64_t *value, uint8_t *quality, uint8_t *reason, int64_t *age,
                          int64_t *t) {
    (void)user;
    (void)now;
    return take_box(&count_box, value, quality, reason, age, t);
}

uint8_t slow_out_pair_last(void *user, int64_t now, int64_t value) {
    (void)user;
    put_box(&last_box, now, value);
    return 1u;
}

uint8_t slow_out_pair_delta(void *user, int64_t now, int64_t value) {
    (void)user;
    (void)now;
    (void)value;
    return 1u;
}

uint8_t slow_alive_pair(void *user, int64_t now) {
    (void)user;
    (void)now;
    return 1u;
}

static void kick(void *ctx) { (void)ctx; }

static uint8_t put(void *ctx, uint8_t byte) { return (fputc((int)byte, (FILE *)ctx) != EOF) ? 1u : 0u; }

int main(void) {
    static uint8_t fast_ring[4096];
    static uint8_t slow_ring[4096];
    static struct fast_arena fast_arena;
    static struct slow_arena slow_arena;
    static takt_core fast_core;
    static takt_core slow_core;
    static takt_sink fast_sink;
    static takt_sink slow_sink;
    const struct takt_line fast_line = {stdout, put, NULL};
    const struct takt_line slow_line = {stderr, put, NULL};
    const struct takt_watchdog watchdog = {NULL, kick};
    if ((takt_sink_init(&fast_sink, &fast_line, fast_ring, sizeof fast_ring) != TAKT_OK) ||
        (takt_sink_init(&slow_sink, &slow_line, slow_ring, sizeof slow_ring) != TAKT_OK)) {
        return 3;
    }
    takt_sink_mark(&fast_sink);
    takt_sink_mark(&slow_sink);
    /* Wie das Binary `pair`: vor dem Ursprung (-1 ns) jedes Programm bis zum
     * Commit von Tick 0, `fast` zuerst; erst danach tastet `slow` ab (12.11). */
    if (takt_core_init(&fast_core, &fast_ops, &fast_arena, NULL, &fast_sink, TAKT_PROFILE_SHARED, &watchdog) != TAKT_OK) {
        return 4;
    }
    takt_next fast_next = takt_core_service(&fast_core, -1);
    if (takt_core_init(&slow_core, &slow_ops, &slow_arena, NULL, &slow_sink, TAKT_PROFILE_SHARED, &watchdog) != TAKT_OK) {
        return 5;
    }
    takt_next slow_next = takt_core_service(&slow_core, -1);
    const int64_t end = TICKS * FAST_TICK_NS;
    for (;;) {
        const int64_t t = (fast_next.deadline_ns < slow_next.deadline_ns) ? fast_next.deadline_ns : slow_next.deadline_ns;
        if (t >= end) {
            break;
        }
        /* An einer gemeinsamen Grenze rechnet `fast` zuerst (12.11). */
        if (fast_next.deadline_ns == t) {
            fast_next = takt_core_service(&fast_core, t);
        }
        if (slow_next.deadline_ns == t) {
            slow_next = takt_core_service(&slow_core, t);
        }
    }
    takt_core_report(&fast_core, NULL, NULL);
    takt_core_report(&slow_core, NULL, NULL);
    return 0;
}
"#;

/// Baut ein Programm des Paars mit der Kopplung aus `takt-pair.hw` nach
/// `dir/prefix`.
fn build(dir: &Path, hw: &Path, name: &str, prefix: &str) -> PathBuf {
    let out = dir.join(prefix);
    let program = format!("{PROGRAMS}/{name}");
    let run = takt(&[
        "build",
        &program,
        "--emit",
        "embed",
        "--form",
        "logical",
        "--target",
        host_triple(),
        "--prefix",
        prefix,
        "--hardware",
        hw.to_str().expect("Pfad"),
        "--out",
        out.to_str().expect("Pfad"),
    ]);
    assert!(run.status.success(), "{name}: {}", String::from_utf8_lossy(&run.stderr));
    out
}

/// Faehrt das Paar im C-Wirt und liefert die Traces von `fast` und `slow`.
fn run_pair(label: &str) -> Option<(String, String)> {
    let clang = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let dir = scratch(label);
    let hw = dir.join("pair.hw");
    let pair = std::fs::read_to_string(root().join("crates/takt-bringup-stm32f401/takt-pair.hw")).expect("pair.hw");
    std::fs::write(&hw, format!("# takt-hw 18\n{pair}")).expect("hw");
    let (fast, slow) = (build(&dir, &hw, "pair_fast.takt", "fast"), build(&dir, &hw, "pair_slow.takt", "slow"));
    let lib = |d: &Path, p: &str| d.join(if cfg!(windows) { format!("{p}.lib") } else { format!("lib{p}.a") });
    let main_c = dir.join("main.c");
    std::fs::write(&main_c, MAIN.replace("TICKS * FAST_TICK_NS", &format!("{TICKS}LL * FAST_TICK_NS")))
        .expect("main.c");
    let sdk = takt_conformance::harness::sdk_library_for(None).unwrap_or_else(|e| panic!("SDK-Archiv: {e}"));
    let exe = dir.join(format!("pair{}", std::env::consts::EXE_SUFFIX));
    let built = Command::new(clang)
        .args(["-std=c11", "-O1", "-Wall", "-Werror", "-I"])
        .arg(&fast)
        .arg("-I")
        .arg(&slow)
        .arg(&main_c)
        .arg(lib(&fast, "fast"))
        .arg(lib(&slow, "slow"))
        .arg(&sdk)
        .args(takt_llvm::toolchain::Clang::HOST_LIBS)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang startet");
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let ran = Command::new(&exe).output().expect("das Paar startet");
    assert!(ran.status.success(), "das Paar brach ab ({})", ran.status);
    Some((String::from_utf8_lossy(&ran.stdout).into_owned(), String::from_utf8_lossy(&ran.stderr).into_owned()))
}

/// Der Trace des Interpreters fuer das Programm `name` des Paars ueber
/// `ticks` Ticks mit `stimulus`.
fn interpreted(name: &str, stimulus: &str, ticks: u64) -> String {
    let text = std::fs::read_to_string(root().join(PROGRAMS).join(name)).expect("Quelle");
    let p = takt_sema::compile(&text, &takt_sema::Options::default()).program.expect("uebersetzt");
    let stimulus = takt_interp::Trace::parse(stimulus).expect("Stimulus");
    let options = takt_interp::RunOptions { ticks, inputs: true, ..Default::default() };
    takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render()
}

/// **Zwei Programme, gekoppelt in beide Richtungen, rechnen wie der
/// Interpreter** (12.11): Kein Trace nennt `runtime Overrun` oder einen
/// Fault, und der Stimulus jedes Programms folgt aus dem Trace des anderen
/// nach der Zeitachse (`takt_conformance::couple`).
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn two_coupled_programs_agree_with_the_interpreter() {
    let Some((fast, slow)) = run_pair("takt-two-programs") else { return };
    for (name, trace) in [("fast", &fast), ("slow", &slow)] {
        let bad: Vec<&str> = trace.lines().filter(|l| l.contains(" runtime ") || l.contains(" fault ")).collect();
        assert!(bad.is_empty(), "{name}: {bad:?}\n{}", trace.lines().take(30).collect::<Vec<_>>().join("\n"));
    }
    const MS: i64 = 1_000_000;
    let ticks = u64::try_from(TICKS).expect("Ticks");
    let higher = Writer { tick_ns: MS, boundary: false, higher: true };
    let lower = Writer { tick_ns: 10 * MS, boundary: true, higher: false };
    for (name, native, stimulus, n) in [
        ("pair_fast.takt", &fast, couple::stimulus(&slow, "last", "back", MS, lower, ticks), ticks),
        ("pair_slow.takt", &slow, couple::stimulus(&fast, "count", "seen", 10 * MS, higher, ticks / 10), ticks / 10),
    ] {
        let diffs = takt_conformance::compare(&interpreted(name, &stimulus, n), native);
        assert!(diffs.is_empty(), "{name}: {} Abweichungen, etwa {:?}", diffs.len(), &diffs[..diffs.len().min(4)]);
    }
}
