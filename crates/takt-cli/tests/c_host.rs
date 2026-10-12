//! Takt als Baustein in C (12.11, M11 Schritt 18a): `examples/c-host` mit
//! der Lieferform von `takt build --emit embed` und dem SDK-Archiv
//! (`crates/takt-embed-c`), gegen den Interpreter.
//!
//! Zwei Wege binden dasselbe: clang allein, wie ein Bausystem es am Ende tut,
//! und das CMake-Paket (`tools/cmake`), das nur mit CMake und Ninja laeuft.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

const VALVE: &str = "examples/c-host/takt/valve.takt";

/// So viele Ticks faehrt der C-Wirt: zehn Sekunden, das Ventil oeffnet darin zweimal.
const TICKS: u64 = 1000;

/// Der Trace des Interpreters fuer `program` ueber `ticks`, ohne
/// gespeicherte Werte (5.9), `f32` wie der Rahmen als `f64` (4.2).
fn interpreted(program: &str, ticks: u64) -> String {
    let text = std::fs::read_to_string(root().join(program)).expect("Quelle");
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(&text, &options).program.expect("uebersetzt");
    let run = takt_interp::RunOptions { ticks, ..Default::default() };
    let trace = takt_interp::run(&p, &takt_interp::Trace::default(), &run).expect("Lauf").trace.render();
    takt_conformance::run::widen_f32(&trace, &takt_conformance::run::f32_outputs(&p))
}

/// Die Ausgaenge, die ein Trace nennt.
fn outputs(trace: &str) -> BTreeSet<&str> {
    trace.lines().filter_map(|l| l.split_whitespace().skip_while(|w| *w != "out").nth(1)).collect()
}

/// Der Trace des C-Wirts fuer `program` ueber `ticks` gleicht dem
/// Interpreter, Ausgang fuer Ausgang (13.1); ein Ausgang, den nur eine Seite
/// nennt, ist eine Abweichung (FB-305).
fn agrees(program: &str, ticks: u64, native: &str) {
    let expected = interpreted(program, ticks);
    assert_eq!(outputs(&expected), outputs(native), "Ausgaenge:\n{native}");
    let diffs = takt_conformance::compare(&expected, native);
    assert!(diffs.is_empty(), "{} Abweichungen, etwa {:?}\n{native}", diffs.len(), &diffs[..diffs.len().min(4)]);
}

