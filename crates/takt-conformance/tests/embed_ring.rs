//! **Der Ring des C-SDK** (`crates/takt-embed-c/include/takt_ring.h`, 12.6,
//! plan/m11.md 2.7, M11 Schritt 22a): Das Pruefprogramm
//! `examples/c-ring/main.c` laeuft auf dem Wirt, ein Faden schreibt wie
//! eine ISR, der Hauptfaden liest wie `poll`. Den Ring in Rust pruefen die
//! Einheitentests von `takt-embed`.

mod common;

/// Uebersetzt und faehrt das Pruefprogramm; es nennt die erste Pruefung,
/// die scheitert, als Rueckgabewert. Faeden gibt es dort ueber POSIX.
#[cfg(unix)]
#[test]
fn the_c_ring_keeps_every_byte_between_an_isr_and_poll() {
    let Some(clang) = common::clang_path() else { return };
    let root = takt_conformance::repo_root();
    let exe = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("c_ring");
    let built = std::process::Command::new(&clang)
        .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-pthread", "-I"])
        .arg(root.join("crates/takt-embed-c/include"))
        .arg(root.join("examples/c-ring/main.c"))
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang startet");
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = std::process::Command::new(&exe).status().expect("Pruefprogramm startet");
    assert_eq!(run.code(), Some(0), "die Pruefung mit dieser Nummer scheiterte");
}
