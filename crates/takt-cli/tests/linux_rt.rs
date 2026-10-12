//! `linux_rt` mit erzeugtem Code (12.2, 12.11, M11 Schritt 17):
//! `examples/rust-host` faehrt Programme der Form `linux` im Tick-Faden des
//! Ports aus `takt-rt-linux`, in Echtzeit, und vergleicht jeden Trace mit dem
//! Interpreter (`examples/rust-host/tests/linux_rt.rs`).

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// **Erzeugter Code laeuft im Tick-Faden unter `linux_rt` wie der
/// Interpreter** (12.2, 12.11, 13.1): `cargo test` im Beispiel ruft ueber
/// `takt_embed::build` dieses Werkzeug fuer die Form `linux`, bindet die
/// Lieferform und faehrt den ausgelassenen Test, der in Echtzeit misst. Er
/// laeuft allein (nextest, `threads-required`), damit kein anderer Test den
/// Tick verspaetet. Die Ausgabe nennt die Zusage jedes Laufs und was ohne
/// Rechte fehlt (`chrt`, `RLIMIT_MEMLOCK`).
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "`linux_rt` gibt es nur unter Linux (12.2)")]
fn generated_code_runs_in_the_tick_thread_like_the_interpreter() {
    let takt = PathBuf::from(env!("CARGO_BIN_EXE_takt"));
    // Dasselbe Zielverzeichnis wie diese Suite: `<ziel>/<profil>/takt`.
    let target = takt.parent().and_then(Path::parent).expect("Zielverzeichnis");
    let run = Command::new("cargo")
        .args(["test", "--offline", "--manifest-path"])
        .arg(root().join("examples/rust-host/Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .args(["--test", "linux_rt", "--", "--ignored", "--nocapture"])
        .env("TAKT", &takt)
        .output()
        .expect("cargo");
    let (stdout, stderr) = (String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    eprintln!("{stderr}");
    assert!(run.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("programs_of_the_linux_form_run_in_their_tick_threads_like_the_interpreter ... ok"),
        "{stdout}"
    );
}