/// Baut einen C-Wirt wie ein Bausystem: die Lieferform des Programms
/// `program` mit dem Praefix `prefix` nach `dir`, dazu das SDK-Archiv, mit
/// clang zu einem Programm aus `main_c` gebunden. `None` ohne clang, wo
/// `TAKT_ALLOW_MISSING` das erlaubt.
fn c_host(program: &str, prefix: &str, main_c: &Path, dir: &Path) -> Option<PathBuf> {
    let clang = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let out = dir.to_str().expect("Pfad");
    let args =
        ["build", program, "--emit", "embed", "--form", "logical", "--target", host_triple(), "--prefix", prefix];
    let run = takt(&[&args[..], &["--out", out]].concat());
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let sdk = takt_conformance::harness::sdk_library_for(None).unwrap_or_else(|e| panic!("SDK-Archiv: {e}"));
    let lib = dir.join(if cfg!(windows) { format!("{prefix}.lib") } else { format!("lib{prefix}.a") });
    let exe = dir.join(format!("c_host{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new(clang)
        .args(["-std=c11", "-O1", "-Wall", "-Werror"])
        .arg("-I")
        .arg(dir)
        .arg(main_c)
        .arg(&lib)
        .arg(&sdk)
        .args(takt_llvm::toolchain::Clang::HOST_LIBS)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang startet");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    Some(exe)
}

/// Faehrt den C-Wirt `exe` mit `args` und liefert seinen Trace.
fn ran(exe: &Path, args: &[&str]) -> String {
    let run = Command::new(exe).args(args).output().expect("der C-Wirt startet");
    assert!(run.status.success(), "der C-Wirt brach ab ({}): {}", run.status, String::from_utf8_lossy(&run.stderr));
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// **Ein C-Wirt rechnet wie der Interpreter** (12.11, 13.1): `main.c` aus
/// `examples/c-host`, mit clang aus der Lieferform und dem SDK-Archiv
/// gebunden, faehrt das Ventil in logischer Zeit ueber `takt_core_service`.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn a_c_host_agrees_with_the_interpreter() {
    let dir = scratch("takt-c-host");
    let Some(exe) = c_host(VALVE, "valve", &root().join("examples/c-host/main.c"), &dir) else { return };
    let trace = ran(&exe, &[&TICKS.to_string()]);
    agrees(VALVE, TICKS, &trace);
    assert!(trace.contains(" out fills 2"), "das Ventil oeffnete keine zwei Mal:\n{trace}");
}

const PERSIST: &str = "corpus-try/35_persist.takt";

/// Ein C-Wirt fuer `35_persist`: ein Flash im Speicher mit zwei Slots,
/// jeder Vorgang sofort fertig, und zwei Laeufe auf ihm (5.9).
const PERSIST_HOST: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "takt_sdk.h"
#include "persist.h"

uint8_t persist_out_o_count(void *user, int64_t now, int64_t value) { (void)user; (void)now; (void)value; return 1; }
uint8_t persist_out_o_code(void *user, int64_t now, int64_t value) { (void)user; (void)now; (void)value; return 1; }
uint8_t persist_out_o_first(void *user, int64_t now, int64_t value) { (void)user; (void)now; (void)value; return 1; }
uint8_t persist_alive_o(void *user, int64_t now) { (void)user; (void)now; return 1; }

void takt_board_trace(const char *line) { fputs(line, stdout); }
void takt_board_trace_i64(long long value) { printf("%lld ", value); }
void takt_board_trace_u64(unsigned long long value) { printf("%llu ", value); }
void takt_board_trace_f64(double value) { printf("%.17g ", value); }
void takt_board_trace_hex8(unsigned char value) { printf("0x%02x", value); }

#define SLOT 4096u
static uint8_t slots[2][SLOT];
static int32_t pending = TAKT_NVM_IDLE;

static uint8_t erase(void *ctx, uint8_t slot) {
    (void)ctx;
    memset(slots[slot & 1u], 0xff, SLOT);
    pending = TAKT_NVM_DONE;
    return 1;
}

static uint8_t write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    (void)ctx;
    if (offset > SLOT || len > SLOT - offset) {
        pending = TAKT_NVM_FAILED;
        return 1;
    }
    memcpy(&slots[slot & 1u][offset], bytes, len);
    pending = TAKT_NVM_DONE;
    return 1;
}

static int32_t poll(void *ctx) {
    (void)ctx;
    int32_t state = pending;
    pending = TAKT_NVM_IDLE;
    return state;
}

static uint8_t read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    (void)ctx;
    if (offset > SLOT || len > SLOT - offset) {
        return 0;
    }
    memcpy(into, &slots[slot & 1u][offset], len);
    return 1;
}

static const struct takt_nvm nvm = {NULL, SLOT, -1, erase, write, poll, read};
static uint8_t current[PERSIST_PERSIST_BOUND], stored[PERSIST_PERSIST_BOUND];
static struct persist_arena arena;
static takt_core core;

static int run(long long ticks) {
    int32_t status = takt_core_init_journaled(&core, &persist_ops, &arena, NULL, TAKT_PROFILE_BAREMETAL, &nvm,
                                              current, stored, PERSIST_PERSIST_BOUND);
    if (status != TAKT_OK) {
        fprintf(stderr, "takt_core_init_journaled: %d\n", (int)status);
        return 2;
    }
    int64_t now = 0;
    int64_t end = (int64_t)ticks * PERSIST_TICK_NS;
    for (;;) {
        takt_next next = takt_core_service(&core, now);
        if (next.ended != 0 || next.deadline_ns >= end) {
            break;
        }
        now = next.deadline_ns;
    }
    takt_core_finish(&core);
    return 0;
}

