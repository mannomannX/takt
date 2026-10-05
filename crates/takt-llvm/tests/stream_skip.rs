//! `s.skip()` (8.6) im erzeugten Code: Das ganze Fenster gilt als
//! untersucht, der Cursor rueckt hinter dessen letztes Element — wie im
//! Interpreter (`exec.rs`, `StmtKind::Skip`). Im Modus ENTRY ist das
//! Fenster leer (9.6).

use takt_llvm::toolchain::{Clang, find};

const SKIPPER: &str = "system:
    language = 1
    tick = 10 ms

input rx : stream<line<32>> @ hw(\"uart0/rx\") with max_rate = 100 Hz, framing = lines, capacity = 4

machine m:
    initial RUN
    state RUN:
        loop:
            rx.skip()
";

/// Uebersetzt `src` mit einem Treiber, dessen Strom drei Elemente mit den
/// `seq` 10, 11 und 12 im Fenster traegt, und liefert, was die Runtime als
/// untersucht gemeldet bekommt. Was die Maschine sonst ruft, ist ein
/// leerer Stummel.
fn run_native(name: &str, src: &str, body: &str) -> Option<String> {
    let path = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let host = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let low = takt_llvm::lower::program(&p, host, &takt_llvm::symbols::Prefix::default());
    assert!(low.skipped.is_empty(), "{:?}", low.skipped);
    let bytes = takt_llvm::arena::of(&p).bytes;
    let own = ["@app_stream_count(", "@app_stream_at(", "@app_stream_examined("];
    let stubs: String = low
        .ir
        .lines()
        .filter(|l| l.starts_with("declare") && l.contains("@app_") && !own.iter().any(|o| l.contains(o)))
        .filter_map(|l| l.split("@app_").nth(1)?.split('(').next().map(|n| format!("void app_{n}(void) {{}}\n")))
        .collect();
    let driver = format!(
        "#include <stdio.h>\n#include <string.h>\nstatic _Alignas(8) unsigned char arena[{bytes}];\n{stubs}\
         int app_stream_count(void *a, int s, long long cur) {{ (void)a; (void)s; return cur <= 10 ? 3 : (int)(13 - cur); }}\n\
         long long app_stream_at(void *a, int s, long long cur, int i, void *out) {{\n\
             (void)a; (void)s; memset(out, 0, 12); return (cur < 10 ? 10 : cur) + i; }}\n\
         void app_stream_examined(void *a, int s, int m, long long seq) {{ (void)a; (void)s; (void)m;\n\
             printf(\"%lld\\n\", seq); }}\n\
         void app_m_init_vars(void *);\nvoid app_m_enter(void *);\nvoid app_m_step(void *);\n\
         int main(void) {{\n{body}    return 0;\n}}\n"
    );
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-llvm-skip-{name}"));
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

/// Ein Schritt untersucht bis `seq` 12, das letzte Element des Fensters;
/// der Eintritt (Modus ENTRY) untersucht nichts. Der zweite Schritt sieht
/// ein leeres Fenster und meldet dieselbe Stelle.
#[test]
fn skip_examines_the_whole_window() {
    let body = "    app_m_init_vars(arena); app_m_enter(arena);\n    app_m_step(arena);\n    app_m_step(arena);\n";
    let Some(out) = run_native("window", SKIPPER, body) else { return };
    assert_eq!(out, "12\n12\n");
}
