//! Frist und Vorruecken einer Maschine im Schlaf (9.9, Satz 9.9.1; FB-429).
//!
//! `<m>_deadline` nennt die Basis-Ticks ab jetzt bis zu dem Tick, in dem
//! die naechste `after`-Frist feuert, und `<m>_advance(n)` rechnet die
//! Aktivierungen in den uebersprungenen Ticks nach — beides aus dem
//! globalen Tick (`now` der Runtime), weil die Maschine nur in den Ticks
//! `t % Periode == Phase` laeuft (7.2). Der Interpreter rechnet so in
//! `Run::earliest_deadline` und `Sim::skip_tick`; den ganzen Lauf mit Schlaf
//! vergleicht `takt-conformance/tests/differential.rs`
//! (`virtual_sleep_is_invisible`).

use takt_llvm::toolchain::{Clang, find};

/// Eine Maschine mit Periode 3 und Phase 1 in einem schlafenden Zustand,
/// dessen Frist sieben Aktivierungen braucht.
const SLEEPER: &str = "system:
    language = 1
    tick = 10 ms

output led : bool @ hw(\"o/led\") with safe = false

machine m every 30 ms phase 10 ms:
    initial SLEEP
    state SLEEP idle:
        after 200 ms: -> RUN
    state RUN:
        enter:
            led = true
";

const PERIOD: i64 = 3;
const PHASE: i64 = 1;
/// `ceil(200 ms / 30 ms)`.
const NEED: i64 = 7;

/// Der Tick, in dem die Frist feuert, wenn jetzt `k` ist und die Maschine
/// in `SLEEP` noch keine Aktivierung hatte: die erste Aktivierung nach
/// jetzt, dann die fehlenden (`Run::earliest_deadline`).
fn target(k: i64) -> i64 {
    let first = (k + 1..).find(|t| t % PERIOD == PHASE).unwrap_or(k + 1);
    first + NEED * PERIOD
}

/// Uebersetzt `src` mit einem Treiber, dessen `main` `body` ausfuehrt, und
/// liefert seine Ausgabe. `app_now` liest den Tick `tick` des Treibers; was
/// die Maschine sonst von der Runtime ruft, ist ein leerer Stummel.
fn run_native(name: &str, src: &str, body: &str) -> Option<String> {
    let path = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let host = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let low = takt_llvm::lower::program(&p, host, &takt_llvm::symbols::Prefix::default());
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let bytes = takt_llvm::arena::of(&p).bytes;
    let stubs: String = low
        .ir
        .lines()
        .filter(|l| l.starts_with("declare") && l.contains("@app_") && !l.contains("@app_now("))
        .filter_map(|l| l.split("@app_").nth(1)?.split('(').next().map(|n| format!("void app_{n}(void) {{}}\n")))
        .collect();
    let driver = format!(
        "#include <stdio.h>\n#include <string.h>\nstatic _Alignas(8) unsigned char arena[{bytes}];\n\
         static long long tick;\nlong long app_now(void *a) {{ (void)a; return tick * 10000000LL; }}\n{stubs}\
         void app_m_init_vars(void *);\nvoid app_m_enter(void *);\nlong long app_m_deadline(void *);\n\
         void app_m_advance(void *, long long);\nint main(void) {{\n{body}    return 0;\n}}\n"
    );
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-sleep-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (ll, c, exe) =
        (dir.join("m.ll"), dir.join("treiber.c"), dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" }));
    std::fs::write(&ll, &low.ir).expect("IR");
    std::fs::write(&c, &driver).expect("Treiber");
    let mut cmd = std::process::Command::new(&path);
    let build = Clang::deterministic(&mut cmd)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));
    let out = std::process::Command::new(&exe).output().expect("Lauf");
    Some(String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"))
}

#[test]
fn deadline_and_advance_follow_the_grid_of_the_machine() {
    // Je Zeile: jetzt `k`, Vorruecken um `n`, Frist davor und danach.
    let (mut body, mut want) = (String::new(), String::new());
    for (k, n) in [(0i64, 4i64), (1, 1), (2, 5), (3, 2), (4, 3), (5, 7), (7, 0)] {
        body.push_str(&format!(
            "    memset(arena, 0, sizeof arena); tick = {k};\n\
             printf(\"%lld \", app_m_deadline(arena));\n\
             app_m_advance(arena, {n}); tick = {k} + {n};\n\
             printf(\"%lld\\n\", app_m_deadline(arena));\n"
        ));
        // Nach dem Vorruecken liegt dieselbe Frist um `n` naeher.
        want.push_str(&format!("{} {}\n", target(k) - k, target(k) - (k + n)));
    }
    let Some(out) = run_native("grid", SLEEPER, &body) else { return };
    assert_eq!(out, want);
}

/// Tick 0 zaehlt nur fuer eine Maschine, die in ihm laeuft (`phase = 0`,
/// 7.2): `System::init` schreibt den Zaehler fort, wenn `countdown == 0`.
/// Der Interpreter wechselt ohne Schlaf mit Phase 10 ms in Tick 22, mit
/// Phase 0 in Tick 21; nach der Initialisierung nennt `_deadline` genau
/// diese Ticks. Zaehlte Tick 0 immer mit, fiele die Frist mit Phase eine
/// Periode zu frueh.
#[test]
fn tick_zero_counts_only_for_a_machine_that_runs_in_it() {
    for (name, phase, fires) in [("phase10", "10 ms", 22), ("phase0", "0 ms", 21)] {
        let src = SLEEPER.replace("phase 10 ms", &format!("phase {phase}"));
        let body = "    tick = 0; app_m_init_vars(arena); app_m_enter(arena);\n\
                    printf(\"%lld\\n\", app_m_deadline(arena));\n";
        let Some(out) = run_native(name, &src, body) else { return };
        assert_eq!(out, format!("{fires}\n"), "Phase {phase}");
    }
}

/// Eine gescopte Instanz unter ihrem Besitzer (5.11), wie
/// `corpus-try/63_scoped_instances.takt`.
const SCOPED: &str = "system:
    language = 1
    tick = 10 ms

input  mode : int in 0..2 @ sim(\"i/mode\")
output c : bool @ hw(\"o/c\") with safe = false

machine blip(o: output bool):
    initial LOW
    state LOW:
        enter:
            o = false
        after 30 ms: -> HIGH
    state HIGH:
        enter:
            o = true

machine ctrl:
    initial FIRST
    state FIRST:
        when mode == 1: -> SECOND
    state SECOND:
        instance r = blip(o = c)
        when mode == 0: -> FIRST
";

/// Eine gescopte Instanz, die im Lauf eintritt, ist in ihrem Eintrittstick
/// nicht aktiv (5.11: `active(inst, k)` fragt zu Tick-Beginn); ihr Zaehler
/// beginnt erst danach. Der Interpreter betritt `r` in Tick 8 und wechselt
/// in Tick 12 nach `HIGH` (`sim/63_scoped_instances/switch`), die Frist nach
/// dem Eintritt ist also vier Ticks. Ein Eintritt in Tick 0 zaehlt wie bei
/// jeder Maschine mit Phase 0: drei Ticks.
#[test]
fn a_scoped_instance_does_not_count_its_entry_tick() {
    for (k, fires) in [(0, 3), (8, 4)] {
        let body = format!(
            "    void app_r_init_vars(void *); void app_r_enter(void *); long long app_r_deadline(void *);\n\
             tick = {k}; app_r_init_vars(arena); app_r_enter(arena);\n\
             printf(\"%lld\\n\", app_r_deadline(arena));\n"
        );
        let Some(out) = run_native(&format!("scoped{k}"), SCOPED, &body) else { return };
        assert_eq!(out, format!("{fires}\n"), "Eintritt in Tick {k}");
    }
}