int main(int argc, char **argv) {
    long long ticks = argc > 1 ? strtoll(argv[1], NULL, 10) : 60;
    memset(slots, 0xff, sizeof slots);
    if (run(ticks) != 0) {
        return 2;
    }
    fputs("takt zweiter lauf\n", stdout);
    return run(ticks);
}
"#;

/// **Ein C-Wirt behaelt `persist` ueber den Lauf hinaus** (5.9, 12.11): Der
/// Kern mit Journal (`takt_core_init_journaled`) auf einem Geraet des Wirts.
/// Der erste Lauf beginnt mit leerem Journal und rechnet wie der
/// Interpreter; `takt_core_finish` schreibt synchron, und der zweite Lauf
/// auf demselben Geraet beginnt mit dem Stand, mit dem der erste endete.
/// Ein Programm mit `persist` nimmt `takt_core_init` nicht an.
#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn a_c_host_keeps_persist_across_runs() {
    let dir = scratch("takt-c-host-persist");
    let main_c = dir.join("persist_host.c");
    std::fs::write(&main_c, PERSIST_HOST).expect("Quelle");
    let Some(exe) = c_host(PERSIST, "persist", &main_c, &dir) else { return };
    let trace = ran(&exe, &["60"]);
    let (first, second) = trace.split_once("takt zweiter lauf").expect("zwei Laeufe");
    agrees(PERSIST, 60, first);
    let count = |text: &str, last: bool| {
        let mut values = text.lines().filter_map(|l| Some(l.split_once(" out count ")?.1.trim().to_string()));
        if last { values.next_back() } else { values.next() }
    };
    let end = count(first, true).expect("`count` im ersten Lauf");
    assert_ne!(end, "3", "der erste Lauf zaehlte nicht hoch:\n{first}");
    assert_eq!(count(second, false), Some(end), "der zweite Lauf beginnt nicht beim Stand des ersten:\n{second}");
}

/// **Das CMake-Paket bindet dasselbe** (12.11, `tools/cmake`):
/// `find_package(Takt)` und `takt_add_program` bauen `examples/c-host` mit
/// Ninja und clang. CMake und Ninja gibt es nur auf manchen Rechnern
/// (`TAKT_CMAKE`, `TAKT_NINJA`; die Werkzeuge der Einbettung liegen unter
/// `D:\takt-tools`).
#[test]
#[ignore = "CMake und Ninja: TAKT_CMAKE, TAKT_NINJA; mit --ignored"]
fn the_cmake_package_builds_the_c_host() {
    let cmake = std::env::var("TAKT_CMAKE").expect("TAKT_CMAKE nennt cmake");
    let ninja = std::env::var("TAKT_NINJA").expect("TAKT_NINJA nennt ninja");
    let clang = find().path().cloned().expect("clang");
    let sdk = takt_conformance::harness::sdk_library_for(None).unwrap_or_else(|e| panic!("SDK-Archiv: {e}"));
    let build = scratch("takt-c-host-cmake");
    let configure = Command::new(&cmake)
        .arg("-S")
        .arg(root().join("examples/c-host"))
        .arg("-B")
        .arg(&build)
        .args(["-G", "Ninja"])
        .arg(format!("-DCMAKE_MAKE_PROGRAM={ninja}"))
        .arg(format!("-DCMAKE_C_COMPILER={}", clang.display()))
        .arg(format!("-DTakt_DIR={}", root().join("tools/cmake").display()))
        .arg(format!("-DTAKT_EXECUTABLE={}", env!("CARGO_BIN_EXE_takt")))
        .arg(format!("-DTAKT_SDK_LIBRARY={}", sdk.display()))
        .output()
        .expect("cmake startet");
    assert!(configure.status.success(), "{}", String::from_utf8_lossy(&configure.stderr));
    let made = Command::new(&cmake).arg("--build").arg(&build).output().expect("cmake --build");
    assert!(
        made.status.success(),
        "{}{}",
        String::from_utf8_lossy(&made.stdout),
        String::from_utf8_lossy(&made.stderr)
    );
    let exe = build.join(format!("c_host{}", std::env::consts::EXE_SUFFIX));
    agrees(VALVE, TICKS, &ran(&exe, &[&TICKS.to_string()]));
}
