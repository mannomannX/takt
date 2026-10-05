//! Die Eintrittssperre des Rahmens (12.11, GEN-041): Ein Eintritt in eine
//! Arena, in der noch ein anderer rechnet, rechnet nicht, sondern ist
//! `Runtime(Overrun)` im naechsten Tick.

use std::path::Path;
use std::process::Command;

use takt_frame::mcu::{Frame, build_with};
use takt_llvm::symbols::Prefix;
use takt_llvm::toolchain::Clang;
use takt_mir::program::{Config, Program};

/// Ein `main`, dessen Trace-Leitung beim ersten Zeichen eines Ticks den Tick
/// noch einmal ruft — wie eine ISR, die den laufenden Schritt unterbricht.
const MAIN: &str = r#"
#include <stdio.h>
#include <stdint.h>
#include "app.h"

static struct app_arena arena;
static int reenter;

void takt_board_trace(const char *text) {
    if (reenter) {
        reenter = 0;
        app_tick(&arena, 99);
    }
    fputs(text, stdout);
}
void takt_board_trace_i64(int64_t v) { printf("%lld ", (long long)v); }
void takt_board_trace_u64(uint64_t v) { printf("%llu ", (unsigned long long)v); }
void takt_board_trace_f64(double v) { printf("%g ", v); }
void takt_board_trace_hex8(uint8_t v) { printf("%02x", v); }

int main(void) {
    app_init(&arena, 0);
    app_overrun(&arena);
    reenter = 1;
    app_tick(&arena, 1);
    printf("nach Tick 1: tick=%lld\n", (long long)arena.tick);
    app_tick(&arena, 2);
    printf("nach Tick 2: tick=%lld\n", (long long)arena.tick);
    return 0;
}
"#;

/// **Ein zweiter Eintritt waehrend eines Ticks rechnet nicht** (12.11): Tick 1
/// meldet den vorgemerkten Ueberlauf, seine Leitung ruft dabei Tick 99; der
/// wird abgewiesen, die Arena bleibt bei Tick 1, und Tick 2 meldet
/// `Runtime(Overrun)` fuer den abgewiesenen Eintritt.
#[test]
fn a_second_entry_during_a_tick_is_refused_and_overruns() {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        panic!("clang fehlt (FB-392)");
    };
    let host = if cfg!(windows) { takt_llvm::Target::X86_64_WINDOWS } else { takt_llvm::Target::X86_64_LINUX };
    let p = Program::new(Config::new(1, 1_000_000));
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-frame-reentry");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let frame = build_with(&p, Frame { stubs: true, ..Frame::default() });
    let (ll, c, main) = (dir.join("programm.ll"), dir.join("rahmen.c"), dir.join("main.c"));
    std::fs::write(&ll, takt_llvm::lower::program(&p, host.triple, &Prefix::default()).ir).expect("IR");
    // Ohne Kanaele liefert der Rand nichts; der echte steht in
    // `takt-native-abi`, das dieser Test nicht baut.
    let settle = "unsigned takt_edge_settle(struct takt_track *t, unsigned c, struct takt_device *d, unsigned nd, \
                  const unsigned *o, struct takt_delivery *l, unsigned n, struct takt_window w, struct takt_event *e, \
                  unsigned m) { (void)t; (void)c; (void)d; (void)nd; (void)o; (void)l; (void)n; (void)w; (void)e; \
                  (void)m; return 0; }\n";
    std::fs::write(&c, format!("{}\n{settle}", frame.source)).expect("Rahmen");
    std::fs::write(dir.join("app.h"), &frame.header).expect("Kopf");
    std::fs::write(&main, MAIN).expect("main");
    let exe = dir.join(format!("lauf{}", std::env::consts::EXE_SUFFIX));
    let mut cmd = Command::new(clang);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .args([&ll, &c, &main])
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().expect("Lauf");
    let out = String::from_utf8_lossy(&run.stdout).replace("\r\n", "\n");
    assert!(run.status.success(), "{out}");
    assert!(out.contains("nach Tick 1: tick=1\n"), "der zweite Eintritt rechnete:\n{out}");
    assert!(out.contains("t=2 runtime Overrun"), "der abgewiesene Eintritt ist ein Ueberlauf:\n{out}");
}
